#![allow(clippy::needless_range_loop)]
use uriel_algo::{
    all_pairs_shortest_paths, astar, bellman_ford, dijkstra, dijkstra_path, yen_k_shortest_paths,
    Error, Projection, XorShift64, MAX_ALL_PAIRS_NODES,
};

mod common;
use common::{approx, random_edges};

/// CLRS fig. 24.6: s=0 t=1 x=2 y=3 z=4.
fn clrs_dijkstra() -> Projection {
    Projection::from_weighted_edges(
        5,
        &[
            (0, 1, 10.0),
            (0, 3, 5.0),
            (1, 2, 1.0),
            (1, 3, 2.0),
            (3, 1, 3.0),
            (3, 2, 9.0),
            (3, 4, 2.0),
            (2, 4, 4.0),
            (4, 0, 7.0),
            (4, 2, 6.0),
        ],
        true,
    )
    .unwrap()
}

/// CLRS fig. 24.4: s=0 t=1 x=2 y=3 z=4.
fn clrs_bellman_ford() -> Projection {
    Projection::from_weighted_edges(
        5,
        &[
            (0, 1, 6.0),
            (0, 3, 7.0),
            (1, 2, 5.0),
            (1, 3, 8.0),
            (1, 4, -4.0),
            (2, 1, -2.0),
            (3, 2, -3.0),
            (3, 4, 9.0),
            (4, 2, 7.0),
            (4, 0, 2.0),
        ],
        true,
    )
    .unwrap()
}

/// Cost of a node path using the cheapest parallel arc at every hop.
fn path_cost(g: &Projection, nodes: &[u32]) -> f64 {
    nodes
        .windows(2)
        .map(|w| {
            g.out_edges(w[0])
                .filter(|&(t, _)| t == w[1])
                .map(|(_, c)| c)
                .fold(f64::INFINITY, f64::min)
        })
        .sum()
}

#[test]
fn dijkstra_textbook_distances_and_path() {
    let g = clrs_dijkstra();
    let r = dijkstra(&g, 0).unwrap();
    assert_eq!(r.dist, vec![0.0, 8.0, 9.0, 5.0, 7.0]);
    assert_eq!(r.path_to(2), Some(vec![0, 3, 1, 2]));
    assert_eq!(r.path_to(0), Some(vec![0]));
    assert_eq!(r.distance_to(4), Some(7.0));
}

#[test]
fn dijkstra_unreachable_and_unweighted() {
    let g = Projection::from_edges(4, &[(0, 1), (1, 2)], true).unwrap();
    let r = dijkstra(&g, 0).unwrap();
    assert_eq!(r.dist[..3], [0.0, 1.0, 2.0]);
    assert!(r.dist[3].is_infinite());
    assert_eq!(r.path_to(3), None);
    assert_eq!(r.distance_to(3), None);
    assert_eq!(r.parent[3], None);
}

#[test]
fn dijkstra_rejects_negative_weights_and_bad_source() {
    let g = Projection::from_weighted_edges(2, &[(0, 1, -1.0)], true).unwrap();
    assert!(matches!(
        dijkstra(&g, 0),
        Err(Error::NegativeWeight { from: 0, to: 1, .. })
    ));
    assert!(matches!(dijkstra(&g, 5), Err(Error::NodeOutOfRange { .. })));
}

#[test]
fn dijkstra_with_parallel_edges_and_self_loops() {
    let g = Projection::from_weighted_edges(
        3,
        &[
            (0, 1, 5.0),
            (0, 1, 2.0),
            (1, 1, 1.0),
            (1, 2, 4.0),
            (0, 0, 0.5),
        ],
        true,
    )
    .unwrap();
    let r = dijkstra(&g, 0).unwrap();
    assert_eq!(r.dist, vec![0.0, 2.0, 6.0]);
    assert_eq!(r.path_to(2), Some(vec![0, 1, 2]));
}

#[test]
fn dijkstra_zero_weight_edges_and_empty_graph() {
    let g = Projection::from_weighted_edges(3, &[(0, 1, 0.0), (1, 2, 0.0)], true).unwrap();
    assert_eq!(dijkstra(&g, 0).unwrap().dist, vec![0.0, 0.0, 0.0]);
    let g = Projection::from_edges(0, &[], true).unwrap();
    assert!(dijkstra(&g, 0).is_err());
}

