use std::collections::VecDeque;

use serde_json::{json, Value as Json};
use uriel_algo::{
    articulation_points, betweenness_centrality, bridges, bfs, closeness_centrality,
    degree_centrality, dijkstra_path, k_core_decomposition, local_clustering_coefficient, pagerank,
    yen_k_shortest_paths, BetweennessConfig, Direction, Orientation, PageRankConfig, Projection,
};
use uriel_core::{NodeId, ReadTx};
use uriel_query::project::weight_of;

use crate::graph_audit;
use crate::graph_project::{
    bad_arg, build, choice, direction, project_in, r4, r6, ranged, reject_unknown,
    resolve, snapshot, top_by_score, typed, who, Dir, Out, Scope, Tab, MAX_K, SCOPE_KEYS,
};
use crate::tools::{format_prop, object, str_list, str_prop, Context, ToolDef, ToolResult};

pub const EXACT_BETWEENNESS_NODES: usize = 5_000;
pub const BETWEENNESS_SAMPLE: usize = 1_024;
pub const CLOSENESS_MAX_NODES: usize = 10_000;
pub const MAX_PATHS: usize = 10;

pub(crate) fn scope_props() -> Json {
    json!({
        "labels": str_list("Keep only nodes carrying one of these labels (default: all nodes)."),
        "rel_types": str_list("Keep only relationships of these types (default: all types)."),
        "max_nodes": {"type": "integer", "minimum": 1, "maximum": 500000,
            "description": "Refuse (typed too_large error) when more nodes than this are selected. Default 50000, hard maximum 500000."},
        "format": format_prop()
    })
}

pub(crate) fn schema(extra: Json, required: &[&str]) -> Json {
    let mut props = scope_props();
    if let (Json::Object(p), Json::Object(e)) = (&mut props, extra) {
        p.extend(e);
    }
    object(props, required)
}

fn node_ref(desc: &str) -> Json {
    json!({
        "description": desc,
        "oneOf": [
            {"type": "integer", "minimum": 0, "description": "Node id."},
            str_prop("Memory entity name."),
            {"type": "object", "properties": {
                "id": {"type": "integer", "minimum": 0},
                "label": {"type": "string"}, "key": {"type": "string"}, "value": {}
            }, "additionalProperties": false, "description": "{id} or {label, key, value}; must match exactly one node."}
        ]
    })
}

fn k_prop(desc: &str) -> Json {
    json!({"type": "integer", "minimum": 1, "maximum": MAX_K, "description": desc})
}

fn direction_prop(desc: &str) -> Json {
    json!({"type": "string", "enum": ["out", "in", "both"], "description": desc})
}

fn rank_schema() -> Json {
    schema(
        json!({
            "algorithm": {"type": "string", "enum": ["pagerank", "betweenness", "closeness", "degree", "kcore", "clustering"],
                "description": "pagerank, betweenness (sampled from 1024 sources above 5000 nodes), closeness (refused above 10000 nodes), degree, kcore (core number) or clustering (local coefficient)."},
            "direction": direction_prop("Arc direction for pagerank, betweenness, closeness and degree: out (default), in or both (treated as undirected). kcore and clustering are always undirected."),
            "k": k_prop("How many top nodes to return. Default 10, maximum 200.")
        }),
        &["algorithm"],
    )
}

fn path_schema() -> Json {
    schema(
        json!({
            "from": node_ref("Start node."),
            "to": node_ref("End node."),
            "mode": {"type": "string", "enum": ["shortest", "weighted", "k"],
                "description": "shortest (fewest hops, default), weighted (Dijkstra on the relationship property `weight`) or k (Yen's k shortest simple paths, hop cost unless `weight` is given)."},
            "weight": str_prop("Relationship property holding a non-negative number; missing values count as 1. Required for mode weighted."),
            "k": k_prop("Paths to return in mode k. Default 3, maximum 10."),
            "max_hops": {"type": "integer", "minimum": 1, "description": "Longest path allowed, in hops. Exact in shortest mode; weighted and k modes drop longer results."},
            "direction": direction_prop("Follow relationships out (default), in, or both.")
        }),
        &["from", "to"],
    )
}

