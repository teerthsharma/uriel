use crate::raft::{Entry, HardState, Index, NodeId, Storage, StorageError, Term};
use std::collections::VecDeque;
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;

const CRC_TABLE: [u32; 256] = crc_table();

const fn crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 == 1 { (c >> 1) ^ 0x82F6_3B78 } else { c >> 1 };
            k += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
}

pub fn crc32c(data: &[u8]) -> u32 {
    let mut c = !0u32;
    for b in data {
        c = CRC_TABLE[((c ^ u32::from(*b)) & 0xFF) as usize] ^ (c >> 8);
    }
    !c
}

fn io_err(e: std::io::Error) -> StorageError {
    StorageError::Io(e.to_string())
}

pub trait Disk {
    fn size(&self, file: &str) -> Result<u64, StorageError>;
    fn read_at(&self, file: &str, offset: u64, len: usize) -> Result<Vec<u8>, StorageError>;
    fn append(&mut self, file: &str, bytes: &[u8]) -> Result<(), StorageError>;
    fn truncate(&mut self, file: &str, len: u64) -> Result<(), StorageError>;
    fn sync(&mut self, file: &str) -> Result<(), StorageError>;
    fn replace(&mut self, file: &str, bytes: &[u8]) -> Result<(), StorageError>;
}

#[derive(Debug, Clone)]
pub struct FsDisk {
    dir: PathBuf,
}

impl FsDisk {
    pub fn open(dir: impl Into<PathBuf>) -> Result<Self, StorageError> {
        let dir = dir.into();
        fs::create_dir_all(&dir).map_err(io_err)?;
        Ok(FsDisk { dir })
    }

    fn path(&self, file: &str) -> PathBuf {
        self.dir.join(file)
    }
}

impl Disk for FsDisk {
    fn size(&self, file: &str) -> Result<u64, StorageError> {
        match fs::metadata(self.path(file)) {
            Ok(m) => Ok(m.len()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(0),
            Err(e) => Err(io_err(e)),
        }
    }

    fn read_at(&self, file: &str, offset: u64, len: usize) -> Result<Vec<u8>, StorageError> {
        let mut f = fs::File::open(self.path(file)).map_err(io_err)?;
        f.seek(SeekFrom::Start(offset)).map_err(io_err)?;
        let mut buf = vec![0u8; len];
        f.read_exact(&mut buf).map_err(io_err)?;
        Ok(buf)
    }

    fn append(&mut self, file: &str, bytes: &[u8]) -> Result<(), StorageError> {
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.path(file))
            .map_err(io_err)?;
        f.write_all(bytes).map_err(io_err)
    }

    fn truncate(&mut self, file: &str, len: u64) -> Result<(), StorageError> {
        let f = fs::OpenOptions::new().write(true).open(self.path(file)).map_err(io_err)?;
        f.set_len(len).map_err(io_err)
    }

    fn sync(&mut self, file: &str) -> Result<(), StorageError> {
        match fs::OpenOptions::new().write(true).open(self.path(file)) {
            Ok(f) => f.sync_all().map_err(io_err),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(io_err(e)),
        }
    }

    fn replace(&mut self, file: &str, bytes: &[u8]) -> Result<(), StorageError> {
        let tmp = self.path(&format!("{file}.tmp"));
        let mut f = fs::File::create(&tmp).map_err(io_err)?;
        f.write_all(bytes).map_err(io_err)?;
        f.sync_all().map_err(io_err)?;
        drop(f);
        fs::rename(&tmp, self.path(file)).map_err(io_err)?;
        #[cfg(unix)]
        fs::File::open(&self.dir).and_then(|d| d.sync_all()).map_err(io_err)?;
        Ok(())
    }
}

const LOG: &str = "log";
const HARD: &str = "hard";
const FRAME: usize = 8;
const MAX_RECORD: usize = 64 << 20;
const KIND_NORMAL: u8 = 1;
const HARD_LEN: usize = 8 + 1 + 8 + 8 + 8;

