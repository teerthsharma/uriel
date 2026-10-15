use crate::arena::{round_up_slab, Arena, Slab, SLAB_ALIGN};
use crate::cap::{Cap, Read, Width, Write};
use crate::error::SysError;
use crate::region::Region;

const HEADER_BYTES: usize = SLAB_ALIGN;
const HEADER_WORDS: usize = 7;

#[derive(Debug, Clone, Copy)]
struct Array {
    read: Cap<Read>,
    write: Cap<Write>,
}

fn make_array(region: &Region, slab: &Slab, rel: usize, elems: usize) -> Result<Array, SysError> {
    let offset = slab.offset().checked_add(rel).ok_or(SysError::TooLarge)?;
    let bytes = elems.max(1).checked_mul(4).ok_or(SysError::TooLarge)?;
    Ok(Array {
        read: region.cap_read(offset, bytes, Width::W32)?,
        write: region.cap_write(offset, bytes, Width::W32)?,
    })
}

fn padded(bytes: usize) -> Result<usize, SysError> {
    round_up_slab(bytes).ok_or(SysError::TooLarge)
}

fn array_bytes(elems: usize) -> Result<usize, SysError> {
    padded(elems.max(1).checked_mul(4).ok_or(SysError::TooLarge)?)
}

pub struct HotCsr<'a> {
    arena: &'a mut Arena,
    nodes: usize,
    edges: usize,
    slab_bytes: usize,
    header: Cap<Read>,
    offsets: Array,
    neighbours: Array,
    degree: Array,
    tag: Array,
    visits: Array,
}

impl<'a> HotCsr<'a> {
    pub fn build(
        arena: &'a mut Arena,
        node_count: usize,
        edges: &[(u32, u32)],
    ) -> Result<HotCsr<'a>, SysError> {
        if node_count >= u32::MAX as usize || edges.len() > u32::MAX as usize {
            return Err(SysError::TooLarge);
        }
        let n = node_count;
        let m = edges.len();
        if edges
            .iter()
            .any(|&(s, d)| s as usize >= n || d as usize >= n)
        {
            return Err(SysError::OutOfRange);
        }
        let offsets_bytes = array_bytes(n.checked_add(1).ok_or(SysError::TooLarge)?)?;
        let neighbours_bytes = array_bytes(m)?;
        let attr_bytes = array_bytes(n)?;
        let total = HEADER_BYTES
            .checked_add(offsets_bytes)
            .and_then(|v| v.checked_add(neighbours_bytes))
            .and_then(|v| v.checked_add(attr_bytes.checked_mul(3)?))
            .ok_or(SysError::TooLarge)?;
        if total > u32::MAX as usize {
            return Err(SysError::TooLarge);
        }
        let slab = arena.alloc(total, Width::W32)?;
        let offsets_at = HEADER_BYTES;
        let neighbours_at = offsets_at + offsets_bytes;
        let degree_at = neighbours_at + neighbours_bytes;
        let tag_at = degree_at + attr_bytes;
        let visits_at = tag_at + attr_bytes;
        let region = arena.region();
        let offsets = make_array(region, &slab, offsets_at, n + 1)?;
        let neighbours = make_array(region, &slab, neighbours_at, m)?;
        let degree = make_array(region, &slab, degree_at, n)?;
        let tag = make_array(region, &slab, tag_at, n)?;
        let visits = make_array(region, &slab, visits_at, n)?;
        let header_fields: [usize; HEADER_WORDS] = [
            n,
            m,
            offsets_at,
            neighbours_at,
            degree_at,
            tag_at,
            visits_at,
        ];
        let region = arena.region_mut();
        let header_cap = region.cap_write(slab.offset(), HEADER_BYTES, Width::W32)?;
        for (i, field) in header_fields.iter().enumerate() {
            region.write::<u32>(&header_cap, i * 4, *field as u32)?;
        }
        {
            let counts = region.view_mut::<u32>(&offsets.write)?;
            for &(s, _) in edges {
                counts[s as usize + 1] += 1;
            }
            let mut running = 0u32;
            for slot in counts.iter_mut() {
                running += *slot;
                *slot = running;
            }
        }
        for &(s, d) in edges {
            let at = s as usize * 4;
            let idx = region.read::<u32>(&offsets.read, at)?;
            region.write::<u32>(&neighbours.write, idx as usize * 4, d)?;
            region.write::<u32>(&offsets.write, at, idx + 1)?;
        }
        {
            let shifted = region.view_mut::<u32>(&offsets.write)?;
            shifted.copy_within(0..n, 1);
            shifted[0] = 0;
        }
        for i in 0..n {
            let a = region.read::<u32>(&offsets.read, i * 4)?;
            let b = region.read::<u32>(&offsets.read, (i + 1) * 4)?;
            region.write::<u32>(&degree.write, i * 4, b - a)?;
        }
        Ok(HotCsr {
            arena,
            nodes: n,
            edges: m,
            slab_bytes: slab.padded_len(),
            header: header_cap.downgrade(),
            offsets,
            neighbours,
            degree,
            tag,
            visits,
        })
    }

    pub fn node_count(&self) -> usize {
        self.nodes
    }

    pub fn edge_count(&self) -> usize {
        self.edges
    }

    pub fn bytes_used(&self) -> usize {
        self.slab_bytes
    }

    pub fn neighbours(&self, node: u32) -> &[u32] {
        let region = self.arena.region();
        let Ok(offsets) = region.view::<u32>(&self.offsets.read) else {
            return &[];
        };
        let i = node as usize;
        let (Some(&start), Some(&end)) = (offsets.get(i), offsets.get(i + 1)) else {
            return &[];
        };
        match region.view::<u32>(&self.neighbours.read) {
            Ok(all) => all.get(start as usize..end as usize).unwrap_or(&[]),
            Err(_) => &[],
        }
    }

    pub fn degree(&self, node: u32) -> Option<u32> {
        self.attr(&self.degree, node)
    }

    pub fn tag(&self, node: u32) -> Option<u32> {
        self.attr(&self.tag, node)
    }

    pub fn visits(&self, node: u32) -> Option<u32> {
        self.attr(&self.visits, node)
    }

    pub fn set_tag(&mut self, node: u32, value: u32) -> Result<(), SysError> {
        self.set_attr(self.tag, node, value)
    }

    pub fn bump_visits(&mut self, node: u32) -> Result<u32, SysError> {
        let next = self.visits(node).ok_or(SysError::OutOfRange)?.wrapping_add(1);
        self.set_attr(self.visits, node, next)?;
        Ok(next)
    }

    pub fn layout_alignments(&self) -> [usize; 6] {
        let region = self.arena.region();
        let align = |c: &Cap<Read>| region.alignment_of(c).unwrap_or(0);
        [
            align(&self.header),
            align(&self.offsets.read),
            align(&self.neighbours.read),
            align(&self.degree.read),
            align(&self.tag.read),
            align(&self.visits.read),
        ]
    }

    fn attr(&self, array: &Array, node: u32) -> Option<u32> {
        if node as usize >= self.nodes {
            return None;
        }
        self.arena
            .region()
            .read::<u32>(&array.read, node as usize * 4)
            .ok()
    }

    fn set_attr(&mut self, array: Array, node: u32, value: u32) -> Result<(), SysError> {
        if node as usize >= self.nodes {
            return Err(SysError::OutOfRange);
        }
        self.arena
            .region_mut()
            .write::<u32>(&array.write, node as usize * 4, value)
    }
}
