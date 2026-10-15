#![deny(unsafe_code)]

use std::collections::BTreeMap;

use crate::raft::{Index, NodeId, Term};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RangeId(pub u64);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RangeError {
    #[error("unknown range {0:?}")]
    UnknownRange(RangeId),
    #[error("split key is not strictly inside the range")]
    InvalidSplitKey,
    #[error("ranges are not adjacent")]
    NotAdjacent,
    #[error("replica sets differ")]
    ReplicaMismatch,
    #[error("replica set must hold 1 to 3 nodes")]
    InvalidReplicas,
    #[error("invalid range map: {0}")]
    Invalid(&'static str),
    #[error("lease held by {holder:?}")]
    LeaseHeld { holder: NodeId },
    #[error("term is older than the observed term")]
    StaleTerm,
    #[error("node does not hold the lease")]
    NotHolder,
    #[error("lease expired")]
    LeaseExpired,
    #[error("no lease for range")]
    NoLease,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RangeDescriptor {
    pub id: RangeId,
    pub start_key: Vec<u8>,
    pub end_key: Vec<u8>,
    pub replicas: Vec<NodeId>,
    pub generation: u64,
}

impl RangeDescriptor {
    pub fn contains(&self, key: &[u8]) -> bool {
        key >= self.start_key.as_slice()
            && (self.end_key.is_empty() || key < self.end_key.as_slice())
    }
}

fn normalize(mut replicas: Vec<NodeId>) -> Vec<NodeId> {
    replicas.sort();
    replicas.dedup();
    replicas.truncate(3);
    replicas
}

#[derive(Debug, Clone)]
pub struct RangeMap {
    ranges: BTreeMap<Vec<u8>, RangeDescriptor>,
    starts: BTreeMap<RangeId, Vec<u8>>,
    sizes: BTreeMap<RangeId, u64>,
    floors: BTreeMap<RangeId, u64>,
    next_id: u64,
}

impl RangeMap {
    pub fn new_single(replicas: Vec<NodeId>) -> Self {
        let id = RangeId(1);
        let desc = RangeDescriptor {
            id,
            start_key: Vec::new(),
            end_key: Vec::new(),
            replicas: normalize(replicas),
            generation: 1,
        };
        let mut map = RangeMap {
            ranges: BTreeMap::new(),
            starts: BTreeMap::new(),
            sizes: BTreeMap::new(),
            floors: BTreeMap::new(),
            next_id: 2,
        };
        map.starts.insert(id, Vec::new());
        map.sizes.insert(id, 0);
        map.floors.insert(id, 1);
        map.ranges.insert(Vec::new(), desc);
        map
    }

    pub fn len(&self) -> usize {
        self.ranges.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    pub fn get(&self, id: RangeId) -> Option<&RangeDescriptor> {
        self.starts.get(&id).and_then(|s| self.ranges.get(s))
    }

    pub fn size_of(&self, id: RangeId) -> Option<u64> {
        self.sizes.get(&id).copied()
    }

    pub fn ranges(&self) -> impl Iterator<Item = &RangeDescriptor> + '_ {
        self.ranges.values()
    }

    pub fn lookup(&self, key: &[u8]) -> &RangeDescriptor {
        let found = self
            .ranges
            .range::<[u8], _>((
                std::ops::Bound::Unbounded,
                std::ops::Bound::Included(key),
            ))
            .next_back()
            .map(|(_, d)| d);
        match found {
            Some(d) => d,
            None => self
                .ranges
                .values()
                .next()
                .expect("range map is never empty"),
        }
    }

    pub fn split(
        &mut self,
        range_id: RangeId,
        at_key: &[u8],
    ) -> Result<(RangeId, RangeId), RangeError> {
        let old = self
            .get(range_id)
            .cloned()
            .ok_or(RangeError::UnknownRange(range_id))?;
        let inside = at_key > old.start_key.as_slice()
            && (old.end_key.is_empty() || at_key < old.end_key.as_slice());
        if !inside {
            return Err(RangeError::InvalidSplitKey);
        }
        let generation = old.generation + 1;
        let right_id = RangeId(self.next_id);
        self.next_id += 1;
        let size = self.sizes.get(&range_id).copied().unwrap_or(0);
        let left = RangeDescriptor {
            id: range_id,
            start_key: old.start_key.clone(),
            end_key: at_key.to_vec(),
            replicas: old.replicas.clone(),
            generation,
        };
        let right = RangeDescriptor {
            id: right_id,
            start_key: at_key.to_vec(),
            end_key: old.end_key,
            replicas: old.replicas,
            generation,
        };
        self.ranges.insert(left.start_key.clone(), left);
        self.starts.insert(right_id, right.start_key.clone());
        self.ranges.insert(right.start_key.clone(), right);
        self.sizes.insert(range_id, size / 2);
        self.sizes.insert(right_id, size - size / 2);
        self.floors.insert(range_id, generation);
        self.floors.insert(right_id, generation);
        Ok((range_id, right_id))
    }

    pub fn merge(&mut self, left: RangeId, right: RangeId) -> Result<(), RangeError> {
        let l = self
            .get(left)
            .cloned()
            .ok_or(RangeError::UnknownRange(left))?;
        let r = self
            .get(right)
            .cloned()
            .ok_or(RangeError::UnknownRange(right))?;
        if left == right || l.end_key.is_empty() || l.end_key != r.start_key {
            return Err(RangeError::NotAdjacent);
        }
        if l.replicas != r.replicas {
            return Err(RangeError::ReplicaMismatch);
        }
        let generation = l.generation.max(r.generation) + 1;
        let size = self.sizes.get(&left).copied().unwrap_or(0)
            + self.sizes.get(&right).copied().unwrap_or(0);
        self.ranges.remove(&r.start_key);
        self.starts.remove(&right);
        self.sizes.remove(&right);
        self.floors.remove(&right);
        let merged = RangeDescriptor {
            end_key: r.end_key,
            generation,
            ..l
        };
        self.ranges.insert(merged.start_key.clone(), merged);
        self.sizes.insert(left, size);
        self.floors.insert(left, generation);
        Ok(())
    }

    pub fn set_replicas(
        &mut self,
        range_id: RangeId,
        replicas: Vec<NodeId>,
    ) -> Result<u64, RangeError> {
        let replicas = normalize(replicas);
        if replicas.is_empty() {
            return Err(RangeError::InvalidReplicas);
        }
        let start = self
            .starts
            .get(&range_id)
            .cloned()
            .ok_or(RangeError::UnknownRange(range_id))?;
        let desc = self
            .ranges
            .get_mut(&start)
            .ok_or(RangeError::Invalid("start index points at a missing range"))?;
        desc.replicas = replicas;
        desc.generation += 1;
        let generation = desc.generation;
        self.floors.insert(range_id, generation);
        Ok(generation)
    }

    pub fn size_report(&mut self, range_id: RangeId, bytes: u64) -> Result<(), RangeError> {
        if !self.starts.contains_key(&range_id) {
            return Err(RangeError::UnknownRange(range_id));
        }
        self.sizes.insert(range_id, bytes);
        Ok(())
    }

    pub fn should_split(
        &self,
        range_id: RangeId,
        threshold: u64,
        key_sample: &[Vec<u8>],
    ) -> Option<Vec<u8>> {
        let desc = self.get(range_id)?;
        if self.sizes.get(&range_id).copied().unwrap_or(0) <= threshold {
            return None;
        }
        let mut inside: Vec<&Vec<u8>> = key_sample
            .iter()
            .filter(|k| {
                k.as_slice() > desc.start_key.as_slice()
                    && (desc.end_key.is_empty() || k.as_slice() < desc.end_key.as_slice())
            })
            .collect();
        inside.sort();
        inside.get(inside.len() / 2).map(|k| (*k).clone())
    }

    pub fn validate(&self) -> Result<(), RangeError> {
        if self.ranges.is_empty() {
            return Err(RangeError::Invalid("no ranges"));
        }
        if self.ranges.len() != self.starts.len() {
            return Err(RangeError::Invalid("start index size mismatch"));
        }
        let mut expected_start: Option<&[u8]> = Some(&[]);
        for (key, desc) in &self.ranges {
            match expected_start {
                Some(s) if s == desc.start_key.as_slice() => {}
                Some(_) => return Err(RangeError::Invalid("gap or overlap between ranges")),
                None => return Err(RangeError::Invalid("range after unbounded end")),
            }
            if key != &desc.start_key {
                return Err(RangeError::Invalid("map key differs from start key"));
            }
            if !desc.end_key.is_empty() && desc.end_key <= desc.start_key {
                return Err(RangeError::Invalid("empty or inverted range"));
            }
            if desc.replicas.is_empty() || desc.replicas.len() > 3 {
                return Err(RangeError::Invalid("replica count outside 1..=3"));
            }
            if desc.replicas.windows(2).any(|w| w[0] >= w[1]) {
                return Err(RangeError::Invalid("replicas not sorted and unique"));
            }
            if self.starts.get(&desc.id) != Some(key) {
                return Err(RangeError::Invalid("id index mismatch"));
            }
            match self.floors.get(&desc.id) {
                Some(floor) if desc.generation >= *floor => {}
                _ => return Err(RangeError::Invalid("generation decreased")),
            }
            expected_start = if desc.end_key.is_empty() {
                None
            } else {
                Some(desc.end_key.as_slice())
            };
        }
        if expected_start.is_some() {
            return Err(RangeError::Invalid("last range is bounded"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lease {
    pub holder: NodeId,
    pub epoch: u64,
    pub start: u64,
    pub duration: u64,
    pub term: Term,
}

impl Lease {
    pub fn expiry(&self) -> u64 {
        self.start.saturating_add(self.duration)
    }

    pub fn unexpired_at(&self, now: u64) -> bool {
        now < self.expiry()
    }
}

#[derive(Debug, Clone)]
pub struct LeaseTable {
    duration: u64,
    leases: BTreeMap<RangeId, Lease>,
    epochs: BTreeMap<RangeId, u64>,
    terms: BTreeMap<RangeId, Term>,
}

impl LeaseTable {
    pub fn new(duration: u64) -> Self {
        LeaseTable {
            duration,
            leases: BTreeMap::new(),
            epochs: BTreeMap::new(),
            terms: BTreeMap::new(),
        }
    }

    pub fn duration(&self) -> u64 {
        self.duration
    }

    pub fn observed_term(&self, range: RangeId) -> Term {
        self.terms.get(&range).copied().unwrap_or_default()
    }

    pub fn last_epoch(&self, range: RangeId) -> u64 {
        self.epochs.get(&range).copied().unwrap_or(0)
    }

    pub fn observe_term(&mut self, range: RangeId, term: Term) {
        let entry = self.terms.entry(range).or_default();
        if term > *entry {
            *entry = term;
        }
    }

    pub fn lease_at(&self, range: RangeId, now: u64) -> Option<&Lease> {
        let lease = self.leases.get(&range)?;
        if lease.unexpired_at(now) && lease.term >= self.observed_term(range) {
            Some(lease)
        } else {
            None
        }
    }

    pub fn holder_at(&self, range: RangeId, now: u64) -> Option<NodeId> {
        self.lease_at(range, now).map(|l| l.holder)
    }

    pub fn acquire(
        &mut self,
        range: RangeId,
        node: NodeId,
        now: u64,
        term: Term,
    ) -> Result<Lease, RangeError> {
        if term < self.observed_term(range) {
            return Err(RangeError::StaleTerm);
        }
        if let Some(existing) = self.leases.get(&range) {
            if existing.holder != node && existing.unexpired_at(now) && existing.term >= term {
                return Err(RangeError::LeaseHeld {
                    holder: existing.holder,
                });
            }
        }
        let epoch = self.last_epoch(range) + 1;
        let lease = Lease {
            holder: node,
            epoch,
            start: now,
            duration: self.duration,
            term,
        };
        self.epochs.insert(range, epoch);
        self.observe_term(range, term);
        self.leases.insert(range, lease);
        Ok(lease)
    }

    pub fn renew(&mut self, range: RangeId, node: NodeId, now: u64) -> Result<Lease, RangeError> {
        let observed = self.observed_term(range);
        let duration = self.duration;
        let lease = self.leases.get_mut(&range).ok_or(RangeError::NoLease)?;
        if lease.holder != node {
            return Err(RangeError::NotHolder);
        }
        if lease.term < observed {
            return Err(RangeError::StaleTerm);
        }
        if !lease.unexpired_at(now) {
            return Err(RangeError::LeaseExpired);
        }
        lease.start = now.max(lease.start);
        lease.duration = duration;
        Ok(*lease)
    }

    pub fn release(&mut self, range: RangeId, node: NodeId) -> Result<(), RangeError> {
        match self.leases.get(&range) {
            None => Err(RangeError::NoLease),
            Some(l) if l.holder != node => Err(RangeError::NotHolder),
            Some(_) => {
                self.leases.remove(&range);
                Ok(())
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DecodeError {
    #[error("input truncated")]
    Truncated,
    #[error("unknown message tag {0}")]
    BadTag(u8),
    #[error("length prefix disagrees with input size")]
    BadLength,
    #[error("replica set malformed")]
    BadReplicas,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RangeMsg {
    Descriptor(RangeDescriptor),
    Split { range: RangeId, at_key: Vec<u8> },
    Merge { left: RangeId, right: RangeId },
    SizeReport { range: RangeId, bytes: u64 },
    LeaseRequest { range: RangeId, node: NodeId, term: Term },
    LeaseGrant { range: RangeId, lease: Lease },
    LeaseRelease { range: RangeId, node: NodeId },
    TermObserved { range: RangeId, term: Term },
    Applied { range: RangeId, index: Index },
}

fn put_u64(out: &mut Vec<u8>, v: u64) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_bytes(out: &mut Vec<u8>, b: &[u8]) {
    let len = u32::try_from(b.len()).unwrap_or(u32::MAX);
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(b);
}

fn put_descriptor(out: &mut Vec<u8>, d: &RangeDescriptor) {
    put_u64(out, d.id.0);
    put_bytes(out, &d.start_key);
    put_bytes(out, &d.end_key);
    out.push(u8::try_from(d.replicas.len()).unwrap_or(u8::MAX));
    for r in &d.replicas {
        put_u64(out, r.0);
    }
    put_u64(out, d.generation);
}

fn put_lease(out: &mut Vec<u8>, l: &Lease) {
    put_u64(out, l.holder.0);
    put_u64(out, l.epoch);
    put_u64(out, l.start);
    put_u64(out, l.duration);
    put_u64(out, l.term.0);
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        let end = self.pos.checked_add(n).ok_or(DecodeError::Truncated)?;
        let slice = self.buf.get(self.pos..end).ok_or(DecodeError::Truncated)?;
        self.pos = end;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32, DecodeError> {
        let mut a = [0u8; 4];
        a.copy_from_slice(self.take(4)?);
        Ok(u32::from_le_bytes(a))
    }

    fn u64(&mut self) -> Result<u64, DecodeError> {
        let mut a = [0u8; 8];
        a.copy_from_slice(self.take(8)?);
        Ok(u64::from_le_bytes(a))
    }

    fn bytes(&mut self) -> Result<Vec<u8>, DecodeError> {
        let len = usize::try_from(self.u32()?).map_err(|_| DecodeError::Truncated)?;
        Ok(self.take(len)?.to_vec())
    }

    fn descriptor(&mut self) -> Result<RangeDescriptor, DecodeError> {
        let id = RangeId(self.u64()?);
        let start_key = self.bytes()?;
        let end_key = self.bytes()?;
        let count = usize::from(self.u8()?);
        if count == 0 || count > 3 {
            return Err(DecodeError::BadReplicas);
        }
        let mut replicas = Vec::with_capacity(count);
        for _ in 0..count {
            replicas.push(NodeId(self.u64()?));
        }
        if replicas.windows(2).any(|w| w[0] >= w[1]) {
            return Err(DecodeError::BadReplicas);
        }
        let generation = self.u64()?;
        Ok(RangeDescriptor {
            id,
            start_key,
            end_key,
            replicas,
            generation,
        })
    }

    fn lease(&mut self) -> Result<Lease, DecodeError> {
        Ok(Lease {
            holder: NodeId(self.u64()?),
            epoch: self.u64()?,
            start: self.u64()?,
            duration: self.u64()?,
            term: Term(self.u64()?),
        })
    }
}

impl RangeMsg {
    pub fn encode(&self) -> Vec<u8> {
        let mut body = Vec::new();
        match self {
            RangeMsg::Descriptor(d) => {
                body.push(0);
                put_descriptor(&mut body, d);
            }
            RangeMsg::Split { range, at_key } => {
                body.push(1);
                put_u64(&mut body, range.0);
                put_bytes(&mut body, at_key);
            }
            RangeMsg::Merge { left, right } => {
                body.push(2);
                put_u64(&mut body, left.0);
                put_u64(&mut body, right.0);
            }
            RangeMsg::SizeReport { range, bytes } => {
                body.push(3);
                put_u64(&mut body, range.0);
                put_u64(&mut body, *bytes);
            }
            RangeMsg::LeaseRequest { range, node, term } => {
                body.push(4);
                put_u64(&mut body, range.0);
                put_u64(&mut body, node.0);
                put_u64(&mut body, term.0);
            }
            RangeMsg::LeaseGrant { range, lease } => {
                body.push(5);
                put_u64(&mut body, range.0);
                put_lease(&mut body, lease);
            }
            RangeMsg::LeaseRelease { range, node } => {
                body.push(6);
                put_u64(&mut body, range.0);
                put_u64(&mut body, node.0);
            }
            RangeMsg::TermObserved { range, term } => {
                body.push(7);
                put_u64(&mut body, range.0);
                put_u64(&mut body, term.0);
            }
            RangeMsg::Applied { range, index } => {
                body.push(8);
                put_u64(&mut body, range.0);
                put_u64(&mut body, index.0);
            }
        }
        let mut out = Vec::with_capacity(body.len() + 4);
        let len = u32::try_from(body.len()).unwrap_or(u32::MAX);
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&body);
        out
    }

    pub fn decode(buf: &[u8]) -> Result<RangeMsg, DecodeError> {
        let mut outer = Reader { buf, pos: 0 };
        let declared = usize::try_from(outer.u32()?).map_err(|_| DecodeError::BadLength)?;
        if buf.len() - 4 != declared {
            return Err(DecodeError::BadLength);
        }
        let mut r = Reader { buf, pos: 4 };
        let msg = match r.u8()? {
            0 => RangeMsg::Descriptor(r.descriptor()?),
            1 => RangeMsg::Split {
                range: RangeId(r.u64()?),
                at_key: r.bytes()?,
            },
            2 => RangeMsg::Merge {
                left: RangeId(r.u64()?),
                right: RangeId(r.u64()?),
            },
            3 => RangeMsg::SizeReport {
                range: RangeId(r.u64()?),
                bytes: r.u64()?,
            },
            4 => RangeMsg::LeaseRequest {
                range: RangeId(r.u64()?),
                node: NodeId(r.u64()?),
                term: Term(r.u64()?),
            },
            5 => RangeMsg::LeaseGrant {
                range: RangeId(r.u64()?),
                lease: r.lease()?,
            },
            6 => RangeMsg::LeaseRelease {
                range: RangeId(r.u64()?),
                node: NodeId(r.u64()?),
            },
            7 => RangeMsg::TermObserved {
                range: RangeId(r.u64()?),
                term: Term(r.u64()?),
            },
            8 => RangeMsg::Applied {
                range: RangeId(r.u64()?),
                index: Index(r.u64()?),
            },
            tag => return Err(DecodeError::BadTag(tag)),
        };
        if r.pos != buf.len() {
            return Err(DecodeError::BadLength);
        }
        Ok(msg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::Rng;

    fn trio() -> Vec<NodeId> {
        vec![NodeId(1), NodeId(2), NodeId(3)]
    }

    fn rand_key(rng: &mut Rng) -> Vec<u8> {
        let len = rng.between(0, 3) as usize;
        rng.bytes(len)
    }

    #[derive(Clone)]
    struct ModelRange {
        id: RangeId,
        start: Vec<u8>,
        end: Vec<u8>,
    }

    fn model_lookup<'a>(model: &'a [ModelRange], key: &[u8]) -> &'a ModelRange {
        let i = model.partition_point(|m| m.start.as_slice() <= key);
        &model[i - 1]
    }

    fn three_way_map() -> (RangeMap, RangeId, RangeId, RangeId) {
        let mut map = RangeMap::new_single(trio());
        let (a, b) = map.split(RangeId(1), b"g").unwrap();
        let (b, c) = map.split(b, b"p").unwrap();
        (map, a, b, c)
    }

    #[test]
    fn lookup_matches_sorted_vector_model_after_random_splits_and_merges() {
        let mut rng = Rng::new(0xA11CE);
        let mut map = RangeMap::new_single(trio());
        let mut model = vec![ModelRange {
            id: RangeId(1),
            start: Vec::new(),
            end: Vec::new(),
        }];
        let mut seen: BTreeMap<RangeId, u64> = BTreeMap::new();
        let mut splits = 0;
        let mut merges = 0;
        for _ in 0..50 {
            if model.len() < 2 || rng.chance(2, 3) {
                let i = rng.below(model.len() as u64) as usize;
                let key = rand_key(&mut rng);
                let m = &model[i];
                let valid = key.as_slice() > m.start.as_slice()
                    && (m.end.is_empty() || key.as_slice() < m.end.as_slice());
                let res = map.split(m.id, &key);
                if valid {
                    let (l, r) = res.unwrap();
                    assert_eq!(l, model[i].id);
                    let old_end = model[i].end.clone();
                    model[i].end = key.clone();
                    model.insert(
                        i + 1,
                        ModelRange {
                            id: r,
                            start: key,
                            end: old_end,
                        },
                    );
                    splits += 1;
                } else {
                    assert_eq!(res, Err(RangeError::InvalidSplitKey));
                }
            } else {
                let a = rng.below(model.len() as u64) as usize;
                let b = if a + 1 < model.len() && rng.chance(3, 4) {
                    a + 1
                } else {
                    rng.below(model.len() as u64) as usize
                };
                let res = map.merge(model[a].id, model[b].id);
                if b == a + 1 {
                    res.unwrap();
                    model[a].end = model[b].end.clone();
                    model.remove(b);
                    merges += 1;
                } else {
                    assert_eq!(res, Err(RangeError::NotAdjacent));
                }
            }
            map.validate().unwrap();
            assert_eq!(map.len(), model.len());
            for d in map.ranges() {
                let prev = seen.get(&d.id).copied().unwrap_or(0);
                assert!(d.generation >= prev);
                seen.insert(d.id, d.generation);
            }
        }
        assert!(splits >= 5);
        assert!(merges >= 1);
        for _ in 0..1000 {
            let key = rand_key(&mut rng);
            let got = map.lookup(&key);
            let want = model_lookup(&model, &key);
            assert_eq!(got.id, want.id);
            assert_eq!(got.start_key, want.start);
            assert_eq!(got.end_key, want.end);
            assert!(got.contains(&key));
        }
        let listed: Vec<RangeId> = map.ranges().map(|d| d.id).collect();
        let expected: Vec<RangeId> = model.iter().map(|m| m.id).collect();
        assert_eq!(listed, expected);
    }

    #[test]
    fn single_range_covers_every_key_and_validates() {
        let map = RangeMap::new_single(vec![NodeId(3), NodeId(1)]);
        map.validate().unwrap();
        assert_eq!(map.lookup(b"").id, RangeId(1));
        assert_eq!(map.lookup(&[0xff; 9]).id, RangeId(1));
        assert_eq!(map.lookup(b"x").replicas, vec![NodeId(1), NodeId(3)]);
    }

    #[test]
    fn split_bumps_both_generations_and_gives_right_half_a_fresh_id() {
        let mut map = RangeMap::new_single(trio());
        let before = map.get(RangeId(1)).unwrap().generation;
        let (l, r) = map.split(RangeId(1), b"m").unwrap();
        assert_eq!(l, RangeId(1));
        assert_ne!(r, RangeId(1));
        assert_eq!(map.get(l).unwrap().generation, before + 1);
        assert_eq!(map.get(r).unwrap().generation, before + 1);
        assert_eq!(map.get(l).unwrap().end_key, b"m".to_vec());
        assert_eq!(map.get(r).unwrap().start_key, b"m".to_vec());
        assert_eq!(map.lookup(b"l").id, l);
        assert_eq!(map.lookup(b"m").id, r);
        map.validate().unwrap();
    }

    #[test]
    fn split_at_invalid_key_errors_and_leaves_map_untouched() {
        let (mut map, a, b, c) = three_way_map();
        let snapshot: Vec<RangeDescriptor> = map.ranges().cloned().collect();
        assert_eq!(map.split(a, b""), Err(RangeError::InvalidSplitKey));
        assert_eq!(map.split(b, b"g"), Err(RangeError::InvalidSplitKey));
        assert_eq!(map.split(b, b"p"), Err(RangeError::InvalidSplitKey));
        assert_eq!(map.split(b, b"z"), Err(RangeError::InvalidSplitKey));
        assert_eq!(map.split(b, b"a"), Err(RangeError::InvalidSplitKey));
        assert_eq!(map.split(c, b"p"), Err(RangeError::InvalidSplitKey));
        assert_eq!(
            map.split(RangeId(99), b"q"),
            Err(RangeError::UnknownRange(RangeId(99)))
        );
        let after: Vec<RangeDescriptor> = map.ranges().cloned().collect();
        assert_eq!(snapshot, after);
        map.validate().unwrap();
    }

    #[test]
    fn merge_of_non_adjacent_ranges_errors() {
        let (mut map, a, b, c) = three_way_map();
        assert_eq!(map.merge(a, c), Err(RangeError::NotAdjacent));
        assert_eq!(map.merge(b, a), Err(RangeError::NotAdjacent));
        assert_eq!(map.merge(c, b), Err(RangeError::NotAdjacent));
        assert_eq!(map.merge(b, b), Err(RangeError::NotAdjacent));
        assert_eq!(map.merge(c, a), Err(RangeError::NotAdjacent));
        assert_eq!(
            map.merge(a, RangeId(77)),
            Err(RangeError::UnknownRange(RangeId(77)))
        );
        assert_eq!(map.len(), 3);
        map.validate().unwrap();
    }

    #[test]
    fn merge_of_adjacent_ranges_bumps_generation_and_restores_bounds() {
        let (mut map, a, b, c) = three_way_map();
        let top = map
            .get(a)
            .unwrap()
            .generation
            .max(map.get(b).unwrap().generation);
        map.merge(a, b).unwrap();
        let merged = map.get(a).unwrap();
        assert_eq!(merged.generation, top + 1);
        assert_eq!(merged.end_key, b"p".to_vec());
        assert!(map.get(b).is_none());
        map.merge(a, c).unwrap();
        let whole = map.get(a).unwrap();
        assert!(whole.start_key.is_empty() && whole.end_key.is_empty());
        assert_eq!(map.len(), 1);
        map.validate().unwrap();
    }

    #[test]
    fn merge_with_different_replica_sets_errors() {
        let (mut map, a, b, _) = three_way_map();
        let generation = map.get(b).unwrap().generation;
        assert_eq!(
            map.set_replicas(b, vec![NodeId(2), NodeId(1)]).unwrap(),
            generation + 1
        );
        assert_eq!(map.merge(a, b), Err(RangeError::ReplicaMismatch));
        map.set_replicas(b, trio()).unwrap();
        map.merge(a, b).unwrap();
        map.validate().unwrap();
    }

    #[test]
    fn set_replicas_rejects_empty_and_normalizes_input() {
        let mut map = RangeMap::new_single(trio());
        assert_eq!(
            map.set_replicas(RangeId(1), Vec::new()),
            Err(RangeError::InvalidReplicas)
        );
        map.set_replicas(RangeId(1), vec![NodeId(5), NodeId(4), NodeId(5)])
            .unwrap();
        assert_eq!(map.lookup(b"k").replicas, vec![NodeId(4), NodeId(5)]);
        map.validate().unwrap();
    }

    #[test]
    fn validate_rejects_replica_sets_outside_one_to_three() {
        let empty = RangeMap::new_single(Vec::new());
        assert!(empty.validate().is_err());
        let capped = RangeMap::new_single((1..=9).map(NodeId).collect());
        assert_eq!(capped.lookup(b"").replicas.len(), 3);
        capped.validate().unwrap();
    }

    #[test]
    fn validate_detects_gap_overlap_and_generation_regression() {
        let (map, _, _, _) = three_way_map();
        let mut gap = map.clone();
        gap.ranges.get_mut(b"p".as_slice()).unwrap().start_key = b"q".to_vec();
        assert!(gap.validate().is_err());
        let mut overlap = map.clone();
        overlap.ranges.get_mut(&Vec::new()).unwrap().end_key = b"h".to_vec();
        assert!(overlap.validate().is_err());
        let mut regress = map.clone();
        regress.ranges.get_mut(&Vec::new()).unwrap().generation = 0;
        assert!(regress.validate().is_err());
        let mut unbounded_middle = map.clone();
        unbounded_middle.ranges.get_mut(b"g".as_slice()).unwrap().end_key = Vec::new();
        assert!(unbounded_middle.validate().is_err());
        map.validate().unwrap();
    }

    #[test]
    fn size_report_drives_should_split_to_the_median_sample_key() {
        let mut map = RangeMap::new_single(trio());
        let sample: Vec<Vec<u8>> = [b"e", b"a", b"c", b"d", b"b"]
            .iter()
            .map(|k| k.to_vec())
            .collect();
        assert_eq!(map.should_split(RangeId(1), 100, &sample), None);
        map.size_report(RangeId(1), 100).unwrap();
        assert_eq!(map.should_split(RangeId(1), 100, &sample), None);
        map.size_report(RangeId(1), 101).unwrap();
        assert_eq!(map.should_split(RangeId(1), 100, &sample), Some(b"c".to_vec()));
        assert_eq!(map.should_split(RangeId(1), 100, &[]), None);
        assert_eq!(map.should_split(RangeId(42), 0, &sample), None);
        assert_eq!(
            map.size_report(RangeId(42), 1),
            Err(RangeError::UnknownRange(RangeId(42)))
        );
    }

    #[test]
    fn should_split_ignores_sample_keys_outside_the_range() {
        let mut map = RangeMap::new_single(trio());
        let (_, right) = map.split(RangeId(1), b"m").unwrap();
        map.size_report(right, 500).unwrap();
        let sample: Vec<Vec<u8>> = [b"a", b"b", b"m", b"n", b"p", b"q", b"z"]
            .iter()
            .map(|k| k.to_vec())
            .collect();
        let key = map.should_split(right, 10, &sample).unwrap();
        assert_eq!(key, b"q".to_vec());
        assert!(map.split(right, &key).is_ok());
        map.validate().unwrap();
    }

    #[test]
    fn split_halves_the_reported_size_and_merge_sums_it() {
        let mut map = RangeMap::new_single(trio());
        map.size_report(RangeId(1), 101).unwrap();
        let (l, r) = map.split(RangeId(1), b"k").unwrap();
        assert_eq!(map.size_of(l).unwrap() + map.size_of(r).unwrap(), 101);
        map.merge(l, r).unwrap();
        assert_eq!(map.size_of(l), Some(101));
        assert_eq!(map.size_of(r), None);
    }

    const DUR: u64 = 10;
    const R1: RangeId = RangeId(1);

    #[derive(Clone, Copy)]
    struct ModelLease {
        node: u64,
        epoch: u64,
        start: u64,
        term: u64,
    }

    #[derive(Default)]
    struct LeaseModel {
        lease: Option<ModelLease>,
        epoch: u64,
        term: u64,
    }

    impl LeaseModel {
        fn acquire(&mut self, node: u64, now: u64, term: u64) -> Result<u64, RangeError> {
            if term < self.term {
                return Err(RangeError::StaleTerm);
            }
            if let Some(l) = self.lease {
                if l.node != node && now < l.start + DUR && l.term >= term {
                    return Err(RangeError::LeaseHeld {
                        holder: NodeId(l.node),
                    });
                }
            }
            self.epoch += 1;
            self.term = term;
            self.lease = Some(ModelLease {
                node,
                epoch: self.epoch,
                start: now,
                term,
            });
            Ok(self.epoch)
        }

        fn renew(&mut self, node: u64, now: u64) -> Result<u64, RangeError> {
            let term = self.term;
            let l = self.lease.as_mut().ok_or(RangeError::NoLease)?;
            if l.node != node {
                return Err(RangeError::NotHolder);
            }
            if l.term < term {
                return Err(RangeError::StaleTerm);
            }
            if now >= l.start + DUR {
                return Err(RangeError::LeaseExpired);
            }
            l.start = l.start.max(now);
            Ok(l.epoch)
        }

        fn release(&mut self, node: u64) -> Result<(), RangeError> {
            match self.lease {
                None => Err(RangeError::NoLease),
                Some(l) if l.node != node => Err(RangeError::NotHolder),
                Some(_) => {
                    self.lease = None;
                    Ok(())
                }
            }
        }

        fn observe(&mut self, term: u64) {
            self.term = self.term.max(term);
        }

        fn holder_at(&self, now: u64) -> Option<u64> {
            self.lease
                .filter(|l| now < l.start + DUR && l.term >= self.term)
                .map(|l| l.node)
        }
    }

    #[test]
    fn lease_exclusivity_matches_model_over_500_random_sequences() {
        for seq in 0..500u64 {
            let mut rng = Rng::new(0x1EA5E ^ seq);
            let mut table = LeaseTable::new(DUR);
            let mut model = LeaseModel::default();
            let mut now = 0u64;
            let mut last_epoch = 0u64;
            for _ in 0..40 {
                now += rng.below(8);
                let node = rng.between(1, 3);
                let term = rng.below(5);
                match rng.below(6) {
                    0 | 1 => {
                        let before = table.lease_at(R1, now).copied();
                        let got = table.acquire(R1, NodeId(node), now, Term(term));
                        let want = model.acquire(node, now, term);
                        match (&got, &want) {
                            (Ok(lease), Ok(epoch)) => {
                                assert_eq!(lease.epoch, *epoch);
                                assert!(lease.epoch > last_epoch);
                                last_epoch = lease.epoch;
                                if let Some(prev) = before {
                                    assert!(prev.holder == NodeId(node) || prev.term < Term(term));
                                }
                                assert_eq!(table.holder_at(R1, now), Some(NodeId(node)));
                            }
                            (Err(a), Err(b)) => assert_eq!(a, b),
                            _ => panic!("acquire diverged from model"),
                        }
                    }
                    2 | 3 => {
                        let got = table.renew(R1, NodeId(node), now).map(|l| l.epoch);
                        assert_eq!(got, model.renew(node, now));
                    }
                    4 => {
                        assert_eq!(table.release(R1, NodeId(node)), model.release(node));
                    }
                    _ => {
                        table.observe_term(R1, Term(term));
                        model.observe(term);
                    }
                }
                for probe in [now, now + 1, now + DUR - 1, now + DUR, now + 3 * DUR] {
                    assert_eq!(
                        table.holder_at(R1, probe).map(|n| n.0),
                        model.holder_at(probe)
                    );
                }
                assert_eq!(table.observed_term(R1), Term(model.term));
            }
        }
    }

    #[test]
    fn epoch_strictly_increases_across_release_and_holder_changes() {
        let mut t = LeaseTable::new(DUR);
        let mut last = 0;
        for i in 0..30u64 {
            let node = NodeId(1 + i % 3);
            let lease = t.acquire(R1, node, i * 100, Term(1)).unwrap();
            assert!(lease.epoch > last);
            last = lease.epoch;
            if i % 2 == 0 {
                t.release(R1, node).unwrap();
            }
            assert_eq!(t.last_epoch(R1), last);
        }
    }

    #[test]
    fn same_holder_reacquire_still_advances_the_epoch() {
        let mut t = LeaseTable::new(DUR);
        let first = t.acquire(R1, NodeId(1), 0, Term(1)).unwrap();
        let second = t.acquire(R1, NodeId(1), 1, Term(1)).unwrap();
        assert!(second.epoch > first.epoch);
    }

    #[test]
    fn higher_term_invalidates_lease_before_expiry() {
        let mut t = LeaseTable::new(DUR);
        t.acquire(R1, NodeId(1), 0, Term(1)).unwrap();
        assert_eq!(t.holder_at(R1, 5), Some(NodeId(1)));
        t.observe_term(R1, Term(2));
        assert_eq!(t.holder_at(R1, 5), None);
        assert_eq!(t.renew(R1, NodeId(1), 5), Err(RangeError::StaleTerm));
        let next = t.acquire(R1, NodeId(2), 5, Term(2)).unwrap();
        assert_eq!(t.holder_at(R1, 5), Some(NodeId(2)));
        assert_eq!(next.term, Term(2));
    }

    #[test]
    fn acquiring_in_a_higher_term_supersedes_an_unexpired_foreign_lease() {
        let mut t = LeaseTable::new(DUR);
        t.acquire(R1, NodeId(1), 0, Term(1)).unwrap();
        assert_eq!(
            t.acquire(R1, NodeId(2), 3, Term(1)),
            Err(RangeError::LeaseHeld { holder: NodeId(1) })
        );
        t.acquire(R1, NodeId(2), 3, Term(2)).unwrap();
        assert_eq!(t.holder_at(R1, 3), Some(NodeId(2)));
        assert_eq!(t.renew(R1, NodeId(1), 4), Err(RangeError::NotHolder));
    }

    #[test]
    fn acquire_with_a_term_older_than_observed_is_rejected() {
        let mut t = LeaseTable::new(DUR);
        t.observe_term(R1, Term(7));
        assert_eq!(
            t.acquire(R1, NodeId(1), 0, Term(6)),
            Err(RangeError::StaleTerm)
        );
        assert!(t.acquire(R1, NodeId(1), 0, Term(7)).is_ok());
        t.observe_term(R1, Term(3));
        assert_eq!(t.observed_term(R1), Term(7));
    }

    #[test]
    fn expired_lease_cannot_be_renewed_and_can_be_replaced() {
        let mut t = LeaseTable::new(DUR);
        let first = t.acquire(R1, NodeId(1), 0, Term(1)).unwrap();
        assert_eq!(first.expiry(), DUR);
        assert_eq!(t.holder_at(R1, DUR - 1), Some(NodeId(1)));
        assert_eq!(t.holder_at(R1, DUR), None);
        assert_eq!(t.renew(R1, NodeId(1), DUR), Err(RangeError::LeaseExpired));
        let second = t.acquire(R1, NodeId(2), DUR, Term(1)).unwrap();
        assert!(second.epoch > first.epoch);
        assert_eq!(t.renew(R1, NodeId(1), DUR + 1), Err(RangeError::NotHolder));
    }

    #[test]
    fn renew_extends_expiry_without_changing_epoch() {
        let mut t = LeaseTable::new(DUR);
        let first = t.acquire(R1, NodeId(1), 0, Term(1)).unwrap();
        let renewed = t.renew(R1, NodeId(1), 7).unwrap();
        assert_eq!(renewed.epoch, first.epoch);
        assert_eq!(renewed.expiry(), 7 + DUR);
        assert_eq!(t.holder_at(R1, 15), Some(NodeId(1)));
        assert_eq!(t.holder_at(R1, 17), None);
    }

    #[test]
    fn release_requires_the_holder_and_frees_the_range() {
        let mut t = LeaseTable::new(DUR);
        assert_eq!(t.release(R1, NodeId(1)), Err(RangeError::NoLease));
        assert_eq!(t.renew(R1, NodeId(1), 0), Err(RangeError::NoLease));
        t.acquire(R1, NodeId(1), 0, Term(1)).unwrap();
        assert_eq!(t.release(R1, NodeId(2)), Err(RangeError::NotHolder));
        t.release(R1, NodeId(1)).unwrap();
        assert_eq!(t.holder_at(R1, 1), None);
        assert!(t.acquire(R1, NodeId(2), 1, Term(1)).is_ok());
    }

    #[test]
    fn leases_on_different_ranges_do_not_interfere() {
        let mut t = LeaseTable::new(DUR);
        t.acquire(RangeId(1), NodeId(1), 0, Term(1)).unwrap();
        t.acquire(RangeId(2), NodeId(2), 0, Term(1)).unwrap();
        t.observe_term(RangeId(1), Term(9));
        assert_eq!(t.holder_at(RangeId(1), 1), None);
        assert_eq!(t.holder_at(RangeId(2), 1), Some(NodeId(2)));
    }

    fn frame(body: &[u8]) -> Vec<u8> {
        let mut out = (body.len() as u32).to_le_bytes().to_vec();
        out.extend_from_slice(body);
        out
    }

    fn sample_messages() -> Vec<RangeMsg> {
        let lease = Lease {
            holder: NodeId(2),
            epoch: 9,
            start: 100,
            duration: 10,
            term: Term(4),
        };
        vec![
            RangeMsg::Descriptor(RangeDescriptor {
                id: RangeId(5),
                start_key: b"abc".to_vec(),
                end_key: Vec::new(),
                replicas: trio(),
                generation: 12,
            }),
            RangeMsg::Descriptor(RangeDescriptor {
                id: RangeId(6),
                start_key: Vec::new(),
                end_key: vec![0xff, 0x00],
                replicas: vec![NodeId(9)],
                generation: u64::MAX,
            }),
            RangeMsg::Split {
                range: RangeId(1),
                at_key: b"split-here".to_vec(),
            },
            RangeMsg::Split {
                range: RangeId(1),
                at_key: Vec::new(),
            },
            RangeMsg::Merge {
                left: RangeId(1),
                right: RangeId(2),
            },
            RangeMsg::SizeReport {
                range: RangeId(3),
                bytes: 1 << 40,
            },
            RangeMsg::LeaseRequest {
                range: RangeId(3),
                node: NodeId(1),
                term: Term(2),
            },
            RangeMsg::LeaseGrant {
                range: RangeId(3),
                lease,
            },
            RangeMsg::LeaseRelease {
                range: RangeId(3),
                node: NodeId(1),
            },
            RangeMsg::TermObserved {
                range: RangeId(3),
                term: Term(8),
            },
            RangeMsg::Applied {
                range: RangeId(3),
                index: Index(77),
            },
        ]
    }

    #[test]
    fn every_message_variant_round_trips() {
        for m in sample_messages() {
            let bytes = m.encode();
            assert_eq!(RangeMsg::decode(&bytes), Ok(m));
        }
    }

    #[test]
    fn every_strict_truncation_of_a_valid_encoding_is_an_error() {
        for m in sample_messages() {
            let bytes = m.encode();
            for cut in 0..bytes.len() {
                assert!(RangeMsg::decode(&bytes[..cut]).is_err());
            }
            let mut longer = bytes.clone();
            longer.push(0);
            assert!(RangeMsg::decode(&longer).is_err());
        }
    }

    #[test]
    fn body_shorter_than_its_fields_is_truncated_even_with_a_matching_prefix() {
        for m in sample_messages() {
            let bytes = m.encode();
            let body = &bytes[4..];
            for cut in 0..body.len() {
                assert!(RangeMsg::decode(&frame(&body[..cut])).is_err());
            }
            let mut padded = body.to_vec();
            padded.push(7);
            assert_eq!(
                RangeMsg::decode(&frame(&padded)),
                Err(DecodeError::BadLength)
            );
        }
    }

    #[test]
    fn unknown_tag_is_rejected() {
        for tag in 9..=255u8 {
            assert_eq!(
                RangeMsg::decode(&frame(&[tag])),
                Err(DecodeError::BadTag(tag))
            );
        }
    }

    #[test]
    fn oversized_byte_length_claim_is_truncated_not_allocated() {
        let mut body = vec![1u8];
        body.extend_from_slice(&1u64.to_le_bytes());
        body.extend_from_slice(&u32::MAX.to_le_bytes());
        body.extend_from_slice(b"xy");
        assert_eq!(
            RangeMsg::decode(&frame(&body)),
            Err(DecodeError::Truncated)
        );
    }

    #[test]
    fn declared_length_must_match_the_input() {
        let mut bytes = RangeMsg::Merge {
            left: RangeId(1),
            right: RangeId(2),
        }
        .encode();
        bytes[0] = bytes[0].wrapping_add(1);
        assert_eq!(RangeMsg::decode(&bytes), Err(DecodeError::BadLength));
        bytes[0] = bytes[0].wrapping_sub(2);
        assert_eq!(RangeMsg::decode(&bytes), Err(DecodeError::BadLength));
    }

    #[test]
    fn descriptor_with_bad_replica_sets_is_rejected() {
        let build = |replicas: &[u64]| {
            let mut body = vec![0u8];
            body.extend_from_slice(&1u64.to_le_bytes());
            body.extend_from_slice(&0u32.to_le_bytes());
            body.extend_from_slice(&0u32.to_le_bytes());
            body.push(replicas.len() as u8);
            for r in replicas {
                body.extend_from_slice(&r.to_le_bytes());
            }
            body.extend_from_slice(&1u64.to_le_bytes());
            frame(&body)
        };
        assert!(RangeMsg::decode(&build(&[1, 2])).is_ok());
        assert_eq!(
            RangeMsg::decode(&build(&[])),
            Err(DecodeError::BadReplicas)
        );
        assert_eq!(
            RangeMsg::decode(&build(&[1, 2, 3, 4])),
            Err(DecodeError::BadReplicas)
        );
        assert_eq!(
            RangeMsg::decode(&build(&[2, 1])),
            Err(DecodeError::BadReplicas)
        );
        assert_eq!(
            RangeMsg::decode(&build(&[2, 2])),
            Err(DecodeError::BadReplicas)
        );
    }

    #[test]
    fn decoder_never_panics_on_20000_deterministic_byte_strings() {
        let mut rng = Rng::new(0xF0_22);
        let mut accepted = 0;
        for i in 0..20_000u32 {
            let len = rng.between(0, 48) as usize;
            let raw = rng.bytes(len);
            let input = if i % 2 == 0 {
                raw
            } else {
                let mut body = raw;
                if let Some(first) = body.first_mut() {
                    *first %= 10;
                }
                frame(&body)
            };
            if let Ok(msg) = RangeMsg::decode(&input) {
                assert_eq!(msg.encode(), input);
                accepted += 1;
            }
        }
        assert!(accepted > 0);
    }

    #[test]
    fn decoder_never_panics_on_byte_mutations_of_valid_encodings() {
        let mut rng = Rng::new(0xB17E);
        for m in sample_messages() {
            let bytes = m.encode();
            for pos in 0..bytes.len() {
                for _ in 0..8 {
                    let mut mutated = bytes.clone();
                    mutated[pos] = rng.next_u64() as u8;
                    if let Ok(msg) = RangeMsg::decode(&mutated) {
                        assert_eq!(msg.encode(), mutated);
                    }
                }
            }
        }
    }
}