fn impact_schema() -> Json {
    schema(
        json!({
            "target": node_ref("The node or change to assess."),
            "direction": {"type": "string", "enum": ["dependents", "dependencies", "both"],
                "description": "dependents: nodes that reach the target along relationships (default); dependencies: nodes the target reaches; both."},
            "depth": {"type": "integer", "minimum": 1, "maximum": 64, "description": "Hops to follow. Default 3."},
            "k": k_prop("How many most-affected nodes to list. Default 10.")
        }),
        &["target"],
    )
}

fn cycles_schema() -> Json {
    schema(
        json!({"k": k_prop("How many cycles (strong components of size > 1) to list, largest first. Default 20.")}),
        &[],
    )
}

fn communities_schema() -> Json {
    schema(
        json!({
            "resolution": {"type": "number", "exclusiveMinimum": 0, "maximum": 100,
                "description": "Louvain resolution; larger gives smaller communities. Default 1."},
            "k": k_prop("How many communities to list, largest first. Default 10.")
        }),
        &[],
    )
}

fn lint_schema() -> Json {
    schema(
        json!({"k": {"type": "integer", "minimum": 1, "maximum": 100,
            "description": "Sample ids per finding, and the ids each fix statement covers. Default 10."}}),
        &[],
    )
}

pub const TOOLS: &[ToolDef] = &[
    ToolDef {
        name: "graph_rank",
        title: "Rank graph nodes",
        description: "Top-k nodes by pagerank, betweenness, closeness, degree, k-core or clustering over a label/relationship-type filtered projection. Ties break by node id. Returns n, edges, convergence info for pagerank and a table of id, label, name, score.",
        writes: false,
        schema: rank_schema,
    },
    ToolDef {
        name: "graph_path",
        title: "Find paths between nodes",
        description: "Shortest, weighted or k shortest simple paths between two nodes given by id, Memory entity name or {label, key, value}. Returns the node and relationship sequence with names and cost; an unreachable pair is a normal no_path result with the reachable-set size from each side.",
        writes: false,
        schema: path_schema,
    },
    ToolDef {
        name: "graph_impact",
        title: "Blast radius of a node",
        description: "Blast radius of a node: BFS layers over its dependents or dependencies, reach as a percentage of the graph, affected nodes ranked by pagerank-weighted proximity, and the articulation points and bridges of the affected subgraph (what splits if removed).",
        writes: false,
        schema: impact_schema,
    },
    ToolDef {
        name: "graph_cycles",
        title: "Find dependency cycles",
        description: "Dependency cycles: strong components of size > 1 and self-loops, a greedy minimal set of edges to cut to make the graph acyclic, and the topological layers of the condensation DAG.",
        writes: false,
        schema: cycles_schema,
    },
    ToolDef {
        name: "graph_communities",
        title: "Detect communities",
        description: "Louvain communities (deterministic) with modularity, per-community size, top members by pagerank, internal and external edge counts and cohesion, the cut edges between communities and the bridge nodes with highest betweenness.",
        writes: false,
        schema: communities_schema,
    },
    ToolDef {
        name: "graph_lint",
        title: "Audit graph health",
        description: "Graph health audit: orphans, case-insensitive duplicate names per label, nodes missing their label's most common property, super-nodes (degree above mean + 3 sigma), component histogram, density, relationship-type histogram and Memory entities without observations. Each finding carries a Cypher statement as text in `fix`; the tool never runs it.",
        writes: false,
        schema: lint_schema,
    },
];

pub fn call(ctx: &Context, name: &str, args: &Json) -> ToolResult {
    match name {
        "graph_rank" => rank(ctx, args),
        "graph_path" => path(ctx, args),
        "graph_impact" => impact(ctx, args),
        "graph_cycles" => graph_audit::cycles(ctx, args),
        "graph_communities" => graph_audit::communities(ctx, args),
        "graph_lint" => graph_audit::lint(ctx, args),
        _ => Err(format!("unhandled tool '{name}'")),
    }
}

fn algo_err(e: uriel_algo::Error) -> String {
    match e {
        uriel_algo::Error::NegativeWeight { .. } | uriel_algo::Error::InvalidWeight(_) => {
            typed("bad_weight", json!({"message": e.to_string()}))
        }
        other => other.to_string(),
    }
}

fn head_counts(out: &mut Out, nodes: usize, edges: usize) {
    out.put("n", nodes);
    out.put("edges", edges);
}

