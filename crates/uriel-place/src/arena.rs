#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Buffer {
    pub id: u32,
    pub size: u64,
    pub align: u64,
    pub first_use: u64,
    pub last_use: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub offsets: Vec<(u32, u64)>,
    pub peak: u64,
}

impl Plan {
    pub fn offset(&self, id: u32) -> Option<u64> {
        self.offsets
            .binary_search_by_key(&id, |&(i, _)| i)
            .ok()
            .map(|k| self.offsets[k].1)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(16 + self.offsets.len() * 12);
        out.extend_from_slice(&self.peak.to_le_bytes());
        out.extend_from_slice(&(self.offsets.len() as u64).to_le_bytes());
        for &(id, off) in &self.offsets {
            out.extend_from_slice(&id.to_le_bytes());
            out.extend_from_slice(&off.to_le_bytes());
        }
        out
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArenaError {
    DuplicateId(u32),
    BadAlignment(u32),
    InvertedLifetime(u32),
    Overflow,
}

impl core::fmt::Display for ArenaError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ArenaError::DuplicateId(i) => write!(f, "duplicate buffer id {i}"),
            ArenaError::BadAlignment(i) => {
                write!(f, "buffer {i} alignment is not a nonzero power of two")
            }
            ArenaError::InvertedLifetime(i) => write!(f, "buffer {i} has last_use < first_use"),
            ArenaError::Overflow => write!(f, "arena offsets overflow u64"),
        }
    }
}

impl std::error::Error for ArenaError {}

pub fn plan_offsets(buffers: &[Buffer]) -> Result<Plan, ArenaError> {
    let mut offsets: Vec<(u32, u64)> = buffers.iter().map(|b| (b.id, 0)).collect();
    offsets.sort();
    Ok(Plan { offsets, peak: 0 })
}

pub fn plan_first_fit(buffers: &[Buffer]) -> Result<Plan, ArenaError> {
    plan_offsets(buffers)
}

pub fn lower_bound(buffers: &[Buffer]) -> u64 {
    let _ = buffers;
    0
}

pub fn plan_arena(items: &[(u64, u64, u64)], align: u64) -> Result<(Vec<u64>, u64), ArenaError> {
    let _ = (items, align);
    Ok((Vec::new(), 0))
}