fn frame(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(FRAME + payload.len());
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(&crc32c(payload).to_le_bytes());
    out.extend_from_slice(payload);
    out
}

fn entry_record(e: &Entry) -> Vec<u8> {
    let mut payload = Vec::with_capacity(17 + e.data.len());
    payload.push(KIND_NORMAL);
    payload.extend_from_slice(&e.term.0.to_le_bytes());
    payload.extend_from_slice(&e.index.0.to_le_bytes());
    payload.extend_from_slice(&e.data);
    frame(&payload)
}

fn u64_at(b: &[u8], at: usize) -> u64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(&b[at..at + 8]);
    u64::from_le_bytes(a)
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    let mut a = [0u8; 4];
    a.copy_from_slice(&b[at..at + 4]);
    u32::from_le_bytes(a)
}

fn decode_entry(payload: &[u8]) -> Option<Entry> {
    if payload.len() < 17 || payload[0] != KIND_NORMAL {
        return None;
    }
    Some(Entry {
        term: Term(u64_at(payload, 1)),
        index: Index(u64_at(payload, 9)),
        data: payload[17..].to_vec(),
    })
}

fn read_frame<D: Disk>(disk: &D, file: &str, at: u64, size: u64) -> Option<(Vec<u8>, u64)> {
    if at + FRAME as u64 > size {
        return None;
    }
    let head = disk.read_at(file, at, FRAME).ok()?;
    let len = u32_at(&head, 0) as usize;
    let crc = u32_at(&head, 4);
    let end = at + FRAME as u64 + len as u64;
    if len > MAX_RECORD || end > size {
        return None;
    }
    let payload = disk.read_at(file, at + FRAME as u64, len).ok()?;
    if crc32c(&payload) != crc {
        return None;
    }
    Some((payload, end))
}

fn encode_hard(hs: &HardState, applied: Index) -> Vec<u8> {
    let mut p = Vec::with_capacity(HARD_LEN);
    p.extend_from_slice(&hs.term.0.to_le_bytes());
    p.push(u8::from(hs.vote.is_some()));
    p.extend_from_slice(&hs.vote.map_or(0, |v| v.0).to_le_bytes());
    p.extend_from_slice(&hs.commit.0.to_le_bytes());
    p.extend_from_slice(&applied.0.to_le_bytes());
    frame(&p)
}

fn decode_hard(bytes: &[u8]) -> Option<(HardState, Index)> {
    if bytes.len() != FRAME + HARD_LEN {
        return None;
    }
    let p = &bytes[FRAME..];
    if u32_at(bytes, 0) as usize != HARD_LEN || crc32c(p) != u32_at(bytes, 4) {
        return None;
    }
    let vote = (p[8] == 1).then(|| NodeId(u64_at(p, 9)));
    let hs = HardState { term: Term(u64_at(p, 0)), vote, commit: Index(u64_at(p, 17)) };
    Some((hs, Index(u64_at(p, 25))))
}

#[derive(Debug)]
pub struct FileStorage<D: Disk> {
    disk: D,
    hard: HardState,
    applied: Index,
    hard_dirty: bool,
    log_dirty: bool,
    terms: Vec<Term>,
    offsets: Vec<u64>,
    end: u64,
    cache: VecDeque<Entry>,
    cache_cap: usize,
}

