#![allow(clippy::needless_range_loop)]
use uriel_algo::{
    betweenness_centrality, closeness_centrality, degree_centrality, pagerank,
    personalized_pagerank, BetweennessConfig, Direction, Error, PageRankConfig, Projection,
    XorShift64,
};

mod common;
use common::{
    approx, assert_vec_approx, karate, random_edges, KARATE_BETWEENNESS, KARATE_CLOSENESS,
    KARATE_PAGERANK,
};

fn tight() -> PageRankConfig {
    PageRankConfig {
        tolerance: 1e-13,
        max_iterations: 1000,
        ..PageRankConfig::default()
    }
}

#[test]
fn pagerank_defaults_match_gds() {
    let c = PageRankConfig::default();
    assert_eq!(
        (c.damping, c.tolerance, c.max_iterations, c.weighted),
        (0.85, 1e-7, 100, false)
    );
}

#[test]
fn pagerank_star_matches_closed_form() {
    // centre 0, leaves 1..=3: c = a(1+3d)/(1-d^2), leaf = (1-c)/3 with a = (1-d)/4.
    let g = Projection::from_edges(4, &[(0, 1), (0, 2), (0, 3)], false).unwrap();
    let r = pagerank(&g, &tight()).unwrap();
    let d: f64 = 0.85;
    let a = (1.0 - d) / 4.0;
    let centre = a * (1.0 + 3.0 * d) / (1.0 - d * d);
    let leaf = (1.0 - centre) / 3.0;
    assert_vec_approx(&r.scores, &[centre, leaf, leaf, leaf], 1e-10);
    assert!(r.converged);
}

#[test]
fn pagerank_dangling_node_redistributes_uniformly() {
    // 0 -> 1, node 1 dangling. p0 = 0.5 / (1 + 0.425 ... ) solved by hand: p0 = 0.5/1.425.
    let g = Projection::from_edges(2, &[(0, 1)], true).unwrap();
    let r = pagerank(&g, &tight()).unwrap();
    let p0 = 0.5 / 1.425;
    assert_vec_approx(&r.scores, &[p0, 1.0 - p0], 1e-10);
}

#[test]
fn pagerank_cycle_is_uniform_and_matches_karate_reference() {
    let g = Projection::from_edges(4, &[(0, 1), (1, 2), (2, 3), (3, 0)], true).unwrap();
    assert_vec_approx(&pagerank(&g, &tight()).unwrap().scores, &[0.25; 4], 1e-10);
    let r = pagerank(&karate(), &tight()).unwrap();
    assert_vec_approx(&r.scores, &KARATE_PAGERANK, 1e-9);
}

#[test]
fn personalized_pagerank_on_directed_cycle_matches_closed_form() {
    // p0 = (1-d)/(1-d^3), p1 = d p0, p2 = d^2 p0.
    let g = Projection::from_edges(3, &[(0, 1), (1, 2), (2, 0)], true).unwrap();
    let r = personalized_pagerank(&g, &[0], &tight()).unwrap();
    let d: f64 = 0.85;
    let p0 = (1.0 - d) / (1.0 - d.powi(3));
    assert_vec_approx(&r.scores, &[p0, d * p0, d * d * p0], 1e-10);
}

#[test]
fn personalized_pagerank_concentrates_mass_near_the_source() {
    let r = personalized_pagerank(&karate(), &[0], &tight()).unwrap();
    assert!(approx(r.scores.iter().sum::<f64>(), 1.0, 1e-9));
    let uniform = pagerank(&karate(), &tight()).unwrap();
    assert!(r.scores[0] > uniform.scores[0]);
    assert!(r.scores[0] > r.scores[33]);
    // two sources split the teleport mass
    let two = personalized_pagerank(&karate(), &[0, 33], &tight()).unwrap();
    assert!(two.scores[33] > r.scores[33]);
}