fn rank(ctx: &Context, args: &Json) -> ToolResult {
    let mut allowed = vec!["algorithm", "direction", "k"];
    allowed.extend_from_slice(SCOPE_KEYS);
    reject_unknown(args, &allowed)?;
    let algorithm = match args.get("algorithm") {
        None | Some(Json::Null) => return Err(bad_arg("algorithm", "required")),
        Some(_) => choice(
            args,
            "algorithm",
            &["pagerank", "betweenness", "closeness", "degree", "kcore", "clustering"],
            "",
        )?,
    };
    let scope = Scope::parse(ctx, args)?;
    let dir = direction(args, Dir::Out)?;
    let k = ranged(args, "k", 10, 1, MAX_K)?;
    let read = snapshot(ctx)?;
    let p = project_in(&read, &scope, None)?;
    let n = p.ids.len();
    let mut out = Out::default();
    out.put("algorithm", algorithm);
    out.put("direction", dir.name());
    head_counts(&mut out, n, p.edges.len());
    let undirected = Orientation::Undirected;
    let scores: Vec<f64> = match algorithm {
        "pagerank" => {
            let g = build(&p, dir.orientation(), false)?;
            let cfg = PageRankConfig::default();
            let r = pagerank(&g, &cfg).map_err(algo_err)?;
            out.put("damping", cfg.damping);
            out.put("iterations", r.iterations);
            out.put("converged", r.converged);
            r.scores
        }
        "betweenness" => {
            let g = build(&p, dir.orientation(), false)?;
            let sampled = n > EXACT_BETWEENNESS_NODES;
            let cfg = BetweennessConfig {
                normalize: false,
                sample_size: sampled.then_some(BETWEENNESS_SAMPLE),
                seed: 0,
            };
            out.put("sampled", sampled);
            if sampled {
                out.put("sample_sources", BETWEENNESS_SAMPLE);
            }
            betweenness_centrality(&g, &cfg).map_err(algo_err)?
        }
        "closeness" => {
            if n > CLOSENESS_MAX_NODES {
                return Err(typed(
                    "too_large",
                    json!({
                        "what": "nodes", "count": n, "limit": CLOSENESS_MAX_NODES,
                        "hint": "closeness costs O(n*(n+m)); pass labels or rel_types to select at most 10000 nodes, or use pagerank or betweenness"
                    }),
                ));
            }
            let g = build(&p, dir.orientation(), false)?;
            closeness_centrality(&g, true)
        }
        "degree" => {
            let g = build(&p, Orientation::Natural, false)?;
            let d = match dir {
                Dir::Out => Direction::Out,
                Dir::In => Direction::In,
                Dir::Both => Direction::Both,
            };
            degree_centrality(&g, d, false)
        }
        "kcore" => {
            let g = build(&p, undirected, false)?;
            k_core_decomposition(&g).into_iter().map(f64::from).collect()
        }
        _ => {
            let g = build(&p, undirected, false)?;
            let c = local_clustering_coefficient(&g);
            out.put("average_clustering", r6(c.average));
            c.coefficients
        }
    };
    let mut top = Tab::new("top", &["id", "label", "name", "score"]);
    for i in top_by_score(&scores, k) {
        let w = who(&read, p.ids[i]);
        let [id, label, name] = w.cells();
        top.rows.push(vec![id, label, name, r6(scores[i])]);
    }
    out.put("returned", top.rows.len());
    out.tabs.push(top);
    Ok(out.render(scope.format))
}

fn numbers(read: &ReadTx, rel: u64, key: &str) -> Option<f64> {
    read.get_rel(uriel_core::RelId(rel))
        .ok()
        .flatten()
        .and_then(|r| weight_of(r.props.get(key)))
}

fn edge_between(
    read: &ReadTx,
    a: NodeId,
    b: NodeId,
    dir: Dir,
    types: &[String],
    weight: Option<&str>,
) -> Option<(u64, String, f64)> {
    let mut best: Option<(f64, u64, String)> = None;
    let mut consider = |edges: Vec<uriel_core::Edge>| {
        for e in edges {
            if e.other != b || !(types.is_empty() || types.contains(&e.rel_type)) {
                continue;
            }
            let w = weight.and_then(|k| numbers(read, e.rel.0, k)).unwrap_or(1.0);
            let better = match &best {
                None => true,
                Some((bw, bid, _)) => w.total_cmp(bw).then(e.rel.0.cmp(bid)).is_lt(),
            };
            if better {
                best = Some((w, e.rel.0, e.rel_type));
            }
        }
    };
    if matches!(dir, Dir::Out | Dir::Both) {
        if let Ok(it) = read.out_edges(a, None) {
            consider(it.filter_map(Result::ok).collect());
        }
    }
    if matches!(dir, Dir::In | Dir::Both) {
        if let Ok(it) = read.in_edges(a, None) {
            consider(it.filter_map(Result::ok).collect());
        }
    }
    best.map(|(w, id, t)| (id, t, w))
}

