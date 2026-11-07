mod common;

use std::collections::BTreeSet;

use common::Session;
use serde_json::{json, Value};
use uriel_algo::{louvain, LouvainConfig, Projection};

const GRAPH_TOOLS: [&str; 6] = [
    "graph_rank",
    "graph_path",
    "graph_impact",
    "graph_cycles",
    "graph_communities",
    "graph_lint",
];

const KARATE: [(usize, usize); 78] = [(0,1), (0,2), (0,3), (0,4), (0,5), (0,6), (0,7), (0,8), (0,10), (0,11), (0,12), (0,13), (0,17), (0,19), (0,21), (0,31), (1,2), (1,3), (1,7), (1,13), (1,17), (1,19), (1,21), (1,30), (2,3), (2,7), (2,8), (2,9), (2,13), (2,27), (2,28), (2,32), (3,7), (3,12), (3,13), (4,6), (4,10), (5,6), (5,10), (5,16), (6,16), (8,30), (8,32), (8,33), (9,33), (13,33), (14,32), (14,33), (15,32), (15,33), (18,32), (18,33), (19,33), (20,32), (20,33), (22,32), (22,33), (23,25), (23,27), (23,29), (23,32), (23,33), (24,25), (24,27), (24,31), (25,31), (26,29), (26,33), (27,33), (28,31), (28,33), (29,32), (29,33), (30,32), (30,33), (31,32), (31,33), (32,33)];

fn start() -> Session {
    Session::start(&["--in-memory".to_string()], &[])
}

fn create(s: &mut Session, nodes: &[(&str, &str)], rels: &[(usize, usize, &str)]) {
    let mut parts: Vec<String> = nodes
        .iter()
        .enumerate()
        .map(|(i, (label, props))| format!("(n{i}:{label} {{{props}}})"))
        .collect();
    parts.extend(rels.iter().map(|(a, b, spec)| format!("(n{a})-[:{spec}]->(n{b})")));
    s.text("write_neo4j_cypher", json!({"query": format!("CREATE {}", parts.join(", "))}));
}

fn named(prefix: &str, n: usize) -> Vec<String> {
    (0..n).map(|i| format!("name: '{prefix}{i}'")).collect()
}

fn nodes_of<'a>(label: &'a str, props: &'a [String]) -> Vec<(&'a str, &'a str)> {
    props.iter().map(|p| (label, p.as_str())).collect()
}

fn call(s: &mut Session, tool: &str, mut args: Value) -> Value {
    if let Some(o) = args.as_object_mut() {
        o.entry("format").or_insert_with(|| json!("json"));
    }
    let text = s.text(tool, args);
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{tool} returned non-JSON ({e}): {text}"))
}

fn fails(s: &mut Session, tool: &str, args: Value) -> Value {
    let text = s.err(tool, args);
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{tool} error is not JSON ({e}): {text}"))
}

fn col(v: &Value, tab: &str, key: &str) -> Vec<Value> {
    v[tab]
        .as_array()
        .unwrap_or_else(|| panic!("no table {tab} in {v}"))
        .iter()
        .map(|r| r[key].clone())
        .collect()
}

fn strs(v: &Value, tab: &str, key: &str) -> Vec<String> {
    col(v, tab, key)
        .iter()
        .map(|x| x.as_str().unwrap_or_default().to_string())
        .collect()
}

fn id_of(s: &mut Session, name: &str) -> u64 {
    let rows = s.ok(
        "read_neo4j_cypher",
        json!({"query": "MATCH (n) WHERE n.name = $name RETURN id(n) AS id", "params": {"name": name}}),
    );
    rows[0]["id"].as_u64().unwrap_or_else(|| panic!("no node named {name}"))
}

fn by_name(label: &str, name: &str) -> Value {
    json!({"label": label, "key": "name", "value": name})
}

fn karate(s: &mut Session) {
    let props = named("n", 34);
    let nodes = nodes_of("P", &props);
    let rels: Vec<(usize, usize, &str)> = KARATE.iter().map(|&(a, b)| (a, b, "K")).collect();
    create(s, &nodes, &rels);
}

fn barbell(s: &mut Session) {
    let props = named("k", 8);
    let nodes = nodes_of("B", &props);
    let mut rels: Vec<(usize, usize, &str)> = Vec::new();
    for base in [0usize, 4] {
        for a in 0..4 {
            for b in (a + 1)..4 {
                rels.push((base + a, base + b, "E"));
            }
        }
    }
    rels.push((3, 4, "E"));
    create(s, &nodes, &rels);
}

fn chain(s: &mut Session) {
    let props = named("n", 6);
    let nodes = nodes_of("P", &props);
    let mut rels: Vec<(usize, usize, &str)> = (0..5).map(|i| (i, i + 1, "NEXT {w: 1}")).collect();
    rels.push((0, 3, "SKIP {w: 10}"));
    create(s, &nodes, &rels);
}