impl<D: Disk> FileStorage<D> {
    pub fn open(disk: D, cache_entries: usize) -> Result<Self, StorageError> {
        let hard_size = disk.size(HARD)?;
        let (hard, applied) = if hard_size == 0 {
            (HardState::default(), Index(0))
        } else {
            let bytes = disk.read_at(HARD, 0, hard_size as usize)?;
            decode_hard(&bytes)
                .ok_or_else(|| StorageError::Corrupt("hard state record is damaged".into()))?
        };
        let mut s = FileStorage {
            disk,
            hard,
            applied,
            hard_dirty: false,
            log_dirty: false,
            terms: Vec::new(),
            offsets: Vec::new(),
            end: 0,
            cache: VecDeque::new(),
            cache_cap: cache_entries,
        };
        s.scan()?;
        if s.hard.commit > s.last_index() {
            return Err(StorageError::Corrupt("committed entries are missing from the log".into()));
        }
        if s.applied > s.hard.commit {
            return Err(StorageError::Corrupt("applied index beyond commit".into()));
        }
        Ok(s)
    }

    fn scan(&mut self) -> Result<(), StorageError> {
        let size = self.disk.size(LOG)?;
        let mut at = 0u64;
        while let Some((payload, next)) = read_frame(&self.disk, LOG, at, size) {
            let Some(e) = decode_entry(&payload) else { break };
            if e.index.0 != self.terms.len() as u64 + 1 {
                break;
            }
            self.terms.push(e.term);
            self.offsets.push(at);
            self.cache_push(e);
            at = next;
        }
        self.end = at;
        if at < size {
            self.disk.truncate(LOG, at)?;
            self.disk.sync(LOG)?;
        }
        Ok(())
    }

    fn cache_push(&mut self, e: Entry) {
        if self.cache_cap == 0 {
            return;
        }
        if self.cache.len() == self.cache_cap {
            self.cache.pop_front();
        }
        self.cache.push_back(e);
    }

    #[cfg(test)]
    fn cache_len(&self) -> usize {
        self.cache.len()
    }
}

impl<D: Disk> Storage for FileStorage<D> {
    fn hard_state(&self) -> HardState {
        self.hard
    }

    fn set_hard_state(&mut self, hs: HardState) -> Result<(), StorageError> {
        if hs != self.hard {
            self.hard = hs;
            self.hard_dirty = true;
        }
        Ok(())
    }

    fn applied(&self) -> Index {
        self.applied
    }

    fn set_applied(&mut self, applied: Index) -> Result<(), StorageError> {
        if applied != self.applied {
            self.applied = applied;
            self.hard_dirty = true;
        }
        Ok(())
    }

    fn last_index(&self) -> Index {
        Index(self.terms.len() as u64)
    }

    fn term_at(&self, index: Index) -> Option<Term> {
        if index.0 == 0 {
            return Some(Term(0));
        }
        self.terms.get(index.0 as usize - 1).copied()
    }

    fn entries(&self, from: Index, to: Index) -> Result<Vec<Entry>, StorageError> {
        let lo = from.0.max(1);
        let hi = to.0.min(self.last_index().0 + 1);
        if lo >= hi {
            return Ok(Vec::new());
        }
        let cached_from = self.cache.front().map_or(u64::MAX, |e| e.index.0);
        let disk_hi = hi.min(cached_from);
        let mut out = Vec::with_capacity((hi - lo) as usize);
        if lo < disk_hi {
            let start = self.offsets[(lo - 1) as usize];
            let stop = self.offsets.get((disk_hi - 1) as usize).copied().unwrap_or(self.end);
            let bytes = self.disk.read_at(LOG, start, (stop - start) as usize)?;
            let mut at = 0usize;
            while at + FRAME <= bytes.len() {
                let len = u32_at(&bytes, at) as usize;
                let payload = bytes
                    .get(at + FRAME..at + FRAME + len)
                    .ok_or_else(|| StorageError::Corrupt("log record overruns its slot".into()))?;
                if crc32c(payload) != u32_at(&bytes, at + 4) {
                    return Err(StorageError::Corrupt("log record checksum mismatch".into()));
                }
                let e = decode_entry(payload)
                    .ok_or_else(|| StorageError::Corrupt("log record is malformed".into()))?;
                out.push(e);
                at += FRAME + len;
            }
        }
        for e in &self.cache {
            if e.index.0 >= lo.max(disk_hi) && e.index.0 < hi {
                out.push(e.clone());
            }
        }
        Ok(out)
    }

