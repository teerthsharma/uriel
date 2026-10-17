use crate::raft::{
    Entry, EntryKind, HardState, Index, NodeId, Snapshot, SnapshotMeta, Storage, StorageError, Term,
};
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
const SNAP: &str = "snap";
const FRAME: usize = 8;
const MAX_RECORD: usize = 64 << 20;
const KIND_HEADER: u8 = 0;
const KIND_NORMAL: u8 = 1;
const KIND_CONF: u8 = 2;
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
    payload.push(if e.kind == EntryKind::Conf { KIND_CONF } else { KIND_NORMAL });
    payload.extend_from_slice(&e.term.0.to_le_bytes());
    payload.extend_from_slice(&e.index.0.to_le_bytes());
    payload.extend_from_slice(&e.data);
    frame(&payload)
}

fn header_record(base: Index, term: Term) -> Vec<u8> {
    let mut payload = vec![KIND_HEADER];
    payload.extend_from_slice(&base.0.to_le_bytes());
    payload.extend_from_slice(&term.0.to_le_bytes());
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
    if payload.len() < 17 {
        return None;
    }
    let kind = match payload[0] {
        KIND_NORMAL => EntryKind::Normal,
        KIND_CONF => EntryKind::Conf,
        _ => return None,
    };
    Some(Entry {
        term: Term(u64_at(payload, 1)),
        index: Index(u64_at(payload, 9)),
        data: payload[17..].to_vec(),
        kind,
    })
}

