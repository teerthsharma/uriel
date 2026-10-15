#![allow(clippy::needless_range_loop)]
use uriel_algo::{
    component_count, label_propagation, louvain, modularity, weakly_connected_components, Error,
    LabelPropagationConfig, LouvainConfig, Projection, XorShift64,
};

mod common;
use common::{approx, karate, random_edges, KARATE_CLUB};

fn two_triangles() -> Projection {
    Projection::from_edges(
        6,
        &[(0, 1), (1, 2), (0, 2), (3, 4), (4, 5), (3, 5), (2, 3)],
        false,
    )
    .unwrap()
}

fn ring_of_cliques(k: u32, size: u32) -> Projection {
    let mut e = Vec::new();
    for c in 0..k {
        for i in 0..size {
            for j in i + 1..size {
                e.push((c * size + i, c * size + j));
            }
        }
        e.push((c * size + size - 1, ((c + 1) % k) * size));
    }
    Projection::from_edges((k * size) as usize, &e, false).unwrap()
}

/// Q = 1/(2m) sum_ij (A_ij - k_i k_j / 2m) [c_i == c_j] on the multigraph matrix
/// (A_ii = 2 * loops), computed straight from the definition.
fn modularity_oracle(n: usize, edges: &[(u32, u32, f64)], comm: &[u32]) -> f64 {
    let mut a = vec![vec![0.0; n]; n];
    let mut m = 0.0;
    for &(u, v, w) in edges {
        let (u, v) = (u as usize, v as usize);
        if u == v {
            a[u][u] += 2.0 * w;
        } else {
            a[u][v] += w;
            a[v][u] += w;
        }
        m += w;
    }
    if m == 0.0 {
        return 0.0;
    }
    let k: Vec<f64> = (0..n).map(|i| a[i].iter().sum()).collect();
    let mut q = 0.0;
    for i in 0..n {
        for j in 0..n {
            if comm[i] == comm[j] {
                q += a[i][j] - k[i] * k[j] / (2.0 * m);
            }
        }
    }
    q / (2.0 * m)
}

#[test]
fn modularity_known_values() {
    let g = two_triangles();
    // m = 7; each side has 3 internal edges and degree sum 7
    let q = modularity(&g, &[0, 0, 0, 1, 1, 1], false).unwrap();
    assert!(approx(q, 2.0 * (6.0 / 14.0 - 0.25), 1e-12));
    assert!(approx(modularity(&g, &[5; 6], false).unwrap(), 0.0, 1e-12));
    assert!(approx(
        modularity(&karate(), &KARATE_CLUB, false).unwrap(),
        0.3582347140039448,
        1e-12
    ));
}

#[test]
fn modularity_validates_input() {
    let g = two_triangles();
    assert!(matches!(
        modularity(&g, &[0, 1], false),
        Err(Error::InvalidParameter(_))
    ));
    let e = Projection::from_edges(3, &[], false).unwrap();
    assert_eq!(modularity(&e, &[0, 1, 2], false).unwrap(), 0.0);
}

#[test]
fn modularity_matches_definition_with_loops_parallel_edges_and_weights() {
    let mut rng = XorShift64::new(17);
    for case in 0..100 {
        let n = 2 + rng.gen_range(12) as usize;
        let m = 1 + rng.gen_range(30) as usize;
        let edges = random_edges(&mut rng, n as u32, m, 0.5, 4.0);
        let directed = case % 2 == 0;
        let g = Projection::from_weighted_edges(n, &edges, directed).unwrap();
        let comm: Vec<u32> = (0..n).map(|_| rng.gen_range(4) as u32).collect();
        let got = modularity(&g, &comm, true).unwrap();
        assert!(
            approx(got, modularity_oracle(n, &edges, &comm), 1e-10),
            "case {case}"
        );
        let unit: Vec<_> = edges.iter().map(|&(u, v, _)| (u, v, 1.0)).collect();
        assert!(approx(
            modularity(&g, &comm, false).unwrap(),
            modularity_oracle(n, &unit, &comm),
            1e-10
        ));
    }
}