    fn append(&mut self, entries: &[Entry]) -> Result<(), StorageError> {
        if entries.is_empty() {
            return Ok(());
        }
        let first = self.last_index().0 + 1;
        let mut buf = Vec::new();
        let mut offsets = Vec::with_capacity(entries.len());
        for (next, e) in (first..).zip(entries) {
            if e.index.0 != next {
                return Err(StorageError::Invalid(format!(
                    "append at {} but expected {next}",
                    e.index.0
                )));
            }
            offsets.push(self.end + buf.len() as u64);
            buf.extend_from_slice(&entry_record(e));
        }
        self.disk.append(LOG, &buf)?;
        self.log_dirty = true;
        self.end += buf.len() as u64;
        self.offsets.extend(offsets);
        self.terms.extend(entries.iter().map(|e| e.term));
        for e in entries {
            self.cache_push(e.clone());
        }
        Ok(())
    }

    fn truncate_from(&mut self, index: Index) -> Result<(), StorageError> {
        if index.0 == 0 {
            return Err(StorageError::Invalid("truncate below the first index".into()));
        }
        if index > self.last_index() {
            return Ok(());
        }
        let cut = self.offsets[(index.0 - 1) as usize];
        self.disk.truncate(LOG, cut)?;
        self.log_dirty = true;
        self.end = cut;
        self.offsets.truncate((index.0 - 1) as usize);
        self.terms.truncate((index.0 - 1) as usize);
        while self.cache.back().is_some_and(|e| e.index >= index) {
            self.cache.pop_back();
        }
        Ok(())
    }