fn reach_count(g: &Projection, start: u32, forward: bool) -> usize {
    let mut seen = vec![false; g.node_count()];
    let mut queue = VecDeque::from([start]);
    seen[start as usize] = true;
    let mut count = 0usize;
    while let Some(u) = queue.pop_front() {
        let next = if forward || !g.is_directed() {
            g.out_neighbors(u)
        } else {
            g.in_neighbors(u)
        };
        for &v in next {
            if !seen[v as usize] {
                seen[v as usize] = true;
                count += 1;
                queue.push_back(v);
            }
        }
    }
    count
}

fn path(ctx: &Context, args: &Json) -> ToolResult {
    let mut allowed = vec!["from", "to", "mode", "weight", "k", "max_hops", "direction"];
    allowed.extend_from_slice(SCOPE_KEYS);
    reject_unknown(args, &allowed)?;
    let scope = Scope::parse(ctx, args)?;
    let mode = choice(args, "mode", &["shortest", "weighted", "k"], "shortest")?;
    let dir = direction(args, Dir::Out)?;
    let weight = match args.get("weight") {
        None | Some(Json::Null) => None,
        Some(Json::String(s)) => {
            crate::memory::quote_ident(s, "weight").map_err(|m| bad_arg("weight", m))?;
            Some(s.clone())
        }
        Some(_) => return Err(bad_arg("weight", "must be a string")),
    };
    match (mode, &weight) {
        ("weighted", None) => return Err(bad_arg("weight", "required for mode weighted")),
        ("shortest", Some(_)) => {
            return Err(bad_arg("weight", "mode shortest counts hops; use mode weighted or k"))
        }
        _ => {}
    }
    let k = ranged(args, "k", 3, 1, MAX_PATHS)?;
    let max_hops = match args.get("max_hops") {
        None | Some(Json::Null) => None,
        Some(_) => Some(ranged(args, "max_hops", 1, 1, 1_000_000)?),
    };
    let read = snapshot(ctx)?;
    let from = resolve(&read, args.get("from").unwrap_or(&Json::Null), "from")?;
    let to = resolve(&read, args.get("to").unwrap_or(&Json::Null), "to")?;
    let p = project_in(&read, &scope, weight.as_deref())?;
    let outside = |which: &str, id: NodeId| {
        crate::graph_project::not_found(
            which,
            json!({"id": id.0, "reason": "node is outside the labels/max_nodes selection"}),
        )
    };
    let s = p.index_of(from).ok_or_else(|| outside("from", from))?;
    let t = p.index_of(to).ok_or_else(|| outside("to", to))?;
    let weighted = weight.is_some();
    let g = build(&p, dir.orientation(), weighted)?;

    let mut found: Vec<(Vec<u32>, f64)> = Vec::new();
    let mut reason = "unreachable";
    match mode {
        "shortest" => {
            let tr = bfs(&g, s).map_err(algo_err)?;
            if let Some(d) = tr.depth[t as usize] {
                if max_hops.is_none_or(|m| d as usize <= m) {
                    let mut nodes = vec![t];
                    while let Some(prev) = tr.parent[*nodes.last().unwrap_or(&t) as usize] {
                        nodes.push(prev);
                    }
                    nodes.reverse();
                    found.push((nodes, f64::from(d)));
                } else {
                    reason = "max_hops";
                }
            }
        }
        "weighted" => {
            if let Some(path) = dijkstra_path(&g, s, t).map_err(algo_err)? {
                if max_hops.is_none_or(|m| path.nodes.len() - 1 <= m) {
                    found.push((path.nodes, path.cost));
                } else {
                    reason = "max_hops";
                }
            }
        }
        _ => {
            let all = yen_k_shortest_paths(&g, s, t, k).map_err(algo_err)?;
            let total = all.len();
            for path in all {
                if max_hops.is_none_or(|m| path.nodes.len() - 1 <= m) {
                    found.push((path.nodes, path.cost));
                }
            }
            if found.is_empty() && total > 0 {
                reason = "max_hops";
            }
        }
    }

    let mut out = Out::default();
    let (wf, wt) = (who(&read, from), who(&read, to));
    out.put("result", if found.is_empty() { "no_path" } else { "path" });
    out.put("mode", mode);
    out.put("direction", dir.name());
    head_counts(&mut out, p.ids.len(), p.edges.len());
    out.put("from_id", wf.id);
    out.put("from_name", wf.name);
    out.put("to_id", wt.id);
    out.put("to_name", wt.name);
    let mut paths = Tab::new("paths", &["path", "hops", "cost"]);
    let mut steps = Tab::new(
        "steps",
        &["path", "step", "id", "label", "name", "rel_id", "rel_type", "weight"],
    );
    if found.is_empty() {
        out.put("reason", reason);
        out.put("reach_from", reach_count(&g, s, true));
        out.put("reach_to", reach_count(&g, t, false));
    } else {
        out.put("path_count", found.len());
        out.put("hops", found[0].0.len() - 1);
        out.put("cost", r6(found[0].1));
    }
    for (pi, (nodes, cost)) in found.iter().enumerate() {
        paths.rows.push(vec![json!(pi + 1), json!(nodes.len() - 1), r6(*cost)]);
        for (si, &ix) in nodes.iter().enumerate() {
            let id = p.ids[ix as usize];
            let w = who(&read, id);
            let [wid, label, name] = w.cells();
            let via = (si > 0)
                .then(|| {
                    edge_between(&read, p.ids[nodes[si - 1] as usize], id, dir, &scope.rel_types, weight.as_deref())
                })
                .flatten();
            let (rid, rty, rw) = match via {
                Some((rid, rty, rw)) => (json!(rid), json!(rty), r6(rw)),
                None => (Json::Null, Json::Null, Json::Null),
            };
            steps.rows.push(vec![json!(pi + 1), json!(si), wid, label, name, rid, rty, rw]);
        }
    }
    out.tabs.push(paths);
    out.tabs.push(steps);
    Ok(out.render(scope.format))
}