#[test]
fn dijkstra_path_stops_at_target() {
    let g = clrs_dijkstra();
    let p = dijkstra_path(&g, 0, 2).unwrap().unwrap();
    assert_eq!(p.nodes, vec![0, 3, 1, 2]);
    assert_eq!(p.cost, 9.0);
    let g = Projection::from_edges(3, &[(0, 1)], true).unwrap();
    assert_eq!(dijkstra_path(&g, 0, 2).unwrap(), None);
    assert_eq!(dijkstra_path(&g, 1, 1).unwrap().unwrap().nodes, vec![1]);
}

#[test]
fn bellman_ford_textbook_with_negative_edges() {
    let g = clrs_bellman_ford();
    let r = bellman_ford(&g, 0).unwrap();
    assert_eq!(r.dist, vec![0.0, 2.0, 4.0, 7.0, -2.0]);
    assert_eq!(r.path_to(4), Some(vec![0, 3, 2, 1, 4]));
}

#[test]
fn bellman_ford_detects_reachable_negative_cycle() {
    let g = Projection::from_weighted_edges(3, &[(0, 1, 1.0), (1, 2, -3.0), (2, 1, 1.0)], true)
        .unwrap();
    assert_eq!(bellman_ford(&g, 0).unwrap_err(), Error::NegativeCycle);
}

#[test]
fn bellman_ford_ignores_unreachable_negative_cycle() {
    let g = Projection::from_weighted_edges(
        5,
        &[(0, 1, 1.0), (1, 2, 1.0), (3, 4, -2.0), (4, 3, 1.0)],
        true,
    )
    .unwrap();
    let r = bellman_ford(&g, 0).unwrap();
    assert_eq!(r.dist[..3], [0.0, 1.0, 2.0]);
    assert!(r.dist[3].is_infinite() && r.dist[4].is_infinite());
}

#[test]
fn bellman_ford_negative_self_loop_and_undirected_negative_edge() {
    let g = Projection::from_weighted_edges(2, &[(0, 1, 1.0), (1, 1, -1.0)], true).unwrap();
    assert_eq!(bellman_ford(&g, 0).unwrap_err(), Error::NegativeCycle);
    let g = Projection::from_weighted_edges(2, &[(0, 1, -1.0)], false).unwrap();
    assert_eq!(bellman_ford(&g, 0).unwrap_err(), Error::NegativeCycle);
}

#[test]
fn dijkstra_equals_bellman_ford_on_200_random_graphs() {
    let mut rng = XorShift64::new(2024);
    for case in 0..200 {
        let n = 5 + rng.gen_range(40) as u32;
        let m = rng.gen_range(4 * n as u64 + 1) as usize;
        let edges = random_edges(&mut rng, n, m, 0.0, 20.0);
        let g = Projection::from_weighted_edges(n as usize, &edges, case % 2 == 0).unwrap();
        let s = rng.gen_range(n as u64) as u32;
        let d = dijkstra(&g, s).unwrap();
        let b = bellman_ford(&g, s).unwrap();
        for v in 0..n as usize {
            assert!(approx(d.dist[v], b.dist[v], 1e-9), "case {case} node {v}");
            if let Some(p) = d.path_to(v as u32) {
                assert_eq!(p[0], s);
                assert_eq!(*p.last().unwrap(), v as u32);
                assert!(
                    approx(path_cost(&g, &p), d.dist[v], 1e-9),
                    "case {case} path to {v}"
                );
            } else {
                assert!(d.dist[v].is_infinite());
            }
        }
    }
}

fn grid(w: u32, h: u32, blocked: &[(u32, u32)]) -> Projection {
    let id = |x: u32, y: u32| y * w + x;
    let mut e = Vec::new();
    for y in 0..h {
        for x in 0..w {
            if blocked.contains(&(x, y)) {
                continue;
            }
            for (nx, ny) in [(x + 1, y), (x, y + 1)] {
                if nx < w && ny < h && !blocked.contains(&(nx, ny)) {
                    e.push((id(x, y), id(nx, ny)));
                }
            }
        }
    }
    Projection::from_edges((w * h) as usize, &e, false).unwrap()
}

