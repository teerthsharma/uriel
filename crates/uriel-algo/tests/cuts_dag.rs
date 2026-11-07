use uriel_algo::{
    articulation_points, bridges, condensation_layers, feedback_arc_set, strongly_connected_components,
    topological_sort, Error, Projection, XorShift64,
};

mod common;
use common::{karate, random_edges};

fn undirected(n: usize, e: &[(u32, u32)]) -> Projection {
    Projection::from_edges(n, e, false).unwrap()
}

fn directed(n: usize, e: &[(u32, u32)]) -> Projection {
    Projection::from_edges(n, e, true).unwrap()
}

#[test]
fn path_graph_has_inner_nodes_as_cut_vertices_and_every_edge_as_bridge() {
    let g = undirected(4, &[(0, 1), (1, 2), (2, 3)]);
    assert_eq!(articulation_points(&g), vec![1, 2]);
    assert_eq!(bridges(&g), vec![(0, 1), (1, 2), (2, 3)]);
}

#[test]
fn barbell_of_two_triangles_cuts_at_the_joining_edge() {
    let g = undirected(6, &[(0, 1), (1, 2), (0, 2), (3, 4), (4, 5), (3, 5), (2, 3)]);
    assert_eq!(articulation_points(&g), vec![2, 3]);
    assert_eq!(bridges(&g), vec![(2, 3)]);
}

#[test]
fn a_cycle_has_no_cut_vertex_and_no_bridge() {
    let g = undirected(5, &[(0, 1), (1, 2), (2, 3), (3, 4), (4, 0)]);
    assert!(articulation_points(&g).is_empty());
    assert!(bridges(&g).is_empty());
}

#[test]
fn star_centre_is_the_only_cut_vertex() {
    let g = undirected(5, &[(0, 1), (0, 2), (0, 3), (0, 4)]);
    assert_eq!(articulation_points(&g), vec![0]);
    assert_eq!(bridges(&g).len(), 4);
}

#[test]
fn parallel_arcs_and_loops_collapse_and_direction_is_ignored() {
    let g = directed(3, &[(0, 1), (1, 0), (1, 1), (1, 2)]);
    assert_eq!(articulation_points(&g), vec![1]);
    assert_eq!(bridges(&g), vec![(0, 1), (1, 2)]);
}

#[test]
fn karate_has_node_zero_as_its_only_cut_vertex_and_one_bridge() {
    let g = karate();
    assert_eq!(articulation_points(&g), vec![0]);
    assert_eq!(bridges(&g), vec![(0, 11)]);
}

#[test]
fn empty_and_edgeless_graphs_have_no_cuts() {
    assert!(articulation_points(&undirected(0, &[])).is_empty());
    assert!(bridges(&undirected(3, &[])).is_empty());
}

#[test]
fn random_graphs_match_brute_force_removal() {
    let mut rng = XorShift64::new(7);
    for round in 0..40 {
        let n = 4 + (round % 9) as u32;
        let m = 3 + (round % 14) as usize;
        let edges: Vec<(u32, u32)> = random_edges(&mut rng, n, m, 1.0, 2.0)
            .into_iter()
            .map(|(u, v, _)| (u, v))
            .collect();
        let g = undirected(n as usize, &edges);
        let (aps, br) = brute_cuts(n as usize, &edges);
        assert_eq!(articulation_points(&g), aps, "round {round} {edges:?}");
        assert_eq!(bridges(&g), br, "round {round} {edges:?}");
    }
}

fn brute_cuts(n: usize, edges: &[(u32, u32)]) -> (Vec<u32>, Vec<(u32, u32)>) {
    let simple: std::collections::BTreeSet<(u32, u32)> = edges
        .iter()
        .filter(|(u, v)| u != v)
        .map(|&(u, v)| (u.min(v), u.max(v)))
        .collect();
    let simple: Vec<(u32, u32)> = simple.into_iter().collect();
    let count = |skip_node: Option<u32>, skip_edge: Option<(u32, u32)>| -> usize {
        let mut parent: Vec<usize> = (0..n).collect();
        fn find(p: &[usize], x: usize) -> usize {
            let mut r = x;
            while p[r] != r {
                r = p[r];
            }
            r
        }
        for &(u, v) in &simple {
            if Some(u) == skip_node || Some(v) == skip_node || Some((u, v)) == skip_edge {
                continue;
            }
            let (a, b) = (find(&parent, u as usize), find(&parent, v as usize));
            if a != b {
                parent[a] = b;
            }
        }
        (0..n)
            .filter(|&x| Some(x as u32) != skip_node)
            .map(|x| find(&parent, x))
            .collect::<std::collections::BTreeSet<_>>()
            .len()
    };
    let base = count(None, None);
    let aps = (0..n as u32).filter(|&x| count(Some(x), None) > base).collect();
    let br = simple.iter().copied().filter(|&e| count(None, Some(e)) > base).collect();
    (aps, br)
}