#[test]
fn louvain_splits_two_triangles() {
    let r = louvain(&two_triangles(), &LouvainConfig::default()).unwrap();
    assert_eq!(r.communities, vec![0, 0, 0, 1, 1, 1]);
    assert!(approx(r.modularity, 2.0 * (6.0 / 14.0 - 0.25), 1e-12));
}

#[test]
fn louvain_recovers_a_ring_of_cliques() {
    let r = louvain(&ring_of_cliques(4, 5), &LouvainConfig::default()).unwrap();
    for c in 0..4usize {
        let first = r.communities[c * 5];
        assert!(
            (0..5).all(|i| r.communities[c * 5 + i] == first),
            "clique {c} split"
        );
    }
    let mut distinct = r.communities.clone();
    distinct.sort_unstable();
    distinct.dedup();
    assert_eq!(distinct.len(), 4);
}

#[test]
fn louvain_on_karate_finds_good_modularity() {
    let g = karate();
    let r = louvain(&g, &LouvainConfig::default()).unwrap();
    assert!(r.modularity > 0.40, "Q = {}", r.modularity);
    assert!(r.modularity > modularity(&g, &KARATE_CLUB, false).unwrap());
    let k = *r.communities.iter().max().unwrap() + 1;
    assert!((3..=5).contains(&k), "{k} communities");
    assert!(approx(
        r.modularity,
        modularity(&g, &r.communities, false).unwrap(),
        1e-12
    ));
    assert!(r.levels >= 1);
    // the two hubs never share a community with each other
    assert_ne!(r.communities[0], r.communities[33]);
}

