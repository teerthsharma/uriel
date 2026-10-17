use uriel_algo::{
    bucket_islands, component_sizes, island_labels, weakly_connected_components, Projection,
    XorShift64,
};

mod common;
use common::random_edges;

fn pairs(edges: Vec<(u32, u32, f64)>) -> Vec<(u32, u32)> {
    edges.into_iter().map(|(u, v, _)| (u, v)).collect()
}

#[test]
fn island_labels_empty_graph_is_empty() {
    let g = Projection::from_edges(0, &[], true).unwrap();
    assert!(island_labels(&g).is_empty());
}

#[test]
fn island_labels_single_node_is_its_own_island() {
    let g = Projection::from_edges(1, &[], false).unwrap();
    assert_eq!(island_labels(&g), vec![0]);
}

#[test]
fn island_labels_self_loops_do_not_merge_nodes() {
    let g = Projection::from_edges(3, &[(0, 0), (1, 1)], true).unwrap();
    assert_eq!(island_labels(&g), vec![0, 1, 2]);
}

#[test]
fn island_labels_two_triangles_and_an_isolated_node_use_min_id() {
    let g = Projection::from_edges(7, &[(5, 3), (4, 5), (3, 4), (2, 0), (1, 2), (0, 1)], false)
        .unwrap();
    assert_eq!(island_labels(&g), vec![0, 0, 0, 3, 3, 3, 6]);
}

#[test]
fn island_labels_ignore_edge_direction() {
    let g = Projection::from_edges(5, &[(4, 3), (3, 1), (2, 2)], true).unwrap();
    assert_eq!(island_labels(&g), vec![0, 1, 2, 1, 1]);
}

#[test]
fn island_labels_equal_weakly_connected_components_on_2500_random_graphs() {
    let mut rng = XorShift64::new(3396);
    for case in 0..2500 {
        let n = 1 + rng.gen_range(80) as u32;
        let m = rng.gen_range(2 * n as u64 + 1) as usize;
        let edges = pairs(random_edges(&mut rng, n, m, 0.0, 1.0));
        let g = Projection::from_edges(n as usize, &edges, case % 2 == 0).unwrap();
        assert_eq!(
            island_labels(&g),
            weakly_connected_components(&g),
            "case {case}"
        );
    }
}

#[test]
fn island_labels_are_independent_of_edge_insertion_order_across_20_shuffles() {
    let mut rng = XorShift64::new(1541);
    for directed in [false, true] {
        let n = 120u32;
        let edges = pairs(random_edges(&mut rng, n, 110, 0.0, 1.0));
        let reference =
            island_labels(&Projection::from_edges(n as usize, &edges, directed).unwrap());
        for round in 0..20 {
            let mut shuffled = edges.clone();
            rng.shuffle(&mut shuffled);
            let g = Projection::from_edges(n as usize, &shuffled, directed).unwrap();
            assert_eq!(island_labels(&g), reference, "round {round}");
        }
    }
}

fn mixed_size_island_labels() -> Vec<u32> {
    let g = Projection::from_edges(
        14,
        &[
            (1, 2),
            (3, 4),
            (4, 5),
            (6, 7),
            (7, 8),
            (8, 9),
            (9, 10),
            (11, 12),
            (12, 13),
            (13, 11),
        ],
        false,
    )
    .unwrap();
    island_labels(&g)
}

#[test]
fn bucket_islands_places_each_island_in_first_bucket_whose_bound_covers_it() {
    let labels = mixed_size_island_labels();
    assert_eq!(
        component_sizes(&labels),
        vec![(0, 1), (1, 2), (3, 3), (6, 5), (11, 3)]
    );
    let buckets = bucket_islands(&labels, &[1, 3]);
    assert_eq!(buckets, vec![vec![0], vec![1, 3, 11], vec![6]]);
}

#[test]
fn bucket_islands_last_bucket_is_open_ended() {
    let labels = mixed_size_island_labels();
    let buckets = bucket_islands(&labels, &[1]);
    assert_eq!(buckets, vec![vec![0], vec![1, 3, 6, 11]]);
}

#[test]
fn bucket_islands_without_bounds_is_one_bucket_in_ascending_label_order() {
    let labels = mixed_size_island_labels();
    assert_eq!(bucket_islands(&labels, &[]), vec![vec![0, 1, 3, 6, 11]]);
}

#[test]
fn bucket_islands_bound_equal_to_size_is_inclusive() {
    let labels = vec![0, 0, 0];
    assert_eq!(
        bucket_islands(&labels, &[2, 3]),
        vec![vec![], vec![0], vec![]]
    );
    assert_eq!(bucket_islands(&labels, &[2]), vec![vec![], vec![0]]);
}

#[test]
fn bucket_islands_empty_labels_give_all_empty_buckets() {
    assert_eq!(bucket_islands(&[], &[4, 8]), vec![vec![]; 3]);
}

#[test]
fn bucket_islands_put_every_node_in_exactly_one_island_and_one_bucket() {
    let mut rng = XorShift64::new(7);
    for case in 0..300 {
        let n = 1 + rng.gen_range(120) as u32;
        let m = rng.gen_range(n as u64 + 1) as usize;
        let edges = pairs(random_edges(&mut rng, n, m, 0.0, 1.0));
        let g = Projection::from_edges(n as usize, &edges, case % 2 == 0).unwrap();
        let labels = island_labels(&g);
        let buckets = bucket_islands(&labels, &[1, 2, 5, 20]);
        assert_eq!(buckets.len(), 5);
        let sizes = component_sizes(&labels);
        let mut seen = vec![0u32; n as usize];
        let mut covered = 0usize;
        for bucket in &buckets {
            assert!(bucket.windows(2).all(|w| w[0] < w[1]), "case {case}");
            for &label in bucket {
                seen[label as usize] += 1;
                covered += sizes.iter().find(|&&(l, _)| l == label).unwrap().1;
            }
        }
        assert_eq!(covered, n as usize, "case {case}");
        for &(label, _) in &sizes {
            assert_eq!(seen[label as usize], 1, "case {case}");
        }
        assert_eq!(
            seen.iter().sum::<u32>() as usize,
            sizes.len(),
            "case {case}"
        );
    }
}