fn decode_header(payload: &[u8]) -> Option<(Index, Term)> {
    if payload.len() != 17 || payload[0] != KIND_HEADER {
        return None;
    }
    Some((Index(u64_at(payload, 1)), Term(u64_at(payload, 9))))
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

fn encode_snapshot(snap: &Snapshot) -> Vec<u8> {
    let mut meta = Vec::with_capacity(20 + 8 * snap.meta.members.len());
    meta.extend_from_slice(&snap.meta.index.0.to_le_bytes());
    meta.extend_from_slice(&snap.meta.term.0.to_le_bytes());
    meta.extend_from_slice(&(snap.meta.members.len() as u32).to_le_bytes());
    for m in &snap.meta.members {
        meta.extend_from_slice(&m.0.to_le_bytes());
    }
    let mut out = frame(&meta);
    out.extend_from_slice(&frame(&snap.data));
    out
}

fn decode_snapshot_meta(payload: &[u8]) -> Option<SnapshotMeta> {
    if payload.len() < 20 {
        return None;
    }
    let n = u32_at(payload, 16) as usize;
    if payload.len() != 20 + 8 * n {
        return None;
    }
    Some(SnapshotMeta {
        index: Index(u64_at(payload, 0)),
        term: Term(u64_at(payload, 8)),
        members: (0..n).map(|i| NodeId(u64_at(payload, 20 + 8 * i))).collect(),
    })
}

#[derive(Debug)]
pub struct FileStorage<D: Disk> {
    disk: D,
    hard: HardState,
    applied: Index,
    hard_dirty: bool,
    log_dirty: bool,
    base: Index,
    base_term: Term,
    snap: SnapshotMeta,
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
        let snap_size = disk.size(SNAP)?;
        let snap = if snap_size == 0 {
            SnapshotMeta::default()
        } else {
            read_frame(&disk, SNAP, 0, snap_size)
                .and_then(|(p, _)| decode_snapshot_meta(&p))
                .ok_or_else(|| StorageError::Corrupt("snapshot record is damaged".into()))?
        };
        let mut s = FileStorage {
            disk,
            hard,
            applied,
            hard_dirty: false,
            log_dirty: false,
            base: Index(0),
            base_term: Term(0),
            snap,
            terms: Vec::new(),
            offsets: Vec::new(),
            end: 0,
            cache: VecDeque::new(),
            cache_cap: cache_entries,
        };
        s.scan()?;
        s.reconcile()?;
        if s.hard.commit > s.last_index() {
            return Err(StorageError::Corrupt("committed entries are missing from the log".into()));
        }
        if s.applied > s.hard.commit.max(s.base) {
            return Err(StorageError::Corrupt("applied index beyond commit".into()));
        }
        Ok(s)
    }

    fn scan(&mut self) -> Result<(), StorageError> {
        let size = self.disk.size(LOG)?;
        let mut at = 0u64;
        if let Some((payload, next)) = read_frame(&self.disk, LOG, 0, size) {
            if let Some((base, term)) = decode_header(&payload) {
                self.base = base;
                self.base_term = term;
                at = next;
            }
        }
        while let Some((payload, next)) = read_frame(&self.disk, LOG, at, size) {
            let Some(e) = decode_entry(&payload) else { break };
            if e.index.0 != self.base.0 + self.terms.len() as u64 + 1 {
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

    fn reconcile(&mut self) -> Result<(), StorageError> {
        let snap_index = self.snap.index;
        if snap_index < self.base {
            return Err(StorageError::Corrupt("snapshot is older than the log base".into()));
        }
        if snap_index == self.base {
            if snap_index.0 > 0 && self.snap.term != self.base_term {
                return Err(StorageError::Corrupt("snapshot and log base disagree on the term".into()));
            }
            return Ok(());
        }
        let keep =
            snap_index <= self.last_index() && self.term_at(snap_index) == Some(self.snap.term);
        let suffix = if keep {
            self.entries(snap_index.plus(1), self.last_index().plus(1))?
        } else {
            Vec::new()
        };
        self.rewrite_log(snap_index, self.snap.term, suffix)
    }

    fn rewrite_log(&mut self, base: Index, term: Term, suffix: Vec<Entry>) -> Result<(), StorageError> {
        let mut buf = header_record(base, term);
        let mut offsets = Vec::with_capacity(suffix.len());
        for e in &suffix {
            offsets.push(buf.len() as u64);
            buf.extend_from_slice(&entry_record(e));
        }
        self.disk.replace(LOG, &buf)?;
        self.base = base;
        self.base_term = term;
        self.terms = suffix.iter().map(|e| e.term).collect();
        self.offsets = offsets;
        self.end = buf.len() as u64;
        self.log_dirty = false;
        self.cache.clear();
        for e in suffix {
            self.cache_push(e);
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

    fn first_index(&self) -> Index {
        self.base.plus(1)
    }

    fn last_index(&self) -> Index {
        self.base.plus(self.terms.len() as u64)
    }

    fn term_at(&self, index: Index) -> Option<Term> {
        if index == self.base {
            return Some(self.base_term);
        }
        let pos = index.0.checked_sub(self.base.0 + 1)?;
        self.terms.get(pos as usize).copied()
    }

    fn entries(&self, from: Index, to: Index) -> Result<Vec<Entry>, StorageError> {
        let lo = from.0.max(self.base.0 + 1);
        let hi = to.0.min(self.last_index().0 + 1);
        if lo >= hi {
            return Ok(Vec::new());
        }
        let off = self.base.0 + 1;
        let cached_from = self.cache.front().map_or(u64::MAX, |e| e.index.0);
        let disk_hi = hi.min(cached_from);
        let mut out = Vec::with_capacity((hi - lo) as usize);
        if lo < disk_hi {
            let start = self.offsets[(lo - off) as usize];
            let stop = self.offsets.get((disk_hi - off) as usize).copied().unwrap_or(self.end);
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
        if index <= self.base {
            return Err(StorageError::Invalid("truncate at or below the snapshot".into()));
        }
        if index > self.last_index() {
            return Ok(());
        }
        let pos = (index.0 - self.base.0 - 1) as usize;
        let cut = self.offsets[pos];
        self.disk.truncate(LOG, cut)?;
        self.log_dirty = true;
        self.end = cut;
        self.offsets.truncate(pos);
        self.terms.truncate(pos);
        while self.cache.back().is_some_and(|e| e.index >= index) {
            self.cache.pop_back();
        }
        Ok(())
    }

    fn snapshot_meta(&self) -> SnapshotMeta {
        self.snap.clone()
    }

    fn snapshot_data(&self) -> Result<Vec<u8>, StorageError> {
        let size = self.disk.size(SNAP)?;
        if size == 0 {
            return Ok(Vec::new());
        }
        let damaged = || StorageError::Corrupt("snapshot data is damaged".into());
        let (_, mid) = read_frame(&self.disk, SNAP, 0, size).ok_or_else(damaged)?;
        let (data, _) = read_frame(&self.disk, SNAP, mid, size).ok_or_else(damaged)?;
        Ok(data)
    }

    fn install_snapshot(&mut self, snap: Snapshot) -> Result<(), StorageError> {
        if snap.meta.index <= self.base {
            return Ok(());
        }
        self.disk.replace(SNAP, &encode_snapshot(&snap))?;
        self.snap = snap.meta;
        self.reconcile()
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
mod tests {
    use super::*;
    use crate::sim::SimDisk;
    use crate::raft::{Body, MemStorage, Message, Raft, Role};
    use crate::rng::Rng;

    fn open(disk: &SimDisk) -> FileStorage<SimDisk> {
        FileStorage::open(disk.clone(), 4).unwrap()
    }

    fn ids(n: u64) -> Vec<NodeId> {
        (1..=n).map(NodeId).collect()
    }

    fn entry(term: u64, index: u64) -> Entry {
        Entry { term: Term(term), index: Index(index), data: vec![index as u8; (index % 5) as usize + 1], kind: EntryKind::Normal }
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
            let replacement = Entry { term: Term(2), index: Index(4), data: vec![42], kind: EntryKind::Normal };
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

    fn same_state(file: &FileStorage<SimDisk>, mem: &MemStorage, ctx: &str) {
        assert_eq!(file.first_index(), mem.first_index(), "{ctx}");
        assert_eq!(file.last_index(), mem.last_index(), "{ctx}");
        assert_eq!(file.snapshot_meta(), mem.snapshot_meta(), "{ctx}");
        assert_eq!(file.snapshot_data().unwrap(), mem.snapshot_data().unwrap(), "{ctx}");
        for i in 0..=mem.last_index().0 + 1 {
            assert_eq!(file.term_at(Index(i)), mem.term_at(Index(i)), "{ctx} term_at {i}");
        }
        let (lo, hi) = (mem.first_index(), mem.last_index().plus(1));
        assert_eq!(file.entries(lo, hi).unwrap(), mem.entries(lo, hi).unwrap(), "{ctx}");
    }

    #[test]
    fn file_storage_agrees_with_the_in_memory_oracle_across_random_operations_snapshots_and_reopens() {
        for seed in 0..40u64 {
            let mut rng = Rng::new(seed);
            let disk = SimDisk::default();
            let mut file = open(&disk);
            let mut mem = MemStorage::new();
            let mut term = 1u64;
            for step in 0..60 {
                let ctx = format!("seed {seed} step {step}");
                let last = mem.last_index().0;
                let first = mem.first_index().0;
                match rng.below(6) {
                    0..=2 => {
                        let n = rng.between(1, 5);
                        if rng.chance(1, 4) {
                            term += 1;
                        }
                        let mut batch = Vec::new();
                        for k in 1..=n {
                            let len = rng.below(20) as usize;
                            let kind =
                                if rng.chance(1, 5) { EntryKind::Conf } else { EntryKind::Normal };
                            batch.push(Entry {
                                term: Term(term),
                                index: Index(last + k),
                                data: rng.bytes(len),
                                kind,
                            });
                        }
                        file.append(&batch).unwrap();
                        mem.append(&batch).unwrap();
                    }
                    3 if last >= first => {
                        let at = Index(rng.between(first, last));
                        file.truncate_from(at).unwrap();
                        mem.truncate_from(at).unwrap();
                    }
                    4 => {
                        let index = Index(rng.between(first, last + 3));
                        let matching = mem.term_at(index);
                        let snap_term = match matching {
                            Some(t) if rng.chance(1, 2) => t,
                            _ => Term(term + rng.below(3)),
                        };
                        let len = rng.below(30) as usize;
                        let snap = Snapshot {
                            meta: SnapshotMeta {
                                index,
                                term: snap_term,
                                members: vec![NodeId(1), NodeId(1 + rng.below(4))],
                            },
                            data: rng.bytes(len),
                        };
                        file.install_snapshot(snap.clone()).unwrap();
                        mem.install_snapshot(snap).unwrap();
                        term = term.max(snap_term.0);
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
                if file.last_index() != mem.last_index() || file.first_index() != mem.first_index() {
                    file.sync().unwrap();
                    disk.crash_drop();
                    file = open(&disk);
                }
                same_state(&file, &mem, &ctx);
                let end = mem.last_index().0 + 1;
                let lo = rng.between(mem.first_index().0, end);
                let hi = rng.between(lo, end);
                assert_eq!(
                    file.entries(Index(lo), Index(hi)).unwrap(),
                    mem.entries(Index(lo), Index(hi)).unwrap(),
                    "{ctx} range {lo}..{hi}"
                );
            }
        }
    }

    #[test]
    fn a_crash_at_any_point_of_a_snapshot_install_leaves_the_old_log_or_the_new_snapshot_never_a_mix() {
        let all: Vec<Entry> = (1..=20).map(|i| entry(1, i)).collect();
        let snap = Snapshot {
            meta: SnapshotMeta { index: Index(12), term: Term(1), members: vec![NodeId(1), NodeId(2)] },
            data: vec![5, 6, 7],
        };
        for fail in 0..4u64 {
            let disk = SimDisk::default();
            let mut s = open(&disk);
            s.append(&all).unwrap();
            s.sync().unwrap();
            disk.fail_at(Some(fail));
            let _ = s.install_snapshot(snap.clone());
            disk.crash_drop();
            let r = FileStorage::open(disk.clone(), 3).expect("recoverable");
            assert_eq!(r.last_index(), Index(20), "fail point {fail}");
            let meta = r.snapshot_meta();
            if meta.index == Index(0) {
                assert_eq!(r.first_index(), Index(1), "fail point {fail}");
                assert_eq!(r.entries(Index(1), Index(21)).unwrap(), all);
            } else {
                assert_eq!(meta, snap.meta, "fail point {fail}");
                assert_eq!(r.snapshot_data().unwrap(), snap.data);
                assert_eq!(r.first_index(), Index(13), "fail point {fail}");
                assert_eq!(r.term_at(Index(12)), Some(Term(1)));
                assert_eq!(r.entries(Index(13), Index(21)).unwrap(), all[12..].to_vec());
            }
        }
    }

    #[test]
    fn a_snapshot_whose_term_disagrees_with_the_log_discards_the_log_on_reopen() {
        let disk = SimDisk::default();
        let mut s = open(&disk);
        s.append(&(1..=6).map(|i| entry(1, i)).collect::<Vec<_>>()).unwrap();
        s.sync().unwrap();
        let snap = Snapshot {
            meta: SnapshotMeta { index: Index(4), term: Term(9), members: vec![NodeId(1)] },
            data: vec![1],
        };
        s.install_snapshot(snap).unwrap();
        drop(s);
        let r = open(&disk);
        assert_eq!(r.first_index(), Index(5));
        assert_eq!(r.last_index(), Index(4));
        assert_eq!(r.term_at(Index(4)), Some(Term(9)));
    }

    #[test]
    fn a_node_that_crashes_before_applying_a_received_snapshot_is_handed_it_again_after_restart() {
        let disk = SimDisk::default();
        let mut r = Raft::new(NodeId(1), ids(3), open(&disk), 1).unwrap();
        let meta = SnapshotMeta {
            index: Index(10),
            term: Term(2),
            members: vec![NodeId(1), NodeId(2), NodeId(3)],
        };
        r.step(Message {
            from: NodeId(2),
            to: NodeId(1),
            term: Term(3),
            body: Body::Snapshot { meta: meta.clone(), data: vec![9, 9] },
        });
        assert!(r.fault().is_none());
        assert_eq!(r.commit_index(), Index(10));
        disk.crash_drop();
        let mut again = Raft::new(NodeId(1), ids(3), open(&disk), 2).unwrap();
        assert_eq!(again.commit_index(), Index(10));
        assert_eq!(again.applied_index(), Index(0));
        let ready = again.ready();
        let snap = ready.snapshot.clone().expect("snapshot is redelivered");
        assert_eq!(snap.meta, meta);
        assert_eq!(snap.data, vec![9, 9]);
        again.advance(ready);
        assert_eq!(again.applied_index(), Index(10));
        let mut third = Raft::new(NodeId(1), ids(3), open(&disk), 3).unwrap();
        assert!(third.ready().snapshot.is_none());
        assert_eq!(third.applied_index(), Index(10));
    }

    #[test]
    fn a_restarted_leader_keeps_its_compaction_and_membership_and_replays_only_after_the_snapshot() {
        let disk = SimDisk::default();
        let mut r = Raft::new(NodeId(1), ids(1), open(&disk), 1).unwrap();
        while r.role() != Role::Leader {
            r.tick();
        }
        for i in 0..10u8 {
            r.propose(vec![i + 1]).unwrap();
        }
        let ready = r.ready();
        r.advance(ready);
        r.compact(Index(8), vec![4, 2]).unwrap();
        r.propose(vec![77]).unwrap();
        let ready = r.ready();
        r.advance(ready);
        disk.crash(&mut Rng::new(11));
        let mut again = Raft::new(NodeId(1), ids(1), open(&disk), 2).unwrap();
        assert_eq!(again.storage().first_index(), Index(9));
        assert_eq!(again.storage().snapshot_data().unwrap(), vec![4, 2]);
        assert_eq!(again.applied_index(), Index(12));
        let ready = again.ready();
        assert!(ready.snapshot.is_none());
        assert!(ready.committed_entries.is_empty());
        assert_eq!(again.storage().entries(Index(9), Index(13)).unwrap().len(), 4);
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
