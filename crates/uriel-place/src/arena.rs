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

#[derive(Clone, Copy, PartialEq, Eq)]
enum Fit {
    BestGap,
    FirstGap,
    Append,
}

fn align_up(x: u64, a: u64) -> Option<u64> {
    let m = a - 1;
    x.checked_add(m).map(|v| v & !m)
}

fn validate(buffers: &[Buffer]) -> Result<(), ArenaError> {
    for b in buffers {
        if b.align == 0 || !b.align.is_power_of_two() {
            return Err(ArenaError::BadAlignment(b.id));
        }
        if b.last_use < b.first_use {
            return Err(ArenaError::InvertedLifetime(b.id));
        }
    }
    let mut ids: Vec<u32> = buffers.iter().map(|b| b.id).collect();
    ids.sort_unstable();
    match ids.windows(2).find(|w| w[0] == w[1]) {
        Some(w) => Err(ArenaError::DuplicateId(w[0])),
        None => Ok(()),
    }
}

fn pick(blocks: &[(u64, u64)], size: u64, align: u64, fit: Fit) -> Result<u64, ArenaError> {
    let mut cursor = 0u64;
    let mut best: Option<(u64, u64)> = None;
    for &(start, end) in blocks {
        if start > cursor && fit != Fit::Append {
            if let Some(off) = align_up(cursor, align) {
                if off.checked_add(size).is_some_and(|e| e <= start) {
                    if fit == Fit::FirstGap {
                        return Ok(off);
                    }
                    let gap = start - cursor;
                    if best.is_none_or(|(g, _)| gap < g) {
                        best = Some((gap, off));
                    }
                }
            }
        }
        cursor = cursor.max(end);
    }
    match best {
        Some((_, off)) => Ok(off),
        None => align_up(cursor, align).ok_or(ArenaError::Overflow),
    }
}

fn place(buffers: &[Buffer], fit: Fit) -> Result<Plan, ArenaError> {
    validate(buffers)?;
    let mut order = buffers.to_vec();
    order.sort_unstable_by_key(|b| {
        (
            core::cmp::Reverse(b.size),
            core::cmp::Reverse(b.last_use - b.first_use),
            b.first_use,
            b.last_use,
            core::cmp::Reverse(b.align),
            b.id,
        )
    });
    let n = order.len();
    let mut by_time: Vec<usize> = (0..n).collect();
    by_time.sort_unstable_by_key(|&i| (order[i].first_use, i));
    let mut rank = vec![0usize; n];
    for (pos, &i) in by_time.iter().enumerate() {
        rank[i] = pos;
    }
    let firsts: Vec<u64> = by_time.iter().map(|&i| order[i].first_use).collect();
    let cap = n.next_power_of_two();
    let mut tree = vec![0u128; 2 * cap];
    let mut offs = vec![0u64; n];
    let mut stack: Vec<(usize, usize, usize)> = Vec::new();
    let mut blocks: Vec<(u64, u64)> = Vec::new();
    let mut offsets: Vec<(u32, u64)> = Vec::with_capacity(n);
    let mut peak = 0u64;
    for (i, b) in order.iter().enumerate() {
        if b.size == 0 {
            offsets.push((b.id, 0));
            continue;
        }
        blocks.clear();
        let hi = firsts.partition_point(|&f| f <= b.last_use);
        let need = u128::from(b.first_use) + 1;
        stack.push((1, 0, cap));
        while let Some((node, l, r)) = stack.pop() {
            if l >= hi || tree[node] < need {
                continue;
            }
            if r - l == 1 {
                let j = by_time[l];
                blocks.push((offs[j], offs[j] + order[j].size));
            } else {
                let m = (l + r) / 2;
                stack.push((2 * node, l, m));
                stack.push((2 * node + 1, m, r));
            }
        }
        blocks.sort_unstable();
        let off = pick(&blocks, b.size, b.align, fit)?;
        let end = off.checked_add(b.size).ok_or(ArenaError::Overflow)?;
        peak = peak.max(end);
        offs[i] = off;
        offsets.push((b.id, off));
        let mut node = cap + rank[i];
        tree[node] = u128::from(b.last_use) + 1;
        while node > 1 {
            node /= 2;
            tree[node] = tree[2 * node].max(tree[2 * node + 1]);
        }
    }
    offsets.sort_unstable();
    Ok(Plan { offsets, peak })
}

pub fn plan_offsets(buffers: &[Buffer]) -> Result<Plan, ArenaError> {
    place(buffers, Fit::BestGap)
}

pub fn plan_first_fit(buffers: &[Buffer]) -> Result<Plan, ArenaError> {
    place(buffers, Fit::FirstGap)
}

pub fn plan_append_only(buffers: &[Buffer]) -> Result<Plan, ArenaError> {
    place(buffers, Fit::Append)
}

pub fn lower_bound(buffers: &[Buffer]) -> u64 {
    let mut events: Vec<(u64, i128)> = Vec::with_capacity(buffers.len() * 2);
    for b in buffers.iter().filter(|b| b.size > 0 && b.last_use >= b.first_use) {
        events.push((b.first_use, i128::from(b.size)));
        if let Some(end) = b.last_use.checked_add(1) {
            events.push((end, -i128::from(b.size)));
        }
    }
    events.sort_unstable();
    let (mut live, mut top) = (0i128, 0i128);
    for (_, d) in events {
        live += d;
        top = top.max(live);
    }
    u64::try_from(top).unwrap_or(u64::MAX)
}

pub fn plan_arena(items: &[(u64, u64, u64)], align: u64) -> Result<(Vec<u64>, u64), ArenaError> {
    let mut buffers: Vec<Buffer> = Vec::with_capacity(items.len());
    for (i, &(size, first_use, last_use)) in items.iter().enumerate() {
        buffers.push(Buffer {
            id: u32::try_from(i).map_err(|_| ArenaError::Overflow)?,
            size,
            align,
            first_use,
            last_use,
        });
    }
    let plan = plan_offsets(&buffers)?;
    Ok((plan.offsets.iter().map(|&(_, o)| o).collect(), plan.peak))
}
