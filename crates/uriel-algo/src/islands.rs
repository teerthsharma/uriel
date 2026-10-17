use crate::components::{component_sizes, find};
use crate::projection::Projection;

pub fn island_labels(g: &Projection) -> Vec<u32> {
    let n = g.node_count();
    let mut parent: Vec<u32> = (0..n as u32).collect();
    let mut size = vec![1u32; n];
    let mut low: Vec<u32> = (0..n as u32).collect();
    for u in 0..n as u32 {
        for &v in g.out_neighbors(u) {
            let (mut a, mut b) = (find(&mut parent, u), find(&mut parent, v));
            if a == b {
                continue;
            }
            if size[a as usize] < size[b as usize] {
                std::mem::swap(&mut a, &mut b);
            }
            parent[b as usize] = a;
            size[a as usize] += size[b as usize];
            low[a as usize] = low[a as usize].min(low[b as usize]);
        }
    }
    for u in 0..n as u32 {
        let r = find(&mut parent, u);
        size[u as usize] = low[r as usize];
    }
    size
}

pub fn bucket_islands(labels: &[u32], bounds: &[usize]) -> Vec<Vec<u32>> {
    let mut buckets = vec![Vec::new(); bounds.len() + 1];
    for (label, size) in component_sizes(labels) {
        let slot = bounds
            .iter()
            .position(|&b| b >= size)
            .unwrap_or(bounds.len());
        buckets[slot].push(label);
    }
    buckets
}