#[test]
fn astar_with_manhattan_heuristic_matches_dijkstra_on_a_grid() {
    let (w, h) = (12u32, 12u32);
    let blocked = [
        (3, 0),
        (3, 1),
        (3, 2),
        (3, 3),
        (3, 4),
        (7, 11),
        (7, 10),
        (7, 9),
        (7, 8),
    ];
    let g = grid(w, h, &blocked);
    let goal = w * h - 1;
    let (gx, gy) = (goal % w, goal / w);
    let manhattan = move |n: u32| ((n % w).abs_diff(gx) + (n / w).abs_diff(gy)) as f64;
    let a = astar(&g, 0, goal, manhattan).unwrap().unwrap();
    let d = dijkstra_path(&g, 0, goal).unwrap().unwrap();
    assert_eq!(a.cost, d.cost);
    assert_eq!(a.nodes.first(), Some(&0));
    assert_eq!(a.nodes.last(), Some(&goal));
    assert_eq!(path_cost(&g, &a.nodes), a.cost);
}

#[test]
fn astar_zero_heuristic_equals_dijkstra_on_random_graphs() {
    let mut rng = XorShift64::new(77);
    for case in 0..50 {
        let n = 20u32;
        let edges = random_edges(&mut rng, n, 70, 0.5, 9.0);
        let g = Projection::from_weighted_edges(n as usize, &edges, case % 2 == 0).unwrap();
        let (s, t) = (rng.gen_range(20) as u32, rng.gen_range(20) as u32);
        let a = astar(&g, s, t, |_| 0.0).unwrap();
        let d = dijkstra(&g, s).unwrap();
        match a {
            Some(p) => assert!(approx(p.cost, d.dist[t as usize], 1e-9)),
            None => assert!(d.dist[t as usize].is_infinite()),
        }
    }
}

#[test]
fn astar_edge_cases() {
    let g = Projection::from_edges(3, &[(0, 1)], true).unwrap();
    assert_eq!(astar(&g, 0, 2, |_| 0.0).unwrap(), None);
    let p = astar(&g, 1, 1, |_| 0.0).unwrap().unwrap();
    assert_eq!((p.nodes, p.cost), (vec![1], 0.0));
    assert!(matches!(
        astar(&g, 0, 9, |_| 0.0),
        Err(Error::NodeOutOfRange { .. })
    ));
    let neg = Projection::from_weighted_edges(2, &[(0, 1, -1.0)], true).unwrap();
    assert!(matches!(
        astar(&neg, 0, 1, |_| 0.0),
        Err(Error::NegativeWeight { .. })
    ));
}

/// Wikipedia's Yen example: C=0 D=1 E=2 F=3 G=4 H=5.
fn yen_graph() -> Projection {
    Projection::from_weighted_edges(
        6,
        &[
            (0, 1, 3.0),
            (0, 2, 2.0),
            (1, 3, 4.0),
            (2, 1, 1.0),
            (2, 3, 2.0),
            (2, 4, 3.0),
            (3, 4, 2.0),
            (3, 5, 1.0),
            (4, 5, 2.0),
        ],
        true,
    )
    .unwrap()
}

#[test]
fn yen_textbook_three_shortest() {
    let r = yen_k_shortest_paths(&yen_graph(), 0, 5, 3).unwrap();
    assert_eq!(
        r.iter().map(|p| p.cost).collect::<Vec<_>>(),
        vec![5.0, 7.0, 8.0]
    );
    assert_eq!(r[0].nodes, vec![0, 2, 3, 5]);
    assert_eq!(r[1].nodes, vec![0, 2, 4, 5]);
}

#[test]
fn yen_enumerates_all_seven_simple_paths_in_order() {
    let g = yen_graph();
    let r = yen_k_shortest_paths(&g, 0, 5, 20).unwrap();
    assert_eq!(
        r.iter().map(|p| p.cost).collect::<Vec<_>>(),
        vec![5.0, 7.0, 8.0, 8.0, 8.0, 11.0, 11.0]
    );
    let mut seen = std::collections::HashSet::new();
    for p in &r {
        assert!(seen.insert(p.nodes.clone()), "duplicate path");
        let mut uniq = p.nodes.clone();
        uniq.sort_unstable();
        uniq.dedup();
        assert_eq!(uniq.len(), p.nodes.len(), "path is not simple");
        assert_eq!(path_cost(&g, &p.nodes), p.cost);
    }
}

