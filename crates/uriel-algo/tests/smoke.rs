//! 100k-node / 500k-edge random-graph smoke test. Run with:
//! `cargo test -p uriel-algo --release -- --ignored --nocapture`

use std::time::Instant;

use uriel_algo::*;

mod common;
use common::{approx, random_edges};

const N: u32 = 100_000;
const M: usize = 500_000;

fn timed<T>(label: &str, total: &mut f64, f: impl FnOnce() -> T) -> T {
    let t = Instant::now();
    let r = f();
    let s = t.elapsed().as_secs_f64();
    *total += s;
    println!("{label:<28} {:>9.1} ms", s * 1000.0);
    r
}

#[test]
#[ignore = "large; run in release mode with --ignored"]
fn every_algorithm_on_100k_nodes_500k_edges() {
    let mut total = 0.0;
    let mut rng = XorShift64::new(20_261_015);
    let weighted = random_edges(&mut rng, N, M, 1.0, 100.0);
    let plain: Vec<(u32, u32)> = weighted.iter().map(|&(u, v, _)| (u, v)).collect();

    let g = timed("build directed weighted", &mut total, || {
        Projection::from_weighted_edges(N as usize, &weighted, true).unwrap()
    });
    let u = timed("build undirected", &mut total, || {
        Projection::from_edges(N as usize, &plain, false).unwrap()
    });
    assert_eq!((g.edge_count(), u.edge_count()), (M, M));

    let b = timed("bfs", &mut total, || bfs(&g, 0).unwrap());
    assert!(b.order.len() > N as usize / 2);
    let d = timed("dfs", &mut total, || dfs(&g, 0).unwrap());
    assert_eq!(d.order.len(), b.order.len());

    let sp = timed("dijkstra", &mut total, || dijkstra(&g, 0).unwrap());
    assert_eq!(
        sp.dist.iter().filter(|x| x.is_finite()).count(),
        b.order.len()
    );
    let bf = timed("bellman_ford", &mut total, || bellman_ford(&g, 0).unwrap());
    assert!(approx(
        bf.dist[b.order[b.order.len() - 1] as usize],
        sp.dist[b.order[b.order.len() - 1] as usize],
        1e-6
    ));
    let target = *b.order.last().unwrap();
    let a = timed("astar (h = 0)", &mut total, || {
        astar(&g, 0, target, |_| 0.0).unwrap()
    });
    assert!(approx(a.unwrap().cost, sp.dist[target as usize], 1e-6));
    let y = timed("yen k=3", &mut total, || {
        yen_k_shortest_paths(&g, 0, target, 3).unwrap()
    });
    assert!(!y.is_empty() && y.windows(2).all(|w| w[0].cost <= w[1].cost + 1e-9));

    let wcc = timed("wcc", &mut total, || weakly_connected_components(&g));
    let scc = timed("scc (tarjan)", &mut total, || {
        strongly_connected_components(&g)
    });
    assert!(component_count(&wcc) <= component_count(&scc));
    assert_eq!(
        component_sizes(&wcc).iter().map(|x| x.1).sum::<usize>(),
        N as usize
    );

    let pr = timed("pagerank", &mut total, || {
        pagerank(&g, &PageRankConfig::default()).unwrap()
    });
    assert!(approx(pr.scores.iter().sum::<f64>(), 1.0, 1e-6));
    let ppr = timed("personalized pagerank", &mut total, || {
        personalized_pagerank(&g, &[0], &PageRankConfig::default()).unwrap()
    });
    assert!(approx(ppr.scores.iter().sum::<f64>(), 1.0, 1e-6));
    let deg = timed("degree centrality", &mut total, || {
        degree_centrality(&g, Direction::Both, false)
    });
    assert_eq!(deg.iter().sum::<f64>() as usize, 2 * M);
    let bc = timed("betweenness (k=16 sources)", &mut total, || {
        betweenness_centrality(
            &u,
            &BetweennessConfig {
                sample_size: Some(16),
                ..Default::default()
            },
        )
        .unwrap()
    });
    assert!(bc.iter().all(|x| *x >= 0.0));

    let lv = timed("louvain", &mut total, || {
        louvain(&u, &LouvainConfig::default()).unwrap()
    });
    println!("  louvain: {} levels, Q = {:.4}", lv.levels, lv.modularity);
    assert!(lv.modularity >= 0.0);
    let lp = timed("label propagation", &mut total, || {
        label_propagation(&u, &LabelPropagationConfig::default()).unwrap()
    });
    assert_eq!(lp.labels.len(), N as usize);

    let tri = timed("triangle count", &mut total, || triangle_count(&u));
    assert_eq!(tri.per_node.iter().sum::<u64>(), 3 * tri.total);
    let core = timed("k-core", &mut total, || k_core_decomposition(&u));
    assert!(core.iter().max().copied().unwrap_or(0) >= 2);

    let wm = timed("similarity (jaccard)", &mut total, || {
        // restricted to the first 2k nodes' neighbourhoods to keep pair counts sane
        let small = Projection::from_edges(
            2000,
            &plain
                .iter()
                .filter(|e| e.0 < 2000 && e.1 < 2000)
                .copied()
                .collect::<Vec<_>>(),
            true,
        )
        .unwrap();
        node_similarity(&small, &SimilarityConfig::default())
            .unwrap()
            .len()
    });
    println!("  similarity pairs on 2k-node induced subgraph: {wm}");

    let dag_edges: Vec<(u32, u32)> = plain
        .iter()
        .filter(|e| e.0 != e.1)
        .map(|&(a, b)| (a.min(b), a.max(b)))
        .collect();
    let dag = Projection::from_edges(N as usize, &dag_edges, true).unwrap();
    let order = timed("topological sort", &mut total, || {
        topological_sort(&dag).unwrap()
    });
    assert_eq!(order.len(), N as usize);

    let mst = timed("kruskal mst", &mut total, || kruskal_mst(&u));
    assert_eq!(mst.edges.len(), N as usize - mst.components);
    let pm = timed("prim mst (component of 0)", &mut total, || {
        prim_mst(&g, 0).unwrap()
    });
    assert!(pm.edges.len() > 1000);

    let walks = timed("random walks (1 x 10)", &mut total, || {
        random_walks(
            &u,
            &RandomWalkConfig {
                walk_length: 10,
                walks_per_node: 1,
                seed: 1,
                weighted: false,
            },
        )
        .unwrap()
    });
    assert_eq!(walks.len(), N as usize);

    let fl = timed("dinic max flow", &mut total, || {
        max_flow(&g, 0, target).unwrap()
    });
    println!("  max flow 0 -> {target}: {:.1}", fl.value);
    assert!(fl.value > 0.0);

    println!("{:<28} {:>9.1} ms", "TOTAL (algorithms)", total * 1000.0);
    assert!(total < 300.0, "smoke run took {total:.1}s");
}
