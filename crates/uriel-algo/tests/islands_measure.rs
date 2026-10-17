use std::time::{Duration, Instant};

use peak_alloc::PeakAlloc;
use uriel_algo::{island_labels, weakly_connected_components, Projection, XorShift64};

#[global_allocator]
static ALLOC: PeakAlloc = PeakAlloc;

const NODES: usize = 100_000;
const EDGES: usize = 80_000;
const RUNS: usize = 7;

fn matrix_dfs_labels(g: &Projection) -> Vec<u32> {
    let n = g.node_count();
    let words = n.div_ceil(64);
    let mut adj = vec![0u64; n * words];
    for u in 0..n as u32 {
        for &v in g.out_neighbors(u) {
            adj[u as usize * words + (v as usize >> 6)] |= 1 << (v & 63);
            adj[v as usize * words + (u as usize >> 6)] |= 1 << (u & 63);
        }
    }
    let mut labels = vec![u32::MAX; n];
    let mut stack = Vec::new();
    for s in 0..n as u32 {
        if labels[s as usize] != u32::MAX {
            continue;
        }
        labels[s as usize] = s;
        stack.push(s);
        while let Some(u) = stack.pop() {
            let row = &adj[u as usize * words..(u as usize + 1) * words];
            for (w, &bits) in row.iter().enumerate() {
                let mut bits = bits;
                while bits != 0 {
                    let v = (w * 64 + bits.trailing_zeros() as usize) as u32;
                    bits &= bits - 1;
                    if labels[v as usize] == u32::MAX {
                        labels[v as usize] = s;
                        stack.push(v);
                    }
                }
            }
        }
    }
    labels
}

fn measure(f: impl Fn() -> Vec<u32>) -> (Duration, Duration, usize) {
    let mut times: Vec<Duration> = (0..RUNS)
        .map(|_| {
            let start = Instant::now();
            std::hint::black_box(f());
            start.elapsed()
        })
        .collect();
    times.sort();
    let base = ALLOC.current_usage();
    ALLOC.reset_peak_usage();
    let out = f();
    let extra = ALLOC.peak_usage() - base;
    drop(out);
    (times[0], times[RUNS / 2], extra)
}

#[test]
#[ignore]
fn island_labels_vs_old_paths_on_100k_nodes() {
    let mut rng = XorShift64::new(100_000);
    let edges: Vec<(u32, u32)> = (0..EDGES)
        .map(|_| {
            (
                rng.gen_range(NODES as u64) as u32,
                rng.gen_range(NODES as u64) as u32,
            )
        })
        .collect();
    let g = Projection::from_edges(NODES, &edges, false).unwrap();
    let reference = island_labels(&g);
    assert_eq!(reference, weakly_connected_components(&g));
    assert_eq!(reference, matrix_dfs_labels(&g));

    let rows = [
        ("island_labels", measure(|| island_labels(&g))),
        (
            "weakly_connected_components",
            measure(|| weakly_connected_components(&g)),
        ),
        ("adjacency_bitmatrix_dfs", measure(|| matrix_dfs_labels(&g))),
    ];
    println!("nodes={NODES} edges={EDGES} runs={RUNS}");
    for (name, (min, median, extra)) in rows {
        println!("{name}: min={min:?} median={median:?} peak_extra_bytes={extra}");
    }
}