#[test]
fn feedback_arc_set_of_a_dag_is_empty() {
    let g = directed(4, &[(0, 1), (1, 2), (0, 2), (2, 3)]);
    assert!(feedback_arc_set(&g).unwrap().is_empty());
}

#[test]
fn feedback_arc_set_cuts_one_arc_of_a_triangle_and_every_self_loop() {
    let g = directed(5, &[(0, 1), (1, 2), (2, 0), (3, 3), (3, 4)]);
    let fas = feedback_arc_set(&g).unwrap();
    assert_eq!(fas.len(), 2, "{fas:?}");
    assert!(fas.contains(&(3, 3)));
    assert!(fas.iter().any(|a| [(0, 1), (1, 2), (2, 0)].contains(a)));
}

#[test]
fn feedback_arc_set_of_a_two_cycle_is_one_arc() {
    let g = directed(2, &[(0, 1), (1, 0)]);
    assert_eq!(feedback_arc_set(&g).unwrap().len(), 1);
}

#[test]
fn feedback_arc_set_needs_one_arc_per_disjoint_cycle() {
    let g = directed(6, &[(0, 1), (1, 2), (2, 0), (3, 4), (4, 5), (5, 3)]);
    assert_eq!(feedback_arc_set(&g).unwrap().len(), 2);
}

#[test]
fn feedback_arc_set_rejects_undirected_projections() {
    assert_eq!(
        feedback_arc_set(&undirected(2, &[(0, 1)])),
        Err(Error::RequiresDirected)
    );
}

#[test]
fn feedback_arc_set_is_a_minimal_cut_on_random_digraphs_and_deterministic() {
    let mut rng = XorShift64::new(11);
    for round in 0..30 {
        let n = 5 + (round % 12) as u32;
        let m = 6 + (round * 3 % 40) as usize;
        let edges: Vec<(u32, u32)> = random_edges(&mut rng, n, m, 1.0, 2.0)
            .into_iter()
            .map(|(u, v, _)| (u, v))
            .collect();
        let g = directed(n as usize, &edges);
        let fas = feedback_arc_set(&g).unwrap();
        assert_eq!(fas, feedback_arc_set(&g).unwrap());
        let rest = |without: &[(u32, u32)]| -> Projection {
            let kept: Vec<(u32, u32)> = edges
                .iter()
                .copied()
                .filter(|e| !without.contains(e))
                .collect();
            directed(n as usize, &kept)
        };
        assert!(topological_sort(&rest(&fas)).is_ok(), "round {round}: still cyclic {edges:?} {fas:?}");
        for skip in 0..fas.len() {
            let mut partial = fas.clone();
            partial.remove(skip);
            assert!(
                topological_sort(&rest(&partial)).is_err(),
                "round {round}: arc {:?} is redundant in {fas:?} for {edges:?}",
                fas[skip]
            );
        }
    }
}

#[test]
fn condensation_layers_follow_the_longest_path_over_strong_components() {
    let g = directed(6, &[(4, 0), (0, 1), (1, 2), (2, 0), (2, 3), (4, 3)]);
    let layers = condensation_layers(&g).unwrap();
    assert_eq!(layers, vec![1, 1, 1, 2, 0, 0]);
    let scc = strongly_connected_components(&g);
    assert_eq!(scc[0], scc[2]);
}

#[test]
fn condensation_layers_of_a_chain_count_up() {
    let g = directed(4, &[(0, 1), (1, 2), (2, 3)]);
    assert_eq!(condensation_layers(&g).unwrap(), vec![0, 1, 2, 3]);
}

#[test]
fn condensation_layers_reject_undirected_projections() {
    assert_eq!(
        condensation_layers(&undirected(2, &[(0, 1)])),
        Err(Error::RequiresDirected)
    );
}