#[test]
fn yen_edge_cases() {
    let g = yen_graph();
    assert!(yen_k_shortest_paths(&g, 0, 5, 0).unwrap().is_empty());
    let same = yen_k_shortest_paths(&g, 2, 2, 3).unwrap();
    assert_eq!(same.len(), 1);
    assert_eq!(same[0].nodes, vec![2]);
    assert!(yen_k_shortest_paths(&g, 5, 0, 3).unwrap().is_empty());
    assert!(matches!(
        yen_k_shortest_paths(&g, 0, 9, 1),
        Err(Error::NodeOutOfRange { .. })
    ));
}

fn all_simple_path_costs(g: &Projection, s: u32, t: u32) -> Vec<f64> {
    fn rec(g: &Projection, u: u32, t: u32, seen: &mut Vec<bool>, cost: f64, out: &mut Vec<f64>) {
        if u == t {
            out.push(cost);
            return;
        }
        seen[u as usize] = true;
        let mut prev: Option<u32> = None;
        for (v, w) in g.out_edges(u) {
            // only the cheapest parallel arc defines a node path; arcs are sorted by target
            if prev == Some(v) {
                continue;
            }
            prev = Some(v);
            let best = g
                .out_edges(u)
                .filter(|&(x, _)| x == v)
                .map(|(_, w)| w)
                .fold(w, f64::min);
            if !seen[v as usize] {
                rec(g, v, t, seen, cost + best, out);
            }
        }
        seen[u as usize] = false;
    }
    let mut out = Vec::new();
    rec(g, s, t, &mut vec![false; g.node_count()], 0.0, &mut out);
    out.sort_by(|a, b| a.partial_cmp(b).unwrap());
    out
}

#[test]
fn yen_matches_brute_force_on_random_small_graphs() {
    let mut rng = XorShift64::new(909);
    for case in 0..60 {
        let n = 7u32;
        let edges = random_edges(&mut rng, n, 16, 1.0, 10.0);
        let g = Projection::from_weighted_edges(n as usize, &edges, case % 3 != 0).unwrap();
        let (s, t) = (0u32, 6u32);
        let truth = all_simple_path_costs(&g, s, t);
        let k = 6;
        let got = yen_k_shortest_paths(&g, s, t, k).unwrap();
        let want: Vec<f64> = truth.iter().take(k).copied().collect();
        assert_eq!(got.len(), want.len(), "case {case}");
        for (p, w) in got.iter().zip(&want) {
            assert!(approx(p.cost, *w, 1e-9), "case {case}: {} vs {w}", p.cost);
        }
    }
}

#[test]
fn all_pairs_matches_textbook_and_per_source_dijkstra() {
    let g = clrs_dijkstra();
    let m = all_pairs_shortest_paths(&g).unwrap();
    assert_eq!(m[0], vec![0.0, 8.0, 9.0, 5.0, 7.0]);
    assert_eq!(m[4][1], 15.0);
    let mut rng = XorShift64::new(5);
    let edges = random_edges(&mut rng, 30, 90, 0.1, 5.0);
    let g = Projection::from_weighted_edges(30, &edges, true).unwrap();
    let m = all_pairs_shortest_paths(&g).unwrap();
    for s in 0..30u32 {
        assert_eq!(m[s as usize], dijkstra(&g, s).unwrap().dist);
    }
}

#[test]
fn all_pairs_edge_cases() {
    let g = Projection::from_edges(0, &[], true).unwrap();
    assert!(all_pairs_shortest_paths(&g).unwrap().is_empty());
    let g = Projection::from_edges(MAX_ALL_PAIRS_NODES + 1, &[], true).unwrap();
    assert!(matches!(
        all_pairs_shortest_paths(&g),
        Err(Error::TooLarge { .. })
    ));
}
