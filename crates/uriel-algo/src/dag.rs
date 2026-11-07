//! Cycle breaking and layering of directed graphs: a greedy feedback arc set and
//! the topological layers of the strong-component condensation.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, VecDeque};

use crate::components::strongly_connected_components;
use crate::error::{Error, Result};
use crate::projection::Projection;

const PRUNE_WORK_BUDGET: usize = 20_000_000;

struct Local {
    out: Vec<Vec<u32>>,
    inc: Vec<Vec<u32>>,
}

fn intra_component_arcs(g: &Projection, scc: &[u32]) -> (Vec<(u32, u32)>, Vec<u32>) {
    let mut arcs: Vec<(u32, u32)> = Vec::new();
    let mut loops: Vec<u32> = Vec::new();
    for u in 0..g.node_count() as u32 {
        let mut prev = None;
        for &v in g.out_neighbors(u) {
            if prev == Some(v) {
                continue;
            }
            prev = Some(v);
            if v == u {
                loops.push(u);
            } else if scc[u as usize] == scc[v as usize] {
                arcs.push((u, v));
            }
        }
    }
    (arcs, loops)
}

fn eades_order(n: usize, arcs: &[(u32, u32)]) -> Vec<u32> {
    let mut l = Local {
        out: vec![Vec::new(); n],
        inc: vec![Vec::new(); n],
    };
    for &(u, v) in arcs {
        l.out[u as usize].push(v);
        l.inc[v as usize].push(u);
    }
    let mut outdeg: Vec<i64> = l.out.iter().map(|a| a.len() as i64).collect();
    let mut indeg: Vec<i64> = l.inc.iter().map(|a| a.len() as i64).collect();
    let mut removed = vec![false; n];
    let mut sinks: VecDeque<u32> = (0..n as u32).filter(|&u| outdeg[u as usize] == 0).collect();
    let mut sources: VecDeque<u32> = (0..n as u32)
        .filter(|&u| indeg[u as usize] == 0 && outdeg[u as usize] != 0)
        .collect();
    let mut heap: BinaryHeap<(i64, Reverse<u32>)> = (0..n as u32)
        .map(|u| (outdeg[u as usize] - indeg[u as usize], Reverse(u)))
        .collect();
    let mut left: Vec<u32> = Vec::with_capacity(n);
    let mut right: Vec<u32> = Vec::new();
    let mut remaining = n;
    while remaining > 0 {
        let (u, to_left) = if let Some(u) = pop_live(&mut sinks, &removed) {
            (u, false)
        } else if let Some(u) = pop_live(&mut sources, &removed) {
            (u, true)
        } else {
            let mut pick = None;
            while let Some((delta, Reverse(u))) = heap.pop() {
                if !removed[u as usize] && delta == outdeg[u as usize] - indeg[u as usize] {
                    pick = Some(u);
                    break;
                }
            }
            match pick {
                Some(u) => (u, true),
                None => break,
            }
        };
        removed[u as usize] = true;
        remaining -= 1;
        if to_left {
            left.push(u);
        } else {
            right.push(u);
        }
        for &v in &l.out[u as usize] {
            let vi = v as usize;
            if removed[vi] {
                continue;
            }
            indeg[vi] -= 1;
            if indeg[vi] == 0 {
                sources.push_back(v);
            }
            heap.push((outdeg[vi] - indeg[vi], Reverse(v)));
        }
        for &w in &l.inc[u as usize] {
            let wi = w as usize;
            if removed[wi] {
                continue;
            }
            outdeg[wi] -= 1;
            if outdeg[wi] == 0 {
                sinks.push_back(w);
            }
            heap.push((outdeg[wi] - indeg[wi], Reverse(w)));
        }
    }
    right.reverse();
    left.extend(right);
    left
}

fn pop_live(q: &mut VecDeque<u32>, removed: &[bool]) -> Option<u32> {
    while let Some(u) = q.pop_front() {
        if !removed[u as usize] {
            return Some(u);
        }
    }
    None
}

fn reaches(adj: &[Vec<u32>], from: u32, to: u32, seen: &mut [u32], stamp: u32, work: &mut usize) -> bool {
    let mut stack = vec![from];
    seen[from as usize] = stamp;
    while let Some(u) = stack.pop() {
        if u == to {
            return true;
        }
        for &v in &adj[u as usize] {
            *work += 1;
            if seen[v as usize] != stamp {
                seen[v as usize] = stamp;
                stack.push(v);
            }
        }
    }
    false
}