#[test]
fn the_six_tools_are_read_only_listed_in_both_modes_and_in_read_only_servers() {
    for extra in [vec![], vec!["--read-only"], vec!["--mode", "house"]] {
        let mut args = vec!["--in-memory".to_string()];
        args.extend(extra.iter().map(|s| s.to_string()));
        let mut s = Session::start(&args, &[]);
        let list = s.rpc("tools/list", json!({}));
        let tools = list["result"]["tools"].as_array().expect("tools");
        for name in GRAPH_TOOLS {
            let t = tools
                .iter()
                .find(|t| t["name"] == name)
                .unwrap_or_else(|| panic!("{name} missing for {extra:?}"));
            assert_eq!(t["annotations"]["readOnlyHint"], json!(true), "{name}");
            assert_eq!(t["inputSchema"]["additionalProperties"], json!(false), "{name}");
        }
    }
}

#[test]
fn the_graph_group_is_unlisted_at_first_listed_by_tools_enable_and_callable_before_that() {
    let mut s = Session::start(&["--in-memory".to_string()], &[("URIEL_TOOLS", "adaptive")]);
    let first = s.names();
    assert!(GRAPH_TOOLS.iter().all(|n| !first.iter().any(|x| x == n)), "{first:?}");
    let r = call(&mut s, "graph_lint", json!({}));
    assert_eq!(r["n"], json!(0));
    let enabled = s.ok("tools_enable", json!({"group": "graph"}));
    let listed: BTreeSet<String> = enabled["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .map(|t| t.as_str().expect("name").to_string())
        .collect();
    assert_eq!(listed, GRAPH_TOOLS.iter().map(|n| n.to_string()).collect::<BTreeSet<_>>());
    let after = s.names();
    assert!(GRAPH_TOOLS.iter().all(|n| after.iter().any(|x| x == n)));
}

#[test]
fn karate_pagerank_and_betweenness_put_the_known_hubs_first() {
    let mut s = start();
    karate(&mut s);
    let pr = call(&mut s, "graph_rank", json!({"algorithm": "pagerank", "direction": "both", "k": 3}));
    assert_eq!(pr["n"], json!(34));
    assert_eq!(pr["edges"], json!(78));
    assert_eq!(pr["converged"], json!(true));
    assert_eq!(strs(&pr, "top", "name"), ["n33", "n0", "n32"]);
    assert!((pr["top"][1]["score"].as_f64().unwrap() - 0.096997).abs() < 1e-5, "{pr}");
    let bc = call(&mut s, "graph_rank", json!({"algorithm": "betweenness", "direction": "both", "k": 3}));
    assert_eq!(strs(&bc, "top", "name"), ["n0", "n33", "n32"]);
    assert!((bc["top"][0]["score"].as_f64().unwrap() - 231.071429).abs() < 1e-5, "{bc}");
    assert_eq!(bc["sampled"], json!(false));
}

#[test]
fn karate_degree_kcore_closeness_and_clustering_match_the_reference() {
    let mut s = start();
    karate(&mut s);
    let deg = call(&mut s, "graph_rank", json!({"algorithm": "degree", "direction": "both", "k": 2}));
    assert_eq!(strs(&deg, "top", "name"), ["n33", "n0"]);
    assert_eq!(col(&deg, "top", "score"), [json!(17.0), json!(16.0)]);
    let core = call(&mut s, "graph_rank", json!({"algorithm": "kcore", "k": 6}));
    assert_eq!(strs(&core, "top", "name"), ["n0", "n1", "n2", "n3", "n7", "n8"]);
    assert!(col(&core, "top", "score").iter().all(|x| *x == json!(4.0)));
    let close = call(&mut s, "graph_rank", json!({"algorithm": "closeness", "direction": "both", "k": 1}));
    assert_eq!(strs(&close, "top", "name"), ["n0"]);
    assert!((close["top"][0]["score"].as_f64().unwrap() - 0.568966).abs() < 1e-5);
    let clu = call(&mut s, "graph_rank", json!({"algorithm": "clustering", "k": 1}));
    assert!((clu["average_clustering"].as_f64().unwrap() - 0.570638).abs() < 1e-5);
    assert_eq!(clu["top"][0]["score"], json!(1.0));
}

#[test]
fn rank_filters_by_label_and_direction_and_breaks_ties_by_id() {
    let mut s = start();
    create(
        &mut s,
        &[("A", "name: 'a0'"), ("A", "name: 'a1'"), ("A", "name: 'a2'"), ("B", "name: 'b0'")],
        &[(0, 1, "R"), (0, 2, "R"), (3, 0, "R")],
    );
    let all = call(&mut s, "graph_rank", json!({"algorithm": "degree", "direction": "out"}));
    assert_eq!(strs(&all, "top", "name"), ["a0", "b0", "a1", "a2"]);
    let only_a = call(&mut s, "graph_rank", json!({"algorithm": "degree", "direction": "in", "labels": ["A"]}));
    assert_eq!(only_a["n"], json!(3));
    assert_eq!(only_a["edges"], json!(2));
    assert_eq!(strs(&only_a, "top", "name"), ["a1", "a2", "a0"]);
    let typed = call(&mut s, "graph_rank", json!({"algorithm": "degree", "rel_types": ["NOPE"]}));
    assert_eq!(typed["edges"], json!(0));
}

#[test]
fn rank_rejects_bad_arguments_with_typed_errors() {
    let mut s = start();
    karate(&mut s);
    for (args, arg) in [
        (json!({}), "algorithm"),
        (json!({"algorithm": "eigenvector"}), "algorithm"),
        (json!({"algorithm": "degree", "k": 0}), "k"),
        (json!({"algorithm": "degree", "k": 201}), "k"),
        (json!({"algorithm": "degree", "direction": "sideways"}), "direction"),
        (json!({"algorithm": "degree", "bogus": true}), "bogus"),
        (json!({"algorithm": "degree", "labels": ["P`; MATCH (n) DETACH DELETE n //"]}), "labels"),
        (json!({"algorithm": "degree", "rel_types": [3]}), "rel_types"),
        (json!({"algorithm": "degree", "max_nodes": 0}), "max_nodes"),
        (json!({"algorithm": "degree", "max_nodes": 500001}), "max_nodes"),
    ] {
        let e = fails(&mut s, "graph_rank", args.clone());
        assert_eq!(e["error"], json!("bad_argument"), "{args}");
        assert_eq!(e["argument"], json!(arg), "{args}");
    }
}

#[test]
fn a_selection_over_max_nodes_is_a_typed_too_large_error_that_names_a_filter_that_fits() {
    let mut s = start();
    let big = named("b", 30);
    let small = named("s", 5);
    let mut nodes = nodes_of("Big", &big);
    nodes.extend(nodes_of("Small", &small));
    create(&mut s, &nodes, &[(0, 1, "R")]);
    for tool in ["graph_rank", "graph_cycles", "graph_communities", "graph_lint"] {
        let args = if tool == "graph_rank" {
            json!({"algorithm": "degree", "max_nodes": 10})
        } else {
            json!({"max_nodes": 10})
        };
        let e = fails(&mut s, tool, args);
        assert_eq!(e["error"], json!("too_large"), "{tool}");
        assert_eq!(e["what"], json!("nodes"));
        assert_eq!(e["count"], json!(35));
        assert_eq!(e["limit"], json!(10));
        assert_eq!(e["labels_that_fit"], json!([{"label": "Small", "nodes": 5}]));
    }
    let e = fails(&mut s, "graph_impact", json!({"target": "b0", "max_nodes": 10}));
    assert_eq!(e["error"], json!("not_found"), "target resolution happens before the projection");
    let b0 = id_of(&mut s, "b0");
    let e = fails(&mut s, "graph_impact", json!({"target": b0, "max_nodes": 10}));
    assert_eq!(e["error"], json!("too_large"));
    let e = fails(&mut s, "graph_path", json!({"from": b0, "to": b0, "max_nodes": 10}));
    assert_eq!(e["error"], json!("too_large"));
    let ok = call(&mut s, "graph_rank", json!({"algorithm": "degree", "max_nodes": 10, "labels": ["Small"]}));
    assert_eq!(ok["n"], json!(5));
    let overlap = call(&mut s, "graph_rank", json!({"algorithm": "degree", "max_nodes": 30, "labels": ["Big", "Big"]}));
    assert_eq!(overlap["n"], json!(30));
}

#[test]
fn closeness_refuses_more_than_ten_thousand_nodes_but_the_filter_unblocks_it() {
    let mut s = start();
    s.text(
        "write_neo4j_cypher",
        json!({"query": "UNWIND range(1, 10001) AS i CREATE (:Many {name: 'm' + toString(i)})"}),
    );
    s.text("write_neo4j_cypher", json!({"query": "CREATE (:Few {name: 'f'})"}));
    let e = fails(&mut s, "graph_rank", json!({"algorithm": "closeness"}));
    assert_eq!(e["error"], json!("too_large"));
    assert_eq!(e["limit"], json!(10000));
    let ok = call(&mut s, "graph_rank", json!({"algorithm": "closeness", "labels": ["Few"]}));
    assert_eq!(ok["n"], json!(1));
    let sampled = call(&mut s, "graph_rank", json!({"algorithm": "betweenness", "k": 1}));
    assert_eq!(sampled["sampled"], json!(true));
    assert_eq!(sampled["sample_sources"], json!(1024));
}

#[test]
fn karate_louvain_modularity_matches_the_algorithm_crate_and_the_reference_band() {
    let mut s = start();
    karate(&mut s);
    let r = call(&mut s, "graph_communities", json!({}));
    let edges: Vec<(u32, u32)> = KARATE.iter().map(|&(a, b)| (a as u32, b as u32)).collect();
    let g = Projection::from_edges(34, &edges, false).unwrap();
    let expect = louvain(&g, &LouvainConfig::default()).unwrap().modularity;
    let q = r["modularity"].as_f64().unwrap();
    assert!((q - expect).abs() < 1e-6, "{q} vs {expect}");
    assert!((0.40..=0.43).contains(&q), "{q}");
    let count = r["community_count"].as_u64().unwrap();
    assert!((3..=5).contains(&count), "{count}");
    let sizes: u64 = col(&r, "communities", "size").iter().map(|x| x.as_u64().unwrap()).sum();
    assert_eq!(sizes, 34);
    let home = |name: &str| -> u64 {
        let rows = r["communities"].as_array().unwrap();
        let row = rows
            .iter()
            .find(|row| row["names"].as_array().unwrap().iter().any(|n| n == name))
            .unwrap_or_else(|| panic!("{name} is not a top member of any community: {r}"));
        row["community"].as_u64().unwrap()
    };
    assert_ne!(home("n0"), home("n33"));
}

#[test]
fn a_barbell_has_two_cliques_one_cut_edge_and_two_bridge_nodes() {
    let mut s = start();
    barbell(&mut s);
    let r = call(&mut s, "graph_communities", json!({}));
    assert_eq!(r["community_count"], json!(2));
    assert!((r["modularity"].as_f64().unwrap() - 0.423077).abs() < 1e-5, "{r}");
    assert_eq!(col(&r, "communities", "size"), [json!(4), json!(4)]);
    assert_eq!(col(&r, "communities", "internal_edges"), [json!(6), json!(6)]);
    assert_eq!(col(&r, "communities", "external_edges"), [json!(1), json!(1)]);
    assert_eq!(col(&r, "communities", "cohesion"), [json!(0.857143), json!(0.857143)]);
    assert_eq!(r["cut_edge_count"], json!(1));
    assert_eq!(strs(&r, "cut_edges", "from_name"), ["k3"]);
    assert_eq!(strs(&r, "cut_edges", "to_name"), ["k4"]);
    assert_eq!(strs(&r, "bridge_nodes", "name"), ["k3", "k4"]);
    assert_eq!(col(&r, "bridge_nodes", "cross_edges"), [json!(1), json!(1)]);
    let fine = call(&mut s, "graph_communities", json!({"resolution": 100}));
    assert!(fine["community_count"].as_u64().unwrap() >= 2);
    let e = fails(&mut s, "graph_communities", json!({"resolution": 0}));
    assert_eq!(e["argument"], json!("resolution"));
}

#[test]
fn a_barbell_blast_radius_names_both_cut_vertices_and_the_bridge() {
    let mut s = start();
    barbell(&mut s);
    let r = call(&mut s, "graph_impact", json!({"target": by_name("B", "k0"), "direction": "both", "depth": 3}));
    assert_eq!(r["reach"], json!(7));
    assert_eq!(r["reach_pct"], json!(100.0));
    assert_eq!(r["articulation_points"], json!(2));
    assert_eq!(r["bridges"], json!(1));
    assert_eq!(strs(&r, "critical", "kind"), ["articulation", "articulation", "bridge"]);
    assert_eq!(strs(&r, "critical", "name"), ["k3", "k4", "k3"]);
    assert_eq!(col(&r, "critical", "splits_into")[..2], [json!(2), json!(2)]);
    assert_eq!(col(&r, "critical", "largest_part")[..2], [json!(4), json!(4)]);
    assert_eq!(strs(&r, "critical", "other_name")[2], "k4");
}

#[test]
fn impact_on_a_dependency_chain_counts_layers_and_reach_per_direction() {
    let mut s = start();
    let props = named("d", 5);
    let nodes = nodes_of("M", &props);
    let rels: Vec<(usize, usize, &str)> = (0..4).map(|i| (i, i + 1, "DEPENDS_ON")).collect();
    create(&mut s, &nodes, &rels);
    let target = by_name("M", "d2");
    let up = call(&mut s, "graph_impact", json!({"target": target}));
    assert_eq!(up["direction"], json!("dependents"));
    assert_eq!(col(&up, "layers", "count"), [json!(1), json!(1), json!(0)]);
    assert_eq!(up["reach"], json!(2));
    assert_eq!(up["reach_pct"], json!(50.0));
    assert_eq!(strs(&up, "affected", "name"), ["d1", "d0"]);
    assert_eq!(col(&up, "affected", "depth"), [json!(1), json!(2)]);
    assert_eq!(up["articulation_points"], json!(1));
    assert_eq!(up["bridges"], json!(2));
    assert_eq!(strs(&up, "critical", "name")[0], "d1");
    let down = call(&mut s, "graph_impact", json!({"target": target, "direction": "dependencies"}));
    assert_eq!(strs(&down, "affected", "name"), ["d3", "d4"]);
    let both = call(&mut s, "graph_impact", json!({"target": target, "direction": "both", "depth": 2}));
    assert_eq!(both["reach"], json!(4));
    assert_eq!(both["reach_pct"], json!(100.0));
    let shallow = call(&mut s, "graph_impact", json!({"target": target, "depth": 1}));
    assert_eq!(shallow["reach"], json!(1));
    assert_eq!(shallow["reach_unbounded"], json!(2));
    assert_eq!(shallow["reach_unbounded_pct"], json!(50.0));
    let leaf = call(&mut s, "graph_impact", json!({"target": by_name("M", "d0")}));
    assert_eq!(leaf["reach"], json!(0));
    assert_eq!(leaf["affected"], json!([]));
    for (args, arg) in [
        (json!({"target": target, "depth": 0}), "depth"),
        (json!({"target": target, "depth": 65}), "depth"),
        (json!({"target": target, "direction": "sideways"}), "direction"),
        (json!({}), "target"),
    ] {
        let e = fails(&mut s, "graph_impact", args.clone());
        assert_eq!(e["error"], json!("bad_argument"), "{args}");
        assert_eq!(e["argument"], json!(arg), "{args}");
    }
}

#[test]
fn a_dag_with_a_planted_cycle_and_a_self_loop_reports_both_and_a_minimal_cut() {
    let mut s = start();
    let props: Vec<String> = ["a", "b", "c", "d", "e", "f", "g"].iter().map(|n| format!("name: '{n}'")).collect();
    let nodes = nodes_of("S", &props);
    create(
        &mut s,
        &nodes,
        &[
            (0, 1, "DEP"),
            (1, 2, "DEP"),
            (2, 0, "BACK"),
            (3, 3, "DEP"),
            (4, 0, "DEP"),
            (2, 5, "DEP"),
            (5, 6, "DEP"),
        ],
    );
    let r = call(&mut s, "graph_cycles", json!({}));
    assert_eq!(r["acyclic"], json!(false));
    assert_eq!(r["cycle_count"], json!(1));
    assert_eq!(r["cyclic_nodes"], json!(3));
    assert_eq!(r["self_loop_count"], json!(1));
    assert_eq!(r["feedback_arc_count"], json!(2));
    assert_eq!(r["layer_count"], json!(4));
    assert_eq!(r["cycles"][0]["size"], json!(3));
    let members: BTreeSet<String> = r["cycles"][0]["names"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_str().unwrap().to_string())
        .collect();
    assert_eq!(members, ["a", "b", "c"].iter().map(|x| x.to_string()).collect());
    assert_eq!(strs(&r, "self_loops", "name"), ["d"]);
    assert_eq!(strs(&r, "feedback_arcs", "kind"), ["cycle", "self_loop"]);
}

#[test]
fn cycles_respect_the_relationship_type_filter_and_a_dag_is_acyclic() {
    let mut s = start();
    let props: Vec<String> = ["a", "b", "c"].iter().map(|n| format!("name: '{n}'")).collect();
    let nodes = nodes_of("S", &props);
    create(&mut s, &nodes, &[(0, 1, "DEP"), (1, 2, "DEP"), (2, 0, "BACK")]);
    let full = call(&mut s, "graph_cycles", json!({}));
    assert_eq!(full["acyclic"], json!(false));
    assert_eq!(full["feedback_arc_count"], json!(1));
    let dag = call(&mut s, "graph_cycles", json!({"rel_types": ["DEP"]}));
    assert_eq!(dag["acyclic"], json!(true));
    assert_eq!(dag["feedback_arc_count"], json!(0));
    assert_eq!(dag["layer_count"], json!(3));
    assert_eq!(dag["cycles"], json!([]));
}

#[test]
fn path_shortest_takes_the_skip_edge_and_names_every_hop() {
    let mut s = start();
    chain(&mut s);
    let from = by_name("P", "n0");
    let to = by_name("P", "n5");
    let r = call(&mut s, "graph_path", json!({"from": from, "to": to}));
    assert_eq!(r["result"], json!("path"));
    assert_eq!(r["hops"], json!(3));
    assert_eq!(r["cost"], json!(3.0));
    assert_eq!(strs(&r, "steps", "name"), ["n0", "n3", "n4", "n5"]);
    assert_eq!(strs(&r, "steps", "rel_type")[1..], ["SKIP", "NEXT", "NEXT"]);
    assert_eq!(col(&r, "steps", "rel_type")[0], Value::Null);
    let only_next = call(&mut s, "graph_path", json!({"from": from, "to": to, "rel_types": ["NEXT"]}));
    assert_eq!(only_next["hops"], json!(5));
}

#[test]
fn path_weighted_and_k_modes_use_the_weight_property() {
    let mut s = start();
    chain(&mut s);
    let from = by_name("P", "n0");
    let to = by_name("P", "n5");
    let w = call(&mut s, "graph_path", json!({"from": from, "to": to, "mode": "weighted", "weight": "w"}));
    assert_eq!(w["cost"], json!(5.0));
    assert_eq!(w["hops"], json!(5));
    assert_eq!(strs(&w, "steps", "name"), ["n0", "n1", "n2", "n3", "n4", "n5"]);
    assert_eq!(col(&w, "steps", "weight")[1], json!(1.0));
    let k = call(&mut s, "graph_path", json!({"from": from, "to": to, "mode": "k", "weight": "w", "k": 5}));
    assert_eq!(col(&k, "paths", "cost"), [json!(5.0), json!(12.0)]);
    assert_eq!(col(&k, "paths", "hops"), [json!(5), json!(3)]);
    let hops = call(&mut s, "graph_path", json!({"from": from, "to": to, "mode": "k", "k": 5}));
    assert_eq!(col(&hops, "paths", "cost"), [json!(3.0), json!(5.0)]);
}

#[test]
fn path_direction_in_walks_edges_backwards_and_accepts_node_ids() {
    let mut s = start();
    chain(&mut s);
    let (n5, n0) = (id_of(&mut s, "n5"), id_of(&mut s, "n0"));
    let back = call(&mut s, "graph_path", json!({"from": n5, "to": {"id": n0}, "direction": "in"}));
    assert_eq!(back["hops"], json!(3));
    assert_eq!(strs(&back, "steps", "name"), ["n5", "n4", "n3", "n0"]);
    assert_eq!(strs(&back, "steps", "rel_type")[3], "SKIP");
    let both = call(&mut s, "graph_path", json!({"from": n5, "to": n0, "direction": "both"}));
    assert_eq!(both["hops"], json!(3));
}

#[test]
fn path_without_a_route_is_a_normal_result_with_reach_sizes_not_an_error() {
    let mut s = start();
    chain(&mut s);
    let n3 = id_of(&mut s, "n3");
    let n1 = id_of(&mut s, "n1");
    let r = call(&mut s, "graph_path", json!({"from": n3, "to": n1}));
    assert_eq!(r["result"], json!("no_path"));
    assert_eq!(r["reason"], json!("unreachable"));
    assert_eq!(r["reach_from"], json!(2));
    assert_eq!(r["reach_to"], json!(1));
    assert_eq!(r["steps"], json!([]));
    let capped = call(
        &mut s,
        "graph_path",
        json!({"from": by_name("P", "n0"), "to": by_name("P", "n5"), "max_hops": 2}),
    );
    assert_eq!(capped["result"], json!("no_path"));
    assert_eq!(capped["reason"], json!("max_hops"));
    let same = call(&mut s, "graph_path", json!({"from": by_name("P", "n2"), "to": by_name("P", "n2")}));
    assert_eq!(same["hops"], json!(0));
    assert_eq!(same["cost"], json!(0.0));
}

#[test]
fn path_endpoints_accept_memory_entity_names_and_report_missing_or_ambiguous_ones() {
    let mut s = start();
    s.ok(
        "create_entities",
        json!({"entities": [
            {"name": "Alice", "type": "Person", "observations": ["a"]},
            {"name": "Bob", "type": "Person", "observations": ["b"]}
        ]}),
    );
    s.ok(
        "create_relations",
        json!({"relations": [{"source": "Alice", "target": "Bob", "relationType": "KNOWS"}]}),
    );
    let r = call(&mut s, "graph_path", json!({"from": "Alice", "to": "Bob"}));
    assert_eq!(strs(&r, "steps", "name"), ["Alice", "Bob"]);
    assert_eq!(strs(&r, "steps", "rel_type")[1], "KNOWS");
    let missing = fails(&mut s, "graph_path", json!({"from": "Alice", "to": "Nobody"}));
    assert_eq!(missing["error"], json!("not_found"));
    assert_eq!(missing["what"], json!("to"));
    let wide = json!({"label": "Person", "key": "type", "value": "Person"});
    let ambiguous = fails(&mut s, "graph_path", json!({"from": "Alice", "to": wide}));
    assert_eq!(ambiguous["error"], json!("ambiguous"));
    let gone = fails(&mut s, "graph_path", json!({"from": 999999, "to": "Bob"}));
    assert_eq!(gone["error"], json!("not_found"));
    let outside = fails(&mut s, "graph_path", json!({"from": "Alice", "to": "Bob", "labels": ["Nope"]}));
    assert_eq!(outside["error"], json!("not_found"));
}

#[test]
fn path_rejects_bad_arguments_with_typed_errors() {
    let mut s = start();
    chain(&mut s);
    let ok = by_name("P", "n0");
    for (args, arg) in [
        (json!({"from": ok, "to": "n1", "mode": "weighted"}), "weight"),
        (json!({"from": ok, "to": "n1", "weight": "w"}), "weight"),
        (json!({"from": ok, "to": "n1", "mode": "k", "k": 11}), "k"),
        (json!({"from": ok, "to": "n1", "mode": "k", "k": 0}), "k"),
        (json!({"from": ok, "to": "n1", "mode": "teleport"}), "mode"),
        (json!({"from": ok, "to": "n1", "direction": "up"}), "direction"),
        (json!({"from": ok, "to": "n1", "max_hops": 0}), "max_hops"),
        (json!({"from": ok, "to": "n1", "bogus": 1}), "bogus"),
        (json!({"from": {"label": "P`", "key": "name", "value": "x"}, "to": "n1"}), "from"),
        (json!({"from": [1], "to": "n1"}), "from"),
        (json!({"from": ok, "to": "n1", "max_nodes": 500001}), "max_nodes"),
        (json!({"from": ok, "to": "n1", "labels": ["A`B"]}), "labels"),
        (json!({"from": ok, "to": "n1", "format": "xml"}), "format"),
    ] {
        let e = fails(&mut s, "graph_path", args.clone());
        assert_eq!(e["error"], json!("bad_argument"), "{args}");
        assert_eq!(e["argument"], json!(arg), "{args}");
    }
}

#[test]
fn path_with_a_negative_weight_is_a_typed_bad_weight_error() {
    let mut s = start();
    create(&mut s, &[("N", "name: 'x'"), ("N", "name: 'y'")], &[(0, 1, "R {w: -2}")]);
    let e = fails(
        &mut s,
        "graph_path",
        json!({"from": by_name("N", "x"), "to": by_name("N", "y"), "mode": "weighted", "weight": "w"}),
    );
    assert_eq!(e["error"], json!("bad_weight"));
}

fn lint_graph(s: &mut Session) {
    let leaves: Vec<String> = (0..40).map(|i| format!("name: 'leaf{i}', size: {i}")).collect();
    let mut nodes: Vec<(&str, &str)> = vec![("Hub", "name: 'hub', size: 0")];
    nodes.extend(nodes_of("Leaf", &leaves));
    let things = [
        "name: 'Alpha', size: 1",
        "name: 'alpha', size: 2",
        "name: 'o1', size: 3",
        "name: 'o2'",
        "size: 5",
    ];
    nodes.extend(things.iter().map(|p| ("Thing", *p)));
    let mut rels: Vec<(usize, usize, &str)> = (1..=40).map(|i| (0, i, "LINKS")).collect();
    rels.push((41, 42, "SAME"));
    create(s, &nodes, &rels);
    s.ok(
        "create_entities",
        json!({"entities": [
            {"name": "Carol", "type": "Person", "observations": []},
            {"name": "Dave", "type": "Person", "observations": ["x"]}
        ]}),
    );
}

fn finding<'a>(r: &'a Value, kind: &str) -> &'a Value {
    r["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["kind"] == kind)
        .unwrap_or_else(|| panic!("no {kind} finding in {r}"))
}