#[test]
fn pagerank_weighted_follows_weights() {
    // 0 -> 1 (w 3), 0 -> 2 (w 1), both return to 0.
    let g = Projection::from_weighted_edges(
        3,
        &[(0, 1, 3.0), (0, 2, 1.0), (1, 0, 1.0), (2, 0, 1.0)],
        true,
    )
    .unwrap();
    let cfg = PageRankConfig {
        weighted: true,
        ..tight()
    };
    let r = pagerank(&g, &cfg).unwrap();
    // exact: node 1 and 2 only receive from 0 plus teleport (1-d)/3
    let d = 0.85;
    let t = (1.0 - d) / 3.0;
    assert!(approx(r.scores[1], t + d * 0.75 * r.scores[0], 1e-9));
    assert!(approx(r.scores[2], t + d * 0.25 * r.scores[0], 1e-9));
    let unweighted = pagerank(&g, &tight()).unwrap();
    assert!(approx(unweighted.scores[1], unweighted.scores[2], 1e-9));
}

#[test]
fn pagerank_sums_to_one_on_random_graphs_with_dangling_nodes_and_loops() {
    let mut rng = XorShift64::new(1234);
    for case in 0..100 {
        let n = 1 + rng.gen_range(50) as u32;
        let m = rng.gen_range(3 * n as u64) as usize;
        let edges = random_edges(&mut rng, n, m, 0.1, 5.0);
        let g = Projection::from_weighted_edges(n as usize, &edges, case % 2 == 0).unwrap();
        for weighted in [false, true] {
            let cfg = PageRankConfig {
                weighted,
                ..PageRankConfig::default()
            };
            let r = pagerank(&g, &cfg).unwrap();
            assert!(
                approx(r.scores.iter().sum::<f64>(), 1.0, 1e-6),
                "case {case}"
            );
            assert!(r.scores.iter().all(|&x| x > 0.0));
            let s = rng.gen_range(n as u64) as u32;
            let p = personalized_pagerank(&g, &[s], &cfg).unwrap();
            assert!(
                approx(p.scores.iter().sum::<f64>(), 1.0, 1e-6),
                "case {case} personalized"
            );
        }
    }
}