/// A greedy minimal feedback arc set: arcs whose removal leaves the graph
/// acyclic. Self loops are always included. Inside each strong component the
/// Eades-Lin-Smyth ordering heuristic picks back arcs; a pruning pass then
/// restores every arc that does not close a cycle, so no returned arc is
/// redundant (the pass stops after a fixed work budget on very large inputs and
/// the remaining candidates are kept). Parallel arcs count as one. Output is
/// sorted and deterministic.
///
/// Errors with [`Error::RequiresDirected`] on undirected projections.
///
/// Complexity: `O((n + m) log n)` for the heuristic plus at most
/// `PRUNE_WORK_BUDGET` steps of pruning.
pub fn feedback_arc_set(g: &Projection) -> Result<Vec<(u32, u32)>> {
    if !g.is_directed() {
        return Err(Error::RequiresDirected);
    }
    let n = g.node_count();
    let scc = strongly_connected_components(g);
    let (arcs, loops) = intra_component_arcs(g, &scc);
    let order = eades_order(n, &arcs);
    let mut pos = vec![0u32; n];
    for (i, &u) in order.iter().enumerate() {
        pos[u as usize] = i as u32;
    }
    let mut kept: Vec<Vec<u32>> = vec![Vec::new(); n];
    let mut back: Vec<(u32, u32)> = Vec::new();
    for &(u, v) in &arcs {
        if pos[u as usize] < pos[v as usize] {
            kept[u as usize].push(v);
        } else {
            back.push((u, v));
        }
    }
    back.sort_unstable();
    let mut seen = vec![0u32; n];
    let mut stamp = 0u32;
    let mut work = 0usize;
    let mut fas: Vec<(u32, u32)> = Vec::with_capacity(back.len() + loops.len());
    for (u, v) in back {
        if work < PRUNE_WORK_BUDGET {
            stamp += 1;
            if !reaches(&kept, v, u, &mut seen, stamp, &mut work) {
                kept[u as usize].push(v);
                continue;
            }
        }
        fas.push((u, v));
    }
    fas.extend(loops.into_iter().map(|u| (u, u)));
    fas.sort_unstable();
    Ok(fas)
}

/// Topological layer of every node's strong component in the condensation DAG:
/// source components are layer 0 and a component sits one layer below its
/// deepest predecessor. Nodes of one cycle share a layer.
///
/// Errors with [`Error::RequiresDirected`] on undirected projections.
///
/// Complexity: `O(n + m)` time and memory.
pub fn condensation_layers(g: &Projection) -> Result<Vec<u32>> {
    if !g.is_directed() {
        return Err(Error::RequiresDirected);
    }
    let n = g.node_count();
    let scc = strongly_connected_components(g);
    let mut start = vec![0usize; n + 1];
    for &c in &scc {
        start[c as usize + 1] += 1;
    }
    for i in 0..n {
        start[i + 1] += start[i];
    }
    let mut fill = start.clone();
    let mut members = vec![0u32; n];
    for u in 0..n as u32 {
        let c = scc[u as usize] as usize;
        members[fill[c]] = u;
        fill[c] += 1;
    }
    let mut indeg = vec![0usize; n];
    for u in 0..n as u32 {
        for &v in g.out_neighbors(u) {
            if scc[u as usize] != scc[v as usize] {
                indeg[scc[v as usize] as usize] += 1;
            }
        }
    }
    let mut layer = vec![0u32; n];
    let mut queue: VecDeque<u32> = (0..n as u32)
        .filter(|&c| start[c as usize + 1] > start[c as usize] && indeg[c as usize] == 0)
        .collect();
    while let Some(c) = queue.pop_front() {
        for &u in &members[start[c as usize]..start[c as usize + 1]] {
            for &v in g.out_neighbors(u) {
                let d = scc[v as usize];
                if d == c {
                    continue;
                }
                layer[d as usize] = layer[d as usize].max(layer[c as usize] + 1);
                indeg[d as usize] -= 1;
                if indeg[d as usize] == 0 {
                    queue.push_back(d);
                }
            }
        }
    }
    Ok((0..n).map(|u| layer[scc[u] as usize]).collect())
}
