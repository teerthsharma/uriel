use uriel_algo::{louvain, LouvainConfig, Projection};

use crate::error::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub order: Vec<u32>,
    pub position: Vec<u32>,
    pub communities: Vec<u32>,
}

struct Canon {
    by_rank: Vec<u32>,
    rank_of: Vec<u32>,
    edges: Vec<(u32, u32)>,
    adj: Vec<Vec<u32>>,
}

fn canonicalise(n: usize, edges: &[(u32, u32)], ids: Option<&[u64]>) -> Result<Canon> {
    if n > u32::MAX as usize {
        return Err(Error::OutOfRange(format!("{n} nodes exceed u32")));
    }
    let keys: Vec<u64> = match ids {
        Some(v) => {
            if v.len() != n {
                return Err(Error::Shape(format!("{} ids for {n} nodes", v.len())));
            }
            v.to_vec()
        }
        None => (0..n as u64).collect(),
    };
    let mut by_rank: Vec<u32> = (0..n as u32).collect();
    by_rank.sort_by_key(|&u| keys[u as usize]);
    if let Some(w) = by_rank
        .windows(2)
        .find(|w| keys[w[0] as usize] == keys[w[1] as usize])
    {
        return Err(Error::DuplicateId(keys[w[0] as usize]));
    }
    let mut rank_of = vec![0u32; n];
    for (r, &u) in by_rank.iter().enumerate() {
        rank_of[u as usize] = r as u32;
    }
    let mut ce: Vec<(u32, u32)> = Vec::with_capacity(edges.len());
    for &(a, b) in edges {
        if a as usize >= n || b as usize >= n {
            return Err(Error::OutOfRange(format!("edge ({a}, {b}) with {n} nodes")));
        }
        let (ra, rb) = (rank_of[a as usize], rank_of[b as usize]);
        ce.push((ra.min(rb), ra.max(rb)));
    }
    ce.sort_unstable();
    let mut adj: Vec<Vec<u32>> = vec![Vec::new(); n];
    let mut prev: Option<(u32, u32)> = None;
    for &(a, b) in &ce {
        if a != b && prev != Some((a, b)) {
            adj[a as usize].push(b);
            adj[b as usize].push(a);
        }
        prev = Some((a, b));
    }
    for l in &mut adj {
        l.sort_unstable();
    }
    Ok(Canon {
        by_rank,
        rank_of,
        edges: ce,
        adj,
    })
}

fn bfs_levels(
    s: u32,
    c: u32,
    comm: &[u32],
    adj: &[Vec<u32>],
    dist: &mut [u32],
    touched: &mut Vec<u32>,
) -> (u32, Vec<u32>) {
    touched.clear();
    dist[s as usize] = 0;
    touched.push(s);
    let mut head = 0;
    let mut ecc = 0;
    while head < touched.len() {
        let u = touched[head];
        head += 1;
        let du = dist[u as usize];
        ecc = du;
        for &v in &adj[u as usize] {
            if comm[v as usize] == c && dist[v as usize] == u32::MAX {
                dist[v as usize] = du + 1;
                touched.push(v);
            }
        }
    }
    let last: Vec<u32> = touched
        .iter()
        .copied()
        .filter(|&v| dist[v as usize] == ecc)
        .collect();
    for &v in touched.iter() {
        dist[v as usize] = u32::MAX;
    }
    (ecc, last)
}

fn rcm_ranks(n: usize, comm: &[u32], k: usize, adj: &[Vec<u32>]) -> Vec<u32> {
    let mut members: Vec<Vec<u32>> = vec![Vec::new(); k];
    for r in 0..n {
        members[comm[r] as usize].push(r as u32);
    }
    let mut visited = vec![false; n];
    let mut dist = vec![u32::MAX; n];
    let mut touched: Vec<u32> = Vec::new();
    let mut out: Vec<u32> = Vec::with_capacity(n);
    for (c, group) in members.iter().enumerate() {
        let c = c as u32;
        let deg = |u: u32| {
            adj[u as usize]
                .iter()
                .filter(|&&v| comm[v as usize] == c)
                .count()
        };
        for &seed in group {
            if visited[seed as usize] {
                continue;
            }
            let mut r = seed;
            let (mut ecc_r, mut last) = bfs_levels(r, c, comm, adj, &mut dist, &mut touched);
            loop {
                let x = last.iter().copied().min().unwrap_or(r);
                if x == r {
                    break;
                }
                let (ecc_x, last_x) = bfs_levels(x, c, comm, adj, &mut dist, &mut touched);
                if ecc_x > ecc_r {
                    r = x;
                    ecc_r = ecc_x;
                    last = last_x;
                } else {
                    break;
                }
            }
            let mut piece: Vec<u32> = vec![r];
            visited[r as usize] = true;
            let mut head = 0;
            while head < piece.len() {
                let u = piece[head];
                head += 1;
                let mut nb: Vec<(usize, u32)> = adj[u as usize]
                    .iter()
                    .copied()
                    .filter(|&v| comm[v as usize] == c && !visited[v as usize])
                    .map(|v| (deg(v), v))
                    .collect();
                nb.sort_unstable();
                for (_, v) in nb {
                    visited[v as usize] = true;
                    piece.push(v);
                }
            }
            piece.reverse();
            out.extend(piece);
        }
    }
    out
}

pub fn topology_order(
    n: usize,
    edges: &[(u32, u32)],
    stable_ids: Option<&[u64]>,
) -> Result<Layout> {
    let canon = canonicalise(n, edges, stable_ids)?;
    if n == 0 {
        return Ok(Layout {
            order: Vec::new(),
            position: Vec::new(),
            communities: Vec::new(),
        });
    }
    let g = Projection::from_edges(n, &canon.edges, false)?;
    let comm = louvain(&g, &LouvainConfig::default())?.communities;
    let k = comm.iter().copied().max().map_or(0, |x| x as usize + 1);
    let ranks = rcm_ranks(n, &comm, k, &canon.adj);
    let order: Vec<u32> = ranks.iter().map(|&r| canon.by_rank[r as usize]).collect();
    let mut position = vec![0u32; n];
    for (p, &u) in order.iter().enumerate() {
        position[u as usize] = p as u32;
    }
    let communities = (0..n).map(|u| comm[canon.rank_of[u] as usize]).collect();
    Ok(Layout {
        order,
        position,
        communities,
    })
}

pub fn reverse_cuthill_mckee(
    n: usize,
    edges: &[(u32, u32)],
    stable_ids: Option<&[u64]>,
) -> Result<Vec<u32>> {
    let canon = canonicalise(n, edges, stable_ids)?;
    let comm = vec![0u32; n];
    let ranks = rcm_ranks(n, &comm, usize::from(n > 0), &canon.adj);
    Ok(ranks.iter().map(|&r| canon.by_rank[r as usize]).collect())
}