#[test]
fn pagerank_reports_non_convergence_and_validates_input() {
    let r = pagerank(
        &karate(),
        &PageRankConfig {
            max_iterations: 2,
            tolerance: 1e-15,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(r.iterations, 2);
    assert!(!r.converged);
    let g = karate();
    for bad in [-0.1, 1.0, f64::NAN] {
        let cfg = PageRankConfig {
            damping: bad,
            ..Default::default()
        };
        assert!(matches!(
            pagerank(&g, &cfg),
            Err(Error::InvalidParameter(_))
        ));
    }
    assert!(matches!(
        personalized_pagerank(&g, &[], &Default::default()),
        Err(Error::InvalidParameter(_))
    ));
    assert!(matches!(
        personalized_pagerank(&g, &[99], &Default::default()),
        Err(Error::NodeOutOfRange { .. })
    ));
}

#[test]
fn pagerank_edge_cases() {
    let g = Projection::from_edges(0, &[], true).unwrap();
    assert!(pagerank(&g, &Default::default()).unwrap().scores.is_empty());
    let g = Projection::from_edges(1, &[], true).unwrap();
    assert_vec_approx(
        &pagerank(&g, &Default::default()).unwrap().scores,
        &[1.0],
        1e-12,
    );
    let g = Projection::from_edges(3, &[], true).unwrap();
    assert_vec_approx(
        &pagerank(&g, &Default::default()).unwrap().scores,
        &[1.0 / 3.0; 3],
        1e-12,
    );
}

#[test]
fn degree_centrality_directed() {
    let g = Projection::from_edges(3, &[(0, 1), (0, 2), (1, 2), (2, 2), (0, 1)], true).unwrap();
    assert_eq!(
        degree_centrality(&g, Direction::Out, false),
        vec![3.0, 1.0, 1.0]
    );
    assert_eq!(
        degree_centrality(&g, Direction::In, false),
        vec![0.0, 2.0, 3.0]
    );
    assert_eq!(
        degree_centrality(&g, Direction::Both, false),
        vec![3.0, 3.0, 4.0]
    );
}

#[test]
fn degree_centrality_weighted_and_undirected_and_karate() {
    let g = Projection::from_weighted_edges(3, &[(0, 1, 2.5), (0, 2, 1.5)], true).unwrap();
    assert_eq!(
        degree_centrality(&g, Direction::Out, true),
        vec![4.0, 0.0, 0.0]
    );
    assert_eq!(
        degree_centrality(&g, Direction::Out, false),
        vec![2.0, 0.0, 0.0]
    );
    let u = Projection::from_edges(3, &[(0, 1), (0, 2)], false).unwrap();
    assert_eq!(
        degree_centrality(&u, Direction::Both, false),
        vec![2.0, 1.0, 1.0]
    );
    assert_eq!(
        degree_centrality(&u, Direction::In, false),
        vec![2.0, 1.0, 1.0]
    );
    let k = degree_centrality(&karate(), Direction::Out, false);
    assert_eq!((k[0], k[33], k[11]), (16.0, 17.0, 1.0));
    assert!(degree_centrality(
        &Projection::from_edges(0, &[], true).unwrap(),
        Direction::Out,
        false
    )
    .is_empty());
}

fn exact() -> BetweennessConfig {
    BetweennessConfig::default()
}

#[test]
fn betweenness_karate_matches_networkx() {
    let r = betweenness_centrality(&karate(), &exact()).unwrap();
    assert_vec_approx(&r, &KARATE_BETWEENNESS, 1e-8);
}

#[test]
fn betweenness_normalisation_matches_networkx() {
    let cfg = BetweennessConfig {
        normalize: true,
        ..exact()
    };
    let r = betweenness_centrality(&karate(), &cfg).unwrap();
    assert!(approx(r[0], 0.43763528138528146, 1e-9));
    assert!(approx(r[33], KARATE_BETWEENNESS[33] / 528.0, 1e-9));
}

#[test]
fn betweenness_textbook_small_graphs() {
    let path = Projection::from_edges(5, &[(0, 1), (1, 2), (2, 3), (3, 4)], false).unwrap();
    assert_vec_approx(
        &betweenness_centrality(&path, &exact()).unwrap(),
        &[0.0, 3.0, 4.0, 3.0, 0.0],
        1e-12,
    );
    let star = Projection::from_edges(5, &[(0, 1), (0, 2), (0, 3), (0, 4)], false).unwrap();
    assert_vec_approx(
        &betweenness_centrality(&star, &exact()).unwrap(),
        &[6.0, 0.0, 0.0, 0.0, 0.0],
        1e-12,
    );
    let cycle4 = Projection::from_edges(4, &[(0, 1), (1, 2), (2, 3), (3, 0)], false).unwrap();
    assert_vec_approx(
        &betweenness_centrality(&cycle4, &exact()).unwrap(),
        &[0.5; 4],
        1e-12,
    );
    let dpath = Projection::from_edges(3, &[(0, 1), (1, 2)], true).unwrap();
    assert_vec_approx(
        &betweenness_centrality(&dpath, &exact()).unwrap(),
        &[0.0, 1.0, 0.0],
        1e-12,
    );
    let dcycle = Projection::from_edges(4, &[(0, 1), (1, 2), (2, 3), (3, 0)], true).unwrap();
    assert_vec_approx(
        &betweenness_centrality(&dcycle, &exact()).unwrap(),
        &[3.0; 4],
        1e-12,
    );
}

#[test]
fn betweenness_treats_parallel_edges_and_self_loops_as_a_simple_graph() {
    let g = Projection::from_edges(4, &[(0, 1), (0, 1), (1, 2), (2, 3), (3, 0), (2, 2)], false)
        .unwrap();
    assert_vec_approx(
        &betweenness_centrality(&g, &exact()).unwrap(),
        &[0.5; 4],
        1e-12,
    );
}

#[test]
fn betweenness_edge_cases() {
    for n in 0..3usize {
        let g = Projection::from_edges(n, &[], false).unwrap();
        let cfg = BetweennessConfig {
            normalize: true,
            ..exact()
        };
        let r = betweenness_centrality(&g, &cfg).unwrap();
        assert_eq!(r, vec![0.0; n]);
    }
    let two = Projection::from_edges(2, &[(0, 1)], false).unwrap();
    assert_eq!(
        betweenness_centrality(
            &two,
            &BetweennessConfig {
                normalize: true,
                ..exact()
            }
        )
        .unwrap(),
        vec![0.0, 0.0]
    );
    let cfg = BetweennessConfig {
        sample_size: Some(0),
        ..exact()
    };
    assert!(matches!(
        betweenness_centrality(&karate(), &cfg),
        Err(Error::InvalidParameter(_))
    ));
}

#[test]
fn betweenness_sampling_is_deterministic_exact_at_full_size_and_unbiased() {
    let g = karate();
    let full = BetweennessConfig {
        sample_size: Some(34),
        ..exact()
    };
    assert_vec_approx(
        &betweenness_centrality(&g, &full).unwrap(),
        &KARATE_BETWEENNESS,
        1e-8,
    );
    let over = BetweennessConfig {
        sample_size: Some(500),
        ..exact()
    };
    assert_vec_approx(
        &betweenness_centrality(&g, &over).unwrap(),
        &KARATE_BETWEENNESS,
        1e-8,
    );

    let cfg = |seed| BetweennessConfig {
        sample_size: Some(12),
        seed,
        ..exact()
    };
    let a = betweenness_centrality(&g, &cfg(7)).unwrap();
    assert_eq!(a, betweenness_centrality(&g, &cfg(7)).unwrap());
    assert_ne!(a, betweenness_centrality(&g, &cfg(8)).unwrap());

    let runs = 400u64;
    let mean: f64 = (0..runs)
        .map(|s| betweenness_centrality(&g, &cfg(s)).unwrap()[0])
        .sum::<f64>()
        / runs as f64;
    assert!(
        (mean - KARATE_BETWEENNESS[0]).abs() / KARATE_BETWEENNESS[0] < 0.06,
        "mean {mean}"
    );
}

#[test]
fn closeness_karate_matches_networkx() {
    assert_vec_approx(
        &closeness_centrality(&karate(), true),
        &KARATE_CLOSENESS,
        1e-9,
    );
    assert_vec_approx(
        &closeness_centrality(&karate(), false),
        &KARATE_CLOSENESS,
        1e-9,
    );
}

#[test]
fn closeness_disconnected_wasserman_faust() {
    let g = Projection::from_edges(4, &[(0, 1), (1, 2)], false).unwrap();
    let wf = closeness_centrality(&g, true);
    assert_vec_approx(&wf, &[4.0 / 9.0, 2.0 / 3.0, 4.0 / 9.0, 0.0], 1e-12);
    let plain = closeness_centrality(&g, false);
    assert_vec_approx(&plain, &[2.0 / 3.0, 1.0, 2.0 / 3.0, 0.0], 1e-12);
}

#[test]
fn closeness_follows_out_arcs_on_directed_graphs() {
    let g = Projection::from_edges(3, &[(0, 1), (1, 2)], true).unwrap();
    assert_vec_approx(
        &closeness_centrality(&g, true),
        &[2.0 / 3.0, 0.5, 0.0],
        1e-12,
    );
    assert!(closeness_centrality(&Projection::from_edges(0, &[], true).unwrap(), true).is_empty());
    assert_eq!(
        closeness_centrality(&Projection::from_edges(1, &[], true).unwrap(), true),
        vec![0.0]
    );
}

#[test]
fn closeness_ignores_self_loops_and_parallel_edges() {
    let g = Projection::from_edges(3, &[(0, 1), (0, 1), (1, 2), (1, 1)], false).unwrap();
    assert_vec_approx(
        &closeness_centrality(&g, true),
        &[2.0 / 3.0, 1.0, 2.0 / 3.0],
        1e-12,
    );
}
