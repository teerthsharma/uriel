use uriel_algo::{bfs, dfs, Error, Projection};

mod common;

#[test]
fn bfs_levels_on_path_graph() {
    let g = Projection::from_edges(5, &[(0, 1), (1, 2), (2, 3), (3, 4)], false).unwrap();
    let t = bfs(&g, 2).unwrap();
    assert_eq!(t.order, vec![2, 1, 3, 0, 4]);
    assert_eq!(t.depth, vec![Some(2), Some(1), Some(0), Some(1), Some(2)]);
    assert_eq!(t.parent, vec![Some(1), Some(2), None, Some(2), Some(3)]);
}

#[test]
fn bfs_respects_direction() {
    let g = Projection::from_edges(3, &[(0, 1), (2, 1)], true).unwrap();
    let t = bfs(&g, 0).unwrap();
    assert_eq!(t.order, vec![0, 1]);
    assert_eq!(t.depth[2], None);
    assert_eq!(t.parent[2], None);
}

#[test]
fn bfs_handles_self_loops_parallel_edges_and_cycles() {
    let g = Projection::from_edges(3, &[(0, 0), (0, 1), (0, 1), (1, 2), (2, 0)], true).unwrap();
    let t = bfs(&g, 0).unwrap();
    assert_eq!(t.order, vec![0, 1, 2]);
    assert_eq!(t.depth, vec![Some(0), Some(1), Some(2)]);
}

#[test]
fn bfs_edge_cases() {
    let g = Projection::from_edges(1, &[], true).unwrap();
    let t = bfs(&g, 0).unwrap();
    assert_eq!(t.order, vec![0]);
    let g = Projection::from_edges(0, &[], true).unwrap();
    assert_eq!(
        bfs(&g, 0).unwrap_err(),
        Error::NodeOutOfRange {
            node: 0,
            node_count: 0
        }
    );
    let g = Projection::from_edges(4, &[(0, 1)], false).unwrap();
    let t = bfs(&g, 0).unwrap();
    assert_eq!(t.order, vec![0, 1]);
    assert_eq!(t.depth, vec![Some(0), Some(1), None, None]);
}

#[test]
fn dfs_preorder_follows_ascending_adjacency() {
    let g = Projection::from_edges(5, &[(0, 1), (0, 2), (1, 3), (2, 3), (3, 4)], true).unwrap();
    let t = dfs(&g, 0).unwrap();
    assert_eq!(t.order, vec![0, 1, 3, 4, 2]);
    assert_eq!(t.parent, vec![None, Some(0), Some(0), Some(1), Some(3)]);
    assert_eq!(t.depth, vec![Some(0), Some(1), Some(1), Some(2), Some(3)]);
}

#[test]
fn dfs_visits_exactly_the_bfs_reachable_set() {
    let mut rng = uriel_algo::XorShift64::new(3);
    for i in 0..50 {
        let edges: Vec<(u32, u32)> = common::random_edges(&mut rng, 40, 60, 0.0, 1.0)
            .into_iter()
            .map(|(u, v, _)| (u, v))
            .collect();
        let g = Projection::from_edges(40, &edges, i % 2 == 0).unwrap();
        let b = bfs(&g, 0).unwrap();
        let d = dfs(&g, 0).unwrap();
        let mut bo = b.order.clone();
        let mut dord = d.order.clone();
        bo.sort_unstable();
        dord.sort_unstable();
        assert_eq!(bo, dord);
        assert_eq!(
            b.depth.iter().map(Option::is_some).collect::<Vec<_>>(),
            d.depth.iter().map(Option::is_some).collect::<Vec<_>>()
        );
    }
}

#[test]
fn dfs_survives_a_200k_deep_path() {
    let n = 200_000u32;
    let edges: Vec<(u32, u32)> = (0..n - 1).map(|i| (i, i + 1)).collect();
    let g = Projection::from_edges(n as usize, &edges, true).unwrap();
    let t = dfs(&g, 0).unwrap();
    assert_eq!(t.order.len(), n as usize);
    assert_eq!(t.depth[n as usize - 1], Some(n - 1));
}

#[test]
fn dfs_rejects_bad_source() {
    let g = Projection::from_edges(2, &[(0, 1)], true).unwrap();
    assert!(matches!(dfs(&g, 9), Err(Error::NodeOutOfRange { .. })));
}