#[test]
fn lint_finds_every_planted_defect_with_counts_samples_and_fixes() {
    let mut s = start();
    lint_graph(&mut s);
    let r = call(&mut s, "graph_lint", json!({}));
    assert_eq!(r["n"], json!(48));
    assert_eq!(r["edges"], json!(41));
    assert!((r["density"].as_f64().unwrap() - 41.0 / (48.0 * 47.0)).abs() < 1e-6);
    assert_eq!(r["orphans"], json!(5));
    let orphans = finding(&r, "orphans");
    assert_eq!(orphans["severity"], json!("warn"));
    assert_eq!(orphans["count"], json!(5));
    assert_eq!(orphans["sample"].as_array().unwrap().len(), 5);
    let dup = finding(&r, "duplicate_names");
    assert_eq!(dup["label"], json!("Thing"));
    assert_eq!(dup["count"], json!(1));
    assert_eq!(dup["sample"].as_array().unwrap().len(), 2);
    let missing = r["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["kind"] == "missing_property" && f["label"] == "Thing")
        .expect("missing_property for Thing");
    assert_eq!(missing["count"], json!(1));
    assert_eq!(missing["rate"], json!(0.2));
    assert_eq!(missing["severity"], json!("warn"));
    let hub = id_of(&mut s, "hub");
    let sup = finding(&r, "super_nodes");
    assert_eq!(sup["count"], json!(1));
    assert_eq!(sup["sample"], json!([hub]));
    let mem = finding(&r, "memory_no_observations");
    assert_eq!(mem["count"], json!(1));
    let frag = finding(&r, "fragmented");
    assert_eq!(frag["severity"], json!("info"));
    assert_eq!(frag["count"], json!(1));
    let hist: Vec<(u64, u64)> = r["components"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| (c["size"].as_u64().unwrap(), c["components"].as_u64().unwrap()))
        .collect();
    assert_eq!(hist, [(41, 1), (2, 1), (1, 5)]);
    assert_eq!(r["component_count"], json!(7));
    let types: Vec<(String, u64)> = r["rel_types"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| (c["type"].as_str().unwrap().to_string(), c["count"].as_u64().unwrap()))
        .collect();
    assert_eq!(types, [("LINKS".to_string(), 40), ("SAME".to_string(), 1)]);
    let order: Vec<&str> = r["findings"].as_array().unwrap().iter().map(|f| f["severity"].as_str().unwrap()).collect();
    let mut sorted = order.clone();
    sorted.sort_by_key(|x| ["error", "warn", "info"].iter().position(|y| y == x));
    assert_eq!(order, sorted);
}

#[test]
fn lint_fix_statements_are_exact_cypher_the_tool_never_runs_and_they_work() {
    let mut s = start();
    lint_graph(&mut s);
    let before = call(&mut s, "graph_lint", json!({}));
    let again = call(&mut s, "graph_lint", json!({}));
    assert_eq!(before, again, "calling lint changes nothing");
    assert_eq!(before["orphans"], json!(5));
    for kind in ["duplicate_names", "missing_property", "super_nodes", "memory_no_observations", "fragmented"] {
        let f = r_finding_with_fix(&before, kind);
        let rows = s.ok("read_neo4j_cypher", json!({"query": f}));
        assert!(!rows.as_array().unwrap().is_empty(), "{kind}: {f}");
    }
    let orphans = finding(&before, "orphans");
    let fix = orphans["fix"].as_str().unwrap();
    assert!(fix.starts_with("MATCH (n) WHERE id(n) IN ["), "{fix}");
    s.text("write_neo4j_cypher", json!({"query": fix}));
    let after = call(&mut s, "graph_lint", json!({}));
    assert_eq!(after["orphans"], json!(0));
    assert_eq!(after["n"], json!(43));
    assert!(after["findings"].as_array().unwrap().iter().all(|f| f["kind"] != "orphans"));
}

fn r_finding_with_fix(r: &Value, kind: &str) -> String {
    finding(r, kind)["fix"].as_str().unwrap_or_else(|| panic!("{kind} has no fix")).to_string()
}

#[test]
fn lint_samples_are_capped_by_k_and_the_fix_covers_exactly_the_sample() {
    let mut s = start();
    lint_graph(&mut s);
    let r = call(&mut s, "graph_lint", json!({"k": 2}));
    let orphans = finding(&r, "orphans");
    assert_eq!(orphans["count"], json!(5));
    assert_eq!(orphans["sample"].as_array().unwrap().len(), 2);
    let ids: Vec<String> = orphans["sample"].as_array().unwrap().iter().map(|x| x.to_string()).collect();
    assert!(orphans["fix"].as_str().unwrap().contains(&format!("[{}]", ids.join(", "))));
    let e = fails(&mut s, "graph_lint", json!({"k": 101}));
    assert_eq!(e["argument"], json!("k"));
}

#[test]
fn lint_of_an_empty_graph_is_one_info_finding() {
    let mut s = start();
    let r = call(&mut s, "graph_lint", json!({}));
    assert_eq!(r["n"], json!(0));
    assert_eq!(finding(&r, "empty_graph")["severity"], json!("info"));
}

#[test]
fn every_tool_is_deterministic_in_both_encodings() {
    let mut s = start();
    karate(&mut s);
    let calls = [
        ("graph_rank", json!({"algorithm": "pagerank", "direction": "both"})),
        ("graph_rank", json!({"algorithm": "betweenness", "direction": "both"})),
        ("graph_path", json!({"from": by_name("P", "n16"), "to": by_name("P", "n26"), "direction": "both", "mode": "k", "k": 4})),
        ("graph_impact", json!({"target": by_name("P", "n0"), "direction": "both", "depth": 2})),
        ("graph_cycles", json!({})),
        ("graph_communities", json!({})),
        ("graph_lint", json!({})),
    ];
    for (tool, args) in calls {
        for format in ["json", "compact"] {
            let mut a = args.clone();
            a["format"] = json!(format);
            let first = s.text(tool, a.clone());
            let second = s.text(tool, a);
            assert_eq!(first, second, "{tool} {format}");
            assert!(!first.is_empty());
        }
    }
}

#[test]
fn compact_is_the_default_encoding_and_every_table_decodes() {
    let mut s = start();
    karate(&mut s);
    let text = s.text("graph_rank", json!({"algorithm": "pagerank", "direction": "both", "k": 3}));
    assert!(text.starts_with("#1\n"), "{text}");
    let (head, rest) = text.split_once("\n## top\n").expect("top section");
    let head = uriel_mcp::compact::decode_table(head).expect("head decodes");
    assert!(head.columns.contains(&"algorithm".to_string()));
    let top = uriel_mcp::compact::decode_table(rest).expect("top decodes");
    assert_eq!(top.columns, ["id", "label", "name", "score"]);
    assert_eq!(top.rows.len(), 3);
    assert_eq!(top.rows[0][2], json!("n33"));
    let json_text = s.text("graph_rank", json!({"algorithm": "pagerank", "direction": "both", "k": 3, "format": "json"}));
    assert!(json_text.len() > text.len(), "compact {} vs json {}", text.len(), json_text.len());
}