fn components_without(g: &Projection, removed: u32) -> (usize, usize) {
    let n = g.node_count();
    let mut seen = vec![false; n];
    seen[removed as usize] = true;
    let (mut comps, mut largest) = (0usize, 0usize);
    for s in 0..n as u32 {
        if seen[s as usize] {
            continue;
        }
        comps += 1;
        let mut size = 0usize;
        let mut stack = vec![s];
        seen[s as usize] = true;
        while let Some(u) = stack.pop() {
            size += 1;
            for (v, _) in g.neighbors_any(u) {
                if !seen[v as usize] {
                    seen[v as usize] = true;
                    stack.push(v);
                }
            }
        }
        largest = largest.max(size);
    }
    (comps, largest)
}

const CRITICAL_ROWS: usize = 20;

fn impact(ctx: &Context, args: &Json) -> ToolResult {
    let mut allowed = vec!["target", "direction", "depth", "k"];
    allowed.extend_from_slice(SCOPE_KEYS);
    reject_unknown(args, &allowed)?;
    let scope = Scope::parse(ctx, args)?;
    let flow = choice(args, "direction", &["dependents", "dependencies", "both"], "dependents")?;
    let depth = ranged(args, "depth", 3, 1, 64)?;
    let k = ranged(args, "k", 10, 1, MAX_K)?;
    let read = snapshot(ctx)?;
    let target = resolve(&read, args.get("target").unwrap_or(&Json::Null), "target")?;
    let p = project_in(&read, &scope, None)?;
    let ti = p.index_of(target).ok_or_else(|| {
        crate::graph_project::not_found(
            "target",
            json!({"id": target.0, "reason": "node is outside the labels/max_nodes selection"}),
        )
    })?;
    let g = build(&p, Orientation::Natural, false)?;
    let n = g.node_count();

    let mut dist = vec![u32::MAX; n];
    dist[ti as usize] = 0;
    let mut queue = VecDeque::from([ti]);
    while let Some(u) = queue.pop_front() {
        let du = dist[u as usize];
        let up: &[u32] = if flow == "dependencies" { &[] } else { g.in_neighbors(u) };
        let down: &[u32] = if flow == "dependents" { &[] } else { g.out_neighbors(u) };
        for &v in up.iter().chain(down) {
            if dist[v as usize] == u32::MAX {
                dist[v as usize] = du + 1;
                queue.push_back(v);
            }
        }
    }
    let mut layer_counts = vec![0usize; depth + 1];
    let mut unbounded = 0usize;
    for (v, &d) in dist.iter().enumerate() {
        if v as u32 == ti || d == u32::MAX {
            continue;
        }
        unbounded += 1;
        if d as usize <= depth {
            layer_counts[d as usize] += 1;
        }
    }
    let reach: usize = layer_counts.iter().sum();
    let pct = |x: usize| if n > 1 { (x as f64 * 10_000.0 / (n - 1) as f64).round() / 100.0 } else { 0.0 };

    let pr = pagerank(&g, &PageRankConfig::default()).map_err(algo_err)?;
    let mut scores = vec![f64::NEG_INFINITY; n];
    for (v, &d) in dist.iter().enumerate() {
        if v as u32 != ti && d != u32::MAX && d as usize <= depth {
            scores[v] = pr.scores[v] * n as f64 / f64::from(d);
        }
    }
    let mut ranked = Tab::new("affected", &["id", "label", "name", "depth", "score"]);
    for v in top_by_score(&scores, k) {
        if scores[v] == f64::NEG_INFINITY {
            break;
        }
        let w = who(&read, p.ids[v]);
        let [id, label, name] = w.cells();
        ranked.rows.push(vec![id, label, name, json!(dist[v]), r4(scores[v])]);
    }

    let members: Vec<u32> = (0..n as u32)
        .filter(|&v| dist[v as usize] != u32::MAX && dist[v as usize] as usize <= depth)
        .collect();
    let local = |v: u32| members.binary_search(&v).ok().map(|i| i as u32);
    let mut sub_edges: Vec<(u32, u32)> = Vec::new();
    for &(a, b, _) in &p.edges {
        if a == b {
            continue;
        }
        if let (Some(x), Some(y)) = (local(a), local(b)) {
            sub_edges.push((x, y));
        }
    }
    let sub = Projection::from_edges(members.len(), &sub_edges, false).map_err(algo_err)?;
    let aps = articulation_points(&sub);
    let brs = bridges(&sub);
    let mut critical = Tab::new(
        "critical",
        &["kind", "id", "label", "name", "other_id", "other_name", "splits_into", "largest_part"],
    );
    for &a in aps.iter().take(CRITICAL_ROWS) {
        let w = who(&read, p.ids[members[a as usize] as usize]);
        let [id, label, name] = w.cells();
        let (parts, largest) = components_without(&sub, a);
        critical.rows.push(vec![
            json!("articulation"), id, label, name, Json::Null, Json::Null, json!(parts), json!(largest),
        ]);
    }
    for &(a, b) in brs.iter().take(CRITICAL_ROWS) {
        let wa = who(&read, p.ids[members[a as usize] as usize]);
        let wb = who(&read, p.ids[members[b as usize] as usize]);
        critical.rows.push(vec![
            json!("bridge"), json!(wa.id), json!(wa.label), json!(wa.name), json!(wb.id), json!(wb.name),
            json!(2), Json::Null,
        ]);
    }

    let mut layers = Tab::new("layers", &["depth", "count"]);
    for (d, c) in layer_counts.iter().enumerate().skip(1) {
        layers.rows.push(vec![json!(d), json!(c)]);
    }
    let tw = who(&read, target);
    let mut out = Out::default();
    out.put("target_id", tw.id);
    out.put("target_name", tw.name);
    out.put("direction", flow);
    out.put("depth", depth);
    head_counts(&mut out, n, p.edges.len());
    out.put("reach", reach);
    out.put("reach_pct", pct(reach));
    out.put("reach_unbounded", unbounded);
    out.put("reach_unbounded_pct", pct(unbounded));
    out.put("articulation_points", aps.len());
    out.put("bridges", brs.len());
    out.tabs.push(layers);
    out.tabs.push(ranked);
    out.tabs.push(critical);
    Ok(out.render(scope.format))
}
