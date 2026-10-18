const PER_MILLE: u128 = 1000;
const RESERVE: u128 = 250;
const REDB_CACHE: u128 = 250;
const HOT: u128 = 80;
const WARM: u128 = 80;
const QUERY_CACHE: u128 = 80;
const RAG_INDEX: u128 = 240;
const SESSION: u128 = 20;
const HOT_ENTRY_BYTES: usize = 2048;
const MIN_HOT_ENTRIES: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MemoryBudget {
    total: usize,
}

impl MemoryBudget {
    pub const MIN_BYTES: usize = 16 * 1024 * 1024;

    pub const fn new(total_bytes: usize) -> MemoryBudget {
        let total = if total_bytes < MemoryBudget::MIN_BYTES {
            MemoryBudget::MIN_BYTES
        } else {
            total_bytes
        };
        MemoryBudget { total }
    }

    pub const fn mib(mebibytes: usize) -> MemoryBudget {
        MemoryBudget::new(mebibytes.saturating_mul(1024 * 1024))
    }

    pub const fn total(self) -> usize {
        self.total
    }

    fn share(self, per_mille: u128) -> usize {
        (self.total as u128 * per_mille / PER_MILLE) as usize
    }

    pub fn reserve(self) -> usize {
        self.share(RESERVE)
    }

    pub fn redb_cache(self) -> usize {
        self.share(REDB_CACHE)
    }

    pub fn hot(self) -> usize {
        self.share(HOT)
    }

    pub fn warm(self) -> usize {
        self.share(WARM)
    }

    pub fn query_cache(self) -> usize {
        self.share(QUERY_CACHE)
    }

    pub fn rag_index(self) -> usize {
        self.share(RAG_INDEX)
    }

    pub fn session(self) -> usize {
        self.share(SESSION)
    }

    pub fn hot_entries(self) -> usize {
        (self.hot() / HOT_ENTRY_BYTES).max(MIN_HOT_ENTRIES)
    }
}