#[test]
fn louvain_uses_weights_when_asked() {
    let g = Projection::from_weighted_edges(
        4,
        &[(0, 1, 10.0), (1, 2, 1.0), (2, 3, 10.0), (3, 0, 1.0)],
        false,
    )
    .unwrap();
    let r = louvain(
        &g,
        &LouvainConfig {
            weighted: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(r.communities, vec![0, 0, 1, 1]);
    assert!(approx(
        r.modularity,
        modularity(&g, &r.communities, true).unwrap(),
        1e-12
    ));
}

#[test]
fn louvain_handles_loops_parallel_edges_and_directed_input() {
    let g = Projection::from_edges(
        5,
        &[
            (0, 1),
            (0, 1),
            (1, 0),
            (1, 2),
            (2, 2),
            (2, 0),
            (3, 4),
            (3, 4),
            (4, 4),
        ],
        true,
    )
    .unwrap();
    let r = louvain(&g, &LouvainConfig::default()).unwrap();
    assert_eq!(r.communities, vec![0, 0, 0, 1, 1]);
}

#[test]
fn louvain_edge_cases_and_validation() {
    let g = Projection::from_edges(0, &[], false).unwrap();
    let r = louvain(&g, &LouvainConfig::default()).unwrap();
    assert!(r.communities.is_empty());
    assert_eq!(r.modularity, 0.0);
    let g = Projection::from_edges(3, &[], false).unwrap();
    assert_eq!(
        louvain(&g, &LouvainConfig::default()).unwrap().communities,
        vec![0, 1, 2]
    );
    let g = Projection::from_edges(1, &[(0, 0)], false).unwrap();
    assert_eq!(
        louvain(&g, &LouvainConfig::default()).unwrap().communities,
        vec![0]
    );
    let bad = LouvainConfig {
        resolution: 0.0,
        ..Default::default()
    };
    assert!(matches!(
        louvain(&two_triangles(), &bad),
        Err(Error::InvalidParameter(_))
    ));
    let neg = Projection::from_weighted_edges(2, &[(0, 1, -1.0)], false).unwrap();
    let w = LouvainConfig {
        weighted: true,
        ..Default::default()
    };
    assert!(matches!(louvain(&neg, &w), Err(Error::InvalidParameter(_))));
}

#[test]
fn louvain_invariants_on_random_graphs() {
    let mut rng = XorShift64::new(321);
    for case in 0..100 {
        let n = 1 + rng.gen_range(40) as usize;
        let m = rng.gen_range(4 * n as u64) as usize;
        let edges = random_edges(&mut rng, n as u32, m, 0.5, 3.0);
        let g = Projection::from_weighted_edges(n, &edges, case % 2 == 0).unwrap();
        let r = louvain(&g, &LouvainConfig::default()).unwrap();
        assert_eq!(r.communities.len(), n);
        let k = r.communities.iter().max().map_or(0, |&x| x as usize + 1);
        for c in 0..k as u32 {
            assert!(r.communities.contains(&c), "labels must be dense");
        }
        let single: Vec<u32> = (0..n as u32).collect();
        assert!(
            approx(
                r.modularity,
                modularity(&g, &r.communities, false).unwrap(),
                1e-9
            ),
            "case {case}"
        );
        assert!(
            r.modularity >= modularity(&g, &single, false).unwrap() - 1e-9,
            "case {case}"
        );
        assert!(r.modularity >= -1e-9, "case {case}");
    }
}

#[test]
fn label_propagation_separates_disjoint_cliques() {
    let mut e = Vec::new();
    for base in [0u32, 6] {
        for i in 0..6 {
            for j in i + 1..6 {
                e.push((base + i, base + j));
            }
        }
    }
    let g = Projection::from_edges(12, &e, false).unwrap();
    let r = label_propagation(&g, &LabelPropagationConfig::default()).unwrap();
    assert!(r.converged);
    assert!((0..6).all(|i| r.labels[i] == r.labels[0]));
    assert!((6..12).all(|i| r.labels[i] == r.labels[6]));
    assert_ne!(r.labels[0], r.labels[6]);
}

#[test]
fn label_propagation_is_deterministic_and_keeps_isolated_nodes_apart() {
    let g = Projection::from_edges(5, &[(0, 1), (1, 2)], true).unwrap();
    let cfg = LabelPropagationConfig {
        seed: 9,
        ..Default::default()
    };
    let a = label_propagation(&g, &cfg).unwrap();
    assert_eq!(a, label_propagation(&g, &cfg).unwrap());
    assert_ne!(a.labels[3], a.labels[4]);
    assert_ne!(a.labels[3], a.labels[0]);
    let e = Projection::from_edges(0, &[], true).unwrap();
    assert!(label_propagation(&e, &cfg).unwrap().labels.is_empty());
    let bad = LabelPropagationConfig {
        max_iterations: 0,
        ..Default::default()
    };
    assert!(matches!(
        label_propagation(&g, &bad),
        Err(Error::InvalidParameter(_))
    ));
}

#[test]
fn label_propagation_never_crosses_weak_components() {
    let mut rng = XorShift64::new(88);
    for case in 0..60 {
        let n = 2 + rng.gen_range(40) as usize;
        let m = rng.gen_range(2 * n as u64) as usize;
        let edges: Vec<_> = random_edges(&mut rng, n as u32, m, 0.0, 1.0)
            .into_iter()
            .map(|(u, v, _)| (u, v))
            .collect();
        let g = Projection::from_edges(n, &edges, case % 2 == 0).unwrap();
        let wcc = weakly_connected_components(&g);
        let r = label_propagation(
            &g,
            &LabelPropagationConfig {
                seed: case,
                ..Default::default()
            },
        )
        .unwrap();
        for i in 0..n {
            for j in 0..n {
                if r.labels[i] == r.labels[j] {
                    assert_eq!(wcc[i], wcc[j], "case {case}: label spans components");
                }
            }
        }
        assert!(component_count(&r.labels) >= component_count(&wcc));
    }
}

#[test]
fn label_propagation_finds_structure_on_karate() {
    let g = karate();
    let good = (0..20u64)
        .filter(|&seed| {
            let r = label_propagation(
                &g,
                &LabelPropagationConfig {
                    seed,
                    ..Default::default()
                },
            )
            .unwrap();
            modularity(&g, &r.labels, false).unwrap() > 0.25
        })
        .count();
    assert!(good >= 14, "only {good}/20 seeds gave Q > 0.25");
}