    fn sync(&mut self) -> Result<(), StorageError> {
        if self.log_dirty {
            self.disk.sync(LOG)?;
            self.log_dirty = false;
        }
        if self.hard_dirty {
            self.disk.replace(HARD, &encode_hard(&self.hard, self.applied))?;
            self.hard_dirty = false;
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) use sim::SimDisk;

#[cfg(test)]
mod sim {
    use super::*;
    use crate::rng::Rng;
    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::rc::Rc;

    #[derive(Debug)]
    enum Pend {
        Append(Vec<u8>),
        Truncate(u64),
    }

    fn apply(v: &mut Vec<u8>, op: &Pend) {
        match op {
            Pend::Append(b) => v.extend_from_slice(b),
            Pend::Truncate(n) => v.truncate(*n as usize),
        }
    }

    #[derive(Default)]
    struct Inner {
        durable: BTreeMap<String, Vec<u8>>,
        pending: BTreeMap<String, Vec<Pend>>,
        ops: u64,
        fail_at: Option<u64>,
        dead: bool,
    }

    impl Inner {
        fn view(&self, file: &str) -> Vec<u8> {
            let mut v = self.durable.get(file).cloned().unwrap_or_default();
            if let Some(p) = self.pending.get(file) {
                for op in p {
                    apply(&mut v, op);
                }
            }
            v
        }

        fn op(&mut self) -> Result<(), StorageError> {
            if self.dead {
                return Err(StorageError::Io("disk dead".into()));
            }
            if self.fail_at == Some(self.ops) {
                self.dead = true;
                return Err(StorageError::Io("injected failure".into()));
            }
            self.ops += 1;
            Ok(())
        }
    }

    #[derive(Clone, Default)]
    pub(crate) struct SimDisk(Rc<RefCell<Inner>>);

    impl SimDisk {
        pub(crate) fn ops(&self) -> u64 {
            self.0.borrow().ops
        }

        pub(crate) fn fail_at(&self, at: Option<u64>) {
            let mut s = self.0.borrow_mut();
            s.fail_at = at;
            s.ops = 0;
            s.dead = false;
        }

        pub(crate) fn file(&self, file: &str) -> Vec<u8> {
            self.0.borrow().view(file)
        }

        pub(crate) fn set_file(&self, file: &str, bytes: Vec<u8>) {
            let mut s = self.0.borrow_mut();
            s.pending.remove(file);
            s.durable.insert(file.to_string(), bytes);
        }

        pub(crate) fn crash_drop(&self) {
            let mut s = self.0.borrow_mut();
            s.pending.clear();
            s.dead = false;
            s.fail_at = None;
        }

        pub(crate) fn crash(&self, rng: &mut Rng) {
            let mut s = self.0.borrow_mut();
            let files: Vec<String> = s.pending.keys().cloned().collect();
            for f in files {
                let ops = s.pending.remove(&f).unwrap_or_default();
                let keep = rng.between(0, ops.len() as u64) as usize;
                let mut v = s.durable.remove(&f).unwrap_or_default();
                for (i, op) in ops.into_iter().enumerate().take(keep) {
                    match op {
                        Pend::Append(b) if i + 1 == keep && rng.chance(1, 2) => {
                            let cut = rng.below(b.len() as u64 + 1) as usize;
                            v.extend_from_slice(&b[..cut]);
                        }
                        other => apply(&mut v, &other),
                    }
                }
                s.durable.insert(f, v);
            }
            s.dead = false;
            s.fail_at = None;
        }
    }

    impl Disk for SimDisk {
        fn size(&self, file: &str) -> Result<u64, StorageError> {
            Ok(self.0.borrow().view(file).len() as u64)
        }

        fn read_at(&self, file: &str, offset: u64, len: usize) -> Result<Vec<u8>, StorageError> {
            let v = self.0.borrow().view(file);
            let lo = offset as usize;
            match v.get(lo..lo + len) {
                Some(s) => Ok(s.to_vec()),
                None => Err(StorageError::Io("short read".into())),
            }
        }

        fn append(&mut self, file: &str, bytes: &[u8]) -> Result<(), StorageError> {
            let mut s = self.0.borrow_mut();
            s.op()?;
            s.pending.entry(file.to_string()).or_default().push(Pend::Append(bytes.to_vec()));
            Ok(())
        }

        fn truncate(&mut self, file: &str, len: u64) -> Result<(), StorageError> {
            let mut s = self.0.borrow_mut();
            s.op()?;
            s.pending.entry(file.to_string()).or_default().push(Pend::Truncate(len));
            Ok(())
        }

        fn sync(&mut self, file: &str) -> Result<(), StorageError> {
            let mut s = self.0.borrow_mut();
            s.op()?;
            let v = s.view(file);
            s.pending.remove(file);
            s.durable.insert(file.to_string(), v);
            Ok(())
        }

        fn replace(&mut self, file: &str, bytes: &[u8]) -> Result<(), StorageError> {
            let mut s = self.0.borrow_mut();
            s.op()?;
            s.pending.remove(file);
            s.durable.insert(file.to_string(), bytes.to_vec());
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raft::{Body, MemStorage, Message, Raft, Role};
    use crate::rng::Rng;

    fn open(disk: &SimDisk) -> FileStorage<SimDisk> {
        FileStorage::open(disk.clone(), 4).unwrap()
    }

    fn ids(n: u64) -> Vec<NodeId> {
        (1..=n).map(NodeId).collect()
    }

    fn entry(term: u64, index: u64) -> Entry {
        Entry { term: Term(term), index: Index(index), data: vec![index as u8; (index % 5) as usize + 1] }
    }

    fn vote(from: u64, term: u64) -> Message {
        Message {
            from: NodeId(from),
            to: NodeId(1),
            term: Term(term),
            body: Body::Vote { last_index: Index(0), last_term: Term(0) },
        }
    }

    fn vote_answer<S: Storage>(r: &mut Raft<S>, m: Message) -> Option<bool> {
        r.step(m);
        let ready = r.ready();
        let granted = ready.messages.iter().find_map(|x| match x.body {
            Body::VoteResp { granted } => Some(granted),
            _ => None,
        });
        r.advance(ready);
        granted
    }

    fn record_len(e: &Entry) -> usize {
        8 + 1 + 8 + 8 + e.data.len()
    }

    fn filled(disk: &SimDisk, n: u64) -> Vec<Entry> {
        let mut s = open(disk);
        let all: Vec<Entry> = (1..=n).map(|i| entry(1, i)).collect();
        s.append(&all).unwrap();
        s.sync().unwrap();
        all
    }

    #[test]
    fn crc32c_matches_the_standard_check_value() {
        assert_eq!(crc32c(b"123456789"), 0xE306_9283);
        assert_eq!(crc32c(b""), 0);
    }

    #[test]
    fn a_vote_cast_before_a_crash_is_not_cast_twice_in_the_same_term() {
        let disk = SimDisk::default();
        let mut r = Raft::new(NodeId(1), ids(3), open(&disk), 1).unwrap();
        assert_eq!(vote_answer(&mut r, vote(2, 5)), Some(true));
        disk.crash_drop();
        let mut again = Raft::new(NodeId(1), ids(3), open(&disk), 99).unwrap();
        assert_eq!(again.term(), Term(5));
        assert_eq!(again.storage().hard_state().vote, Some(NodeId(2)));
        assert_eq!(vote_answer(&mut again, vote(3, 5)), Some(false));
        assert_eq!(vote_answer(&mut again, vote(2, 5)), Some(true));
    }

    #[test]
    fn a_torn_last_record_is_truncated_at_every_byte_cut_and_ignored() {
        let last = entry(1, 4);
        let rec = record_len(&last);
        for cut in 0..rec {
            let disk = SimDisk::default();
            filled(&disk, 4);
            let full = disk.file("log");
            let prev_end = full.len() - rec;
            disk.set_file("log", full[..prev_end + cut].to_vec());
            let mut s = open(&disk);
            assert_eq!(s.last_index(), Index(3), "cut {cut}");
            assert_eq!(disk.file("log").len(), prev_end, "cut {cut}");
            let replacement = Entry { term: Term(2), index: Index(4), data: vec![42] };
            s.append(std::slice::from_ref(&replacement)).unwrap();
            s.sync().unwrap();
            let s = open(&disk);
            assert_eq!(s.last_index(), Index(4));
            assert_eq!(s.entries(Index(4), Index(5)).unwrap(), vec![replacement]);
        }
    }

    #[test]
    fn a_flipped_bit_in_the_uncommitted_tail_drops_that_record_and_everything_after() {
        let disk = SimDisk::default();
        let all = filled(&disk, 4);
        let mut bytes = disk.file("log");
        let start_of_third = record_len(&all[0]) + record_len(&all[1]);
        bytes[start_of_third + 12] ^= 0x40;
        disk.set_file("log", bytes);
        let s = open(&disk);
        assert_eq!(s.last_index(), Index(2));
    }

    #[test]
    fn a_flipped_bit_beneath_the_committed_index_is_an_error_not_a_silent_loss() {
        let disk = SimDisk::default();
        let all = filled(&disk, 4);
        {
            let mut s = open(&disk);
            s.set_hard_state(HardState { term: Term(1), vote: None, commit: Index(4) }).unwrap();
            s.sync().unwrap();
        }
        let mut bytes = disk.file("log");
        bytes[record_len(&all[0]) + 12] ^= 0x01;
        disk.set_file("log", bytes);
        assert!(matches!(FileStorage::open(disk.clone(), 4), Err(StorageError::Corrupt(_))));
    }

    #[test]
    fn a_corrupt_hard_state_file_is_an_error_not_a_forgotten_vote() {
        let disk = SimDisk::default();
        {
            let mut s = open(&disk);
            s.set_hard_state(HardState { term: Term(7), vote: Some(NodeId(3)), commit: Index(0) }).unwrap();
            s.sync().unwrap();
        }
        let mut bytes = disk.file("hard");
        let last = bytes.len() - 1;
        bytes[last] ^= 0x10;
        disk.set_file("hard", bytes);
        assert!(matches!(FileStorage::open(disk.clone(), 4), Err(StorageError::Corrupt(_))));
    }

    #[test]
    fn a_crash_between_any_two_sync_steps_never_leaves_commit_beyond_the_log() {
        for fail in 0..4u64 {
            let disk = SimDisk::default();
            let mut s = open(&disk);
            s.append(&(1..=3).map(|i| entry(1, i)).collect::<Vec<_>>()).unwrap();
            s.set_hard_state(HardState { term: Term(1), vote: Some(NodeId(1)), commit: Index(3) }).unwrap();
            disk.fail_at(Some(fail));
            let _ = s.sync();
            disk.crash_drop();
            let reopened = FileStorage::open(disk.clone(), 4).expect("recoverable");
            assert!(reopened.hard_state().commit <= reopened.last_index(), "fail point {fail}");
        }
    }

    #[test]
    fn hard_state_and_applied_survive_a_crash_once_synced_and_not_before() {
        let disk = SimDisk::default();
        let mut s = open(&disk);
        let hs = HardState { term: Term(4), vote: Some(NodeId(2)), commit: Index(0) };
        s.set_hard_state(hs).unwrap();
        s.set_applied(Index(0)).unwrap();
        disk.crash_drop();
        assert_eq!(open(&disk).hard_state(), HardState::default());
        s.sync().unwrap();
        disk.crash_drop();
        assert_eq!(open(&disk).hard_state(), hs);
    }

    #[test]
    fn a_restarted_node_replays_from_the_last_applied_index_not_from_one() {
        let disk = SimDisk::default();
        let mut r = Raft::new(NodeId(1), ids(1), open(&disk), 1).unwrap();
        while r.role() != Role::Leader {
            r.tick();
        }
        for i in 0..10u8 {
            r.propose(vec![i]).unwrap();
        }
        let ready = r.ready();
        assert_eq!(ready.committed_entries.len(), 11);
        r.advance(ready);
        assert_eq!(r.applied_index(), Index(11));
        disk.crash(&mut Rng::new(5));
        let mut again = Raft::new(NodeId(1), ids(1), open(&disk), 2).unwrap();
        assert_eq!(again.applied_index(), Index(11));
        assert!(again.ready().committed_entries.is_empty());
        while again.role() != Role::Leader {
            again.tick();
        }
        again.propose(vec![99]).unwrap();
        let next = again.ready();
        let got: Vec<u64> = next.committed_entries.iter().map(|e| e.index.0).collect();
        assert_eq!(got, vec![12, 13]);
    }

    #[test]
    fn a_storage_failure_stops_the_node_and_releases_nothing() {
        let disk = SimDisk::default();
        let mut r = Raft::new(NodeId(1), ids(3), open(&disk), 1).unwrap();
        disk.fail_at(Some(0));
        r.step(vote(2, 5));
        assert!(r.fault().is_some());
        let ready = r.ready();
        assert!(ready.messages.is_empty());
        assert!(r.propose(vec![1]).is_err());
        disk.crash_drop();
        assert_eq!(open(&disk).hard_state(), HardState::default());
    }

    #[test]
    fn file_storage_agrees_with_the_in_memory_oracle_across_random_operations_and_reopens() {
        for seed in 0..25u64 {
            let mut rng = Rng::new(seed);
            let disk = SimDisk::default();
            let mut file = open(&disk);
            let mut mem = MemStorage::new();
            let mut term = 1u64;
            for step in 0..60 {
                let last = mem.last_index().0;
                match rng.below(4) {
                    0 | 1 => {
                        let n = rng.between(1, 5);
                        if rng.chance(1, 4) {
                            term += 1;
                        }
                        let mut batch = Vec::new();
                        for k in 1..=n {
                            let len = rng.below(20) as usize;
                            batch.push(Entry {
                                term: Term(term),
                                index: Index(last + k),
                                data: rng.bytes(len),
                            });
                        }
                        file.append(&batch).unwrap();
                        mem.append(&batch).unwrap();
                    }
                    2 if last > 0 => {
                        let at = Index(rng.between(1, last));
                        file.truncate_from(at).unwrap();
                        mem.truncate_from(at).unwrap();
                    }
                    _ => {
                        file.sync().unwrap();
                        disk.crash_drop();
                        file = open(&disk);
                    }
                }
                if rng.chance(1, 3) {
                    file.sync().unwrap();
                }
                if file.last_index() != mem.last_index() {
                    file.sync().unwrap();
                    disk.crash_drop();
                    file = open(&disk);
                }
                assert_eq!(file.last_index(), mem.last_index(), "seed {seed} step {step}");
                let end = mem.last_index().0 + 1;
                let lo = rng.between(1, end);
                let hi = rng.between(lo, end);
                assert_eq!(
                    file.entries(Index(lo), Index(hi)).unwrap(),
                    mem.entries(Index(lo), Index(hi)).unwrap(),
                    "seed {seed} step {step} range {lo}..{hi}"
                );
                for i in 0..=mem.last_index().0 + 1 {
                    assert_eq!(file.term_at(Index(i)), mem.term_at(Index(i)));
                }
            }
        }
    }

    #[test]
    fn the_entry_cache_never_exceeds_its_cap_and_older_entries_still_read_back() {
        let disk = SimDisk::default();
        let mut s = FileStorage::open(disk.clone(), 3).unwrap();
        let all: Vec<Entry> = (1..=100).map(|i| entry(1, i)).collect();
        for chunk in all.chunks(7) {
            s.append(chunk).unwrap();
            assert!(s.cache_len() <= 3);
        }
        assert_eq!(s.entries(Index(1), Index(101)).unwrap(), all);
        assert_eq!(s.entries(Index(40), Index(43)).unwrap(), all[39..42].to_vec());
        let none = FileStorage::open(SimDisk::default(), 0).unwrap();
        assert_eq!(none.cache_len(), 0);
    }

    #[test]
    fn a_real_directory_round_trips_and_recovers_from_a_torn_tail() {
        let dir = std::env::temp_dir().join(format!("uriel-kv-fs-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let all: Vec<Entry> = (1..=5).map(|i| entry(2, i)).collect();
        {
            let mut s = FileStorage::open(FsDisk::open(&dir).unwrap(), 2).unwrap();
            s.append(&all).unwrap();
            s.set_hard_state(HardState { term: Term(2), vote: Some(NodeId(3)), commit: Index(4) }).unwrap();
            s.set_applied(Index(2)).unwrap();
            s.sync().unwrap();
        }
        {
            let s = FileStorage::open(FsDisk::open(&dir).unwrap(), 2).unwrap();
            assert_eq!(s.entries(Index(1), Index(6)).unwrap(), all);
            assert_eq!(s.hard_state().vote, Some(NodeId(3)));
            assert_eq!(s.applied(), Index(2));
        }
        let log = dir.join("log");
        let len = fs::metadata(&log).unwrap().len();
        fs::OpenOptions::new().write(true).open(&log).unwrap().set_len(len - 3).unwrap();
        let s = FileStorage::open(FsDisk::open(&dir).unwrap(), 2).unwrap();
        assert_eq!(s.last_index(), Index(4));
        assert_eq!(fs::metadata(&log).unwrap().len(), len - record_len(&all[4]) as u64);
        drop(s);
        let _ = fs::remove_dir_all(&dir);
    }
}
