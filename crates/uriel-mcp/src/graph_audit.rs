use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};

use serde_json::{json, Value as Json};
use uriel_algo::{
    betweenness_centrality, component_sizes, condensation_layers, feedback_arc_set, louvain,
    pagerank, strongly_connected_components, weakly_connected_components, BetweennessConfig,
    LouvainConfig, Orientation, PageRankConfig, Projection,
};
use uriel_core::{NodeId, ReadTx, Value};
use uriel_query::project::{select_nodes, ProjectError};

use crate::graph_project::{
    bad_arg, build, project_in, r6, ranged, reject_unknown, snapshot, too_large, top_by_score, who, Out, Scope, Tab, MAX_EDGES, MAX_K, SCOPE_KEYS,
};
use crate::graph_tools::{BETWEENNESS_SAMPLE, EXACT_BETWEENNESS_NODES};
use crate::memory::{quote_ident, MEMORY_LABEL};
use crate::tools::ToolResult;
use crate::tools::Context;

const MEMBERS_SHOWN: usize = 25;
const TOP_MEMBERS: usize = 5;
const FEEDBACK_ROWS: usize = 200;
const SELF_LOOP_ROWS: usize = 50;
const LAYER_ROWS: usize = 100;
const CUT_EDGE_ROWS: usize = 50;
const BRIDGE_NODE_ROWS: usize = 10;

fn algo_err(e: uriel_algo::Error) -> String {
    e.to_string()
}

pub fn cycles(ctx: &Context, args: &Json) -> ToolResult {
    let mut allowed = vec!["k"];
    allowed.extend_from_slice(SCOPE_KEYS);
    reject_unknown(args, &allowed)?;
    let scope = Scope::parse(ctx, args)?;
    let k = ranged(args, "k", 20, 1, MAX_K)?;
    let read = snapshot(ctx)?;
    let p = project_in(&read, &scope, None)?;
    let g = build(&p, Orientation::Natural, false)?;
    let n = g.node_count();
    let scc = strongly_connected_components(&g);
    let mut cyc: Vec<(u32, usize)> = component_sizes(&scc).into_iter().filter(|c| c.1 > 1).collect();
    cyc.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let cyclic_nodes: usize = cyc.iter().map(|c| c.1).sum();
    let listed: Vec<(u32, usize)> = cyc.iter().copied().take(k).collect();
    let mut members: HashMap<u32, Vec<u32>> = listed.iter().map(|c| (c.0, Vec::new())).collect();
    for u in 0..n as u32 {
        if let Some(list) = members.get_mut(&scc[u as usize]) {
            list.push(u);
        }
    }
    let loops: Vec<u32> = (0..n as u32).filter(|&u| g.has_edge(u, u)).collect();
    let fas = feedback_arc_set(&g).map_err(algo_err)?;
    let layer_of = condensation_layers(&g).map_err(algo_err)?;

    let mut cycles = Tab::new("cycles", &["scc", "size", "members", "names"]);
    for (label, size) in &listed {
        let ids = &members[label];
        let shown = ids.iter().take(MEMBERS_SHOWN);
        let (mut mids, mut names) = (Vec::new(), Vec::new());
        for &u in shown {
            let w = who(&read, p.ids[u as usize]);
            mids.push(json!(w.id));
            names.push(json!(w.name));
        }
        cycles
            .rows
            .push(vec![json!(p.ids[*label as usize].0), json!(size), json!(mids), json!(names)]);
    }
    let mut self_loops = Tab::new("self_loops", &["id", "label", "name"]);
    for &u in loops.iter().take(SELF_LOOP_ROWS) {
        self_loops.rows.push(who(&read, p.ids[u as usize]).cells().to_vec());
    }
    let mut feedback = Tab::new("feedback_arcs", &["kind", "from_id", "from_name", "to_id", "to_name", "scc"]);
    for &(u, v) in fas.iter().take(FEEDBACK_ROWS) {
        let (wu, wv) = (who(&read, p.ids[u as usize]), who(&read, p.ids[v as usize]));
        let kind = if u == v { "self_loop" } else { "cycle" };
        feedback.rows.push(vec![
            json!(kind),
            json!(wu.id),
            json!(wu.name),
            json!(wv.id),
            json!(wv.name),
            json!(p.ids[scc[u as usize] as usize].0),
        ]);
    }
    let depth = layer_of.iter().copied().max().map_or(0, |m| m as usize + 1);
    let mut per_layer = vec![(0usize, 0usize, Vec::<u64>::new()); depth];
    for u in 0..n {
        let l = &mut per_layer[layer_of[u] as usize];
        l.0 += 1;
        if scc[u] == u as u32 {
            l.1 += 1;
        }
        if l.2.len() < TOP_MEMBERS {
            l.2.push(p.ids[u].0);
        }
    }
    let mut layers = Tab::new("layers", &["layer", "nodes", "components", "sample"]);
    for (i, (nodes, comps, sample)) in per_layer.into_iter().enumerate().take(LAYER_ROWS) {
        layers.rows.push(vec![json!(i), json!(nodes), json!(comps), json!(sample)]);
    }

    let mut out = Out::default();
    out.put("n", n);
    out.put("edges", p.edges.len());
    out.put("acyclic", cyc.is_empty() && loops.is_empty());
    out.put("cycle_count", cyc.len());
    out.put("cyclic_nodes", cyclic_nodes);
    out.put("self_loop_count", loops.len());
    out.put("feedback_arc_count", fas.len());
    out.put("layer_count", depth);
    out.tabs.extend([cycles, self_loops, feedback, layers]);
    Ok(out.render(scope.format))
}

fn pagerank_scores(g: &Projection) -> Result<Vec<f64>, String> {
    Ok(pagerank(g, &PageRankConfig::default()).map_err(algo_err)?.scores)
}

pub fn communities(ctx: &Context, args: &Json) -> ToolResult {
    let mut allowed = vec!["resolution", "k"];
    allowed.extend_from_slice(SCOPE_KEYS);
    reject_unknown(args, &allowed)?;
    let scope = Scope::parse(ctx, args)?;
    let k = ranged(args, "k", 10, 1, MAX_K)?;
    let resolution = match args.get("resolution") {
        None | Some(Json::Null) => 1.0,
        Some(v) => match v.as_f64() {
            Some(r) if r > 0.0 && r <= 100.0 => r,
            _ => return Err(bad_arg("resolution", "must be a number in (0, 100]")),
        },
    };
    let read = snapshot(ctx)?;
    let p = project_in(&read, &scope, None)?;
    let n = p.ids.len();
    let g = build(&p, Orientation::Undirected, false)?;
    let mut out = Out::default();
    out.put("n", n);
    out.put("edges", p.edges.len());
    out.put("resolution", resolution);
    if n == 0 {
        out.put("modularity", 0.0);
        out.put("community_count", 0);
        out.put("cut_edge_count", 0);
        out.tabs.extend([
            Tab::new("communities", &["community", "size", "members", "names", "internal_edges", "external_edges", "cohesion"]),
            Tab::new("cut_edges", &["from_id", "from_name", "from_community", "to_id", "to_name", "to_community"]),
            Tab::new("bridge_nodes", &["id", "label", "name", "community", "betweenness", "cross_edges"]),
        ]);
        return Ok(out.render(scope.format));
    }
    let cfg = LouvainConfig {
        resolution,
        ..LouvainConfig::default()
    };
    let lv = louvain(&g, &cfg).map_err(algo_err)?;
    let comm = &lv.communities;
    let count = comm.iter().copied().max().map_or(0, |m| m as usize + 1);
    let pr = pagerank_scores(&g)?;
    let mut size = vec![0usize; count];
    let mut internal = vec![0usize; count];
    let mut external = vec![0usize; count];
    for &c in comm {
        size[c as usize] += 1;
    }
    let mut cut: Vec<(u32, u32)> = Vec::new();
    for &(a, b, _) in &p.edges {
        let (ca, cb) = (comm[a as usize], comm[b as usize]);
        if a == b || ca == cb {
            internal[ca as usize] += 1;
        } else {
            external[ca as usize] += 1;
            external[cb as usize] += 1;
            cut.push((a.min(b), a.max(b)));
        }
    }
    cut.sort_unstable();
    cut.dedup();
    let mut tops: Vec<Vec<u32>> = vec![Vec::new(); count];
    for v in top_by_score(&pr, n) {
        let list = &mut tops[comm[v] as usize];
        if list.len() < TOP_MEMBERS {
            list.push(v as u32);
        }
    }
    let mut order: Vec<usize> = (0..count).collect();
    order.sort_by(|&a, &b| size[b].cmp(&size[a]).then(a.cmp(&b)));
    let mut table = Tab::new(
        "communities",
        &["community", "size", "members", "names", "internal_edges", "external_edges", "cohesion"],
    );
    for &c in order.iter().take(k) {
        let (mut ids, mut names) = (Vec::new(), Vec::new());
        for &v in &tops[c] {
            let w = who(&read, p.ids[v as usize]);
            ids.push(json!(w.id));
            names.push(json!(w.name));
        }
        let total = internal[c] + external[c];
        let cohesion = if total == 0 { Json::Null } else { r6(internal[c] as f64 / total as f64) };
        table.rows.push(vec![
            json!(c),
            json!(size[c]),
            json!(ids),
            json!(names),
            json!(internal[c]),
            json!(external[c]),
            cohesion,
        ]);
    }
    let mut cut_tab = Tab::new(
        "cut_edges",
        &["from_id", "from_name", "from_community", "to_id", "to_name", "to_community"],
    );
    for &(a, b) in cut.iter().take(CUT_EDGE_ROWS) {
        let (wa, wb) = (who(&read, p.ids[a as usize]), who(&read, p.ids[b as usize]));
        cut_tab.rows.push(vec![
            json!(wa.id),
            json!(wa.name),
            json!(comm[a as usize]),
            json!(wb.id),
            json!(wb.name),
            json!(comm[b as usize]),
        ]);
    }
    let mut cross = vec![0usize; n];
    for &(a, b) in &cut {
        cross[a as usize] += 1;
        cross[b as usize] += 1;
    }
    let mut bridge_tab = Tab::new(
        "bridge_nodes",
        &["id", "label", "name", "community", "betweenness", "cross_edges"],
    );
    let sampled = n > EXACT_BETWEENNESS_NODES;
    if !cut.is_empty() {
        let bc = betweenness_centrality(
            &g,
            &BetweennessConfig {
                normalize: false,
                sample_size: sampled.then_some(BETWEENNESS_SAMPLE),
                seed: 0,
            },
        )
        .map_err(algo_err)?;
        let boundary: Vec<f64> = (0..n)
            .map(|v| if cross[v] > 0 { bc[v] } else { f64::NEG_INFINITY })
            .collect();
        for v in top_by_score(&boundary, BRIDGE_NODE_ROWS) {
            if boundary[v] == f64::NEG_INFINITY {
                break;
            }
            let w = who(&read, p.ids[v]);
            let [id, label, name] = w.cells();
            bridge_tab
                .rows
                .push(vec![id, label, name, json!(comm[v]), r6(bc[v]), json!(cross[v])]);
        }
    }
    out.put("modularity", r6(lv.modularity));
    out.put("community_count", count);
    out.put("levels", lv.levels);
    out.put("cut_edge_count", cut.len());
    out.put("betweenness_sampled", sampled && !cut.is_empty());
    out.tabs.extend([table, cut_tab, bridge_tab]);
    Ok(out.render(scope.format))
}

#[derive(Default)]
struct LabelStat {
    total: usize,
    keys: BTreeMap<String, usize>,
}

impl LabelStat {
    fn common(&self) -> Option<(&str, usize)> {
        let mut best: Option<(&str, usize)> = None;
        for (k, &c) in &self.keys {
            if best.is_none_or(|b| c > b.1) {
                best = Some((k.as_str(), c));
            }
        }
        best
    }
}

struct Finding {
    severity: u8,
    kind: &'static str,
    label: Option<String>,
    detail: String,
    count: usize,
    rate: Option<f64>,
    sample: Vec<u64>,
    fix: Option<String>,
}

fn severity_name(s: u8) -> &'static str {
    ["error", "warn", "info"][s as usize]
}

fn id_list(ids: &[u64]) -> String {
    ids.iter().map(u64::to_string).collect::<Vec<_>>().join(", ")
}

fn name_hash(s: &str) -> u64 {
    let mut h = DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

fn lower_name(props: &std::collections::BTreeMap<String, Value>) -> Option<String> {
    match props.get("name") {
        Some(Value::String(s)) => Some(s.to_lowercase()),
        _ => None,
    }
}

fn scan(read: &ReadTx, id: NodeId) -> Result<Option<uriel_core::Node>, String> {
    read.scan_node(id).map_err(|e| e.to_string())
}

pub fn lint(ctx: &Context, args: &Json) -> ToolResult {
    let mut allowed = vec!["k"];
    allowed.extend_from_slice(SCOPE_KEYS);
    reject_unknown(args, &allowed)?;
    let scope = Scope::parse(ctx, args)?;
    let k = ranged(args, "k", 10, 1, 100)?;
    let read = snapshot(ctx)?;
    let ids = select_nodes(&read, &scope.spec(None)).map_err(|e| too_large(e, &read, &scope))?;
    let n = ids.len();
    let mut deg = vec![0u32; n];
    let mut edges: Vec<(u32, u32)> = Vec::new();
    let mut types: BTreeMap<String, u64> = BTreeMap::new();
    for r in read.all_rels().map_err(|e| e.to_string())? {
        let r = r.map_err(|e| e.to_string())?;
        if !scope.rel_types.is_empty() && !scope.rel_types.contains(&r.rel_type) {
            continue;
        }
        let (Ok(a), Ok(b)) = (ids.binary_search(&r.src), ids.binary_search(&r.dst)) else {
            continue;
        };
        if edges.len() >= MAX_EDGES {
            let e = ProjectError::TooLarge {
                what: "edges",
                count: edges.len() as u64 + 1,
                limit: MAX_EDGES,
            };
            return Err(too_large(e, &read, &scope));
        }
        deg[a] += 1;
        if a != b {
            deg[b] += 1;
        }
        *types.entry(r.rel_type).or_default() += 1;
        edges.push((a as u32, b as u32));
    }

    let mut labels: BTreeMap<String, LabelStat> = BTreeMap::new();
    let mut hashes: BTreeMap<String, HashMap<u64, u32>> = BTreeMap::new();
    let (mut no_obs, mut no_obs_sample) = (0usize, Vec::new());
    let (mut no_name, mut no_name_sample) = (0usize, Vec::new());
    for id in &ids {
        let Some(node) = scan(&read, *id)? else { continue };
        let lower = lower_name(&node.props);
        for label in &node.labels {
            let st = labels.entry(label.clone()).or_default();
            st.total += 1;
            for key in node.props.keys() {
                *st.keys.entry(key.clone()).or_default() += 1;
            }
            if let Some(l) = &lower {
                *hashes.entry(label.clone()).or_default().entry(name_hash(l)).or_default() += 1;
            }
        }
        if node.labels.contains(MEMORY_LABEL) {
            if !matches!(node.props.get("observations"), Some(Value::List(l)) if !l.is_empty()) {
                no_obs += 1;
                if no_obs_sample.len() < k {
                    no_obs_sample.push(id.0);
                }
            }
            if !matches!(node.props.get("name"), Some(Value::String(s)) if !s.is_empty()) {
                no_name += 1;
                if no_name_sample.len() < k {
                    no_name_sample.push(id.0);
                }
            }
        }
    }

    let common: BTreeMap<&str, (&str, usize)> = labels
        .iter()
        .filter_map(|(l, st)| st.common().map(|c| (l.as_str(), c)))
        .collect();
    let dup_groups: BTreeMap<&str, (usize, usize)> = hashes
        .iter()
        .filter_map(|(l, h)| {
            let groups = h.values().filter(|&&c| c > 1).count();
            let nodes: usize = h.values().filter(|&&c| c > 1).map(|&c| c as usize).sum();
            (groups > 0).then_some((l.as_str(), (groups, nodes)))
        })
        .collect();
    let mut dup_sample: BTreeMap<String, Vec<u64>> = BTreeMap::new();
    let mut missing_sample: BTreeMap<String, Vec<u64>> = BTreeMap::new();
    let wants_missing: Vec<&str> = common
        .iter()
        .filter(|(l, (_, c))| labels[**l].total > *c)
        .map(|(l, _)| *l)
        .collect();
    if !dup_groups.is_empty() || !wants_missing.is_empty() {
        for id in &ids {
            let Some(node) = scan(&read, *id)? else { continue };
            let lower = lower_name(&node.props);
            for label in &node.labels {
                if dup_groups.contains_key(label.as_str()) {
                    if let Some(l) = &lower {
                        let shared = hashes[label].get(&name_hash(l)).is_some_and(|&c| c > 1);
                        let list = dup_sample.entry(label.clone()).or_default();
                        if shared && list.len() < k {
                            list.push(id.0);
                        }
                    }
                }
                if wants_missing.contains(&label.as_str()) {
                    let key = common[label.as_str()].0;
                    let list = missing_sample.entry(label.clone()).or_default();
                    if !node.props.contains_key(key) && list.len() < k {
                        list.push(id.0);
                    }
                }
            }
        }
    }

    let quoted = |l: &str| quote_ident(l, "label").ok();
    let mut findings: Vec<Finding> = Vec::new();
    if n == 0 {
        findings.push(Finding {
            severity: 2,
            kind: "empty_graph",
            label: None,
            detail: "the selection contains no nodes".into(),
            count: 0,
            rate: None,
            sample: Vec::new(),
            fix: None,
        });
    }
    let orphans: Vec<usize> = (0..n).filter(|&i| deg[i] == 0).collect();
    if !orphans.is_empty() {
        let sample: Vec<u64> = orphans.iter().take(k).map(|&i| ids[i].0).collect();
        findings.push(Finding {
            severity: 1,
            kind: "orphans",
            label: None,
            detail: "nodes without any relationship in the selection".into(),
            count: orphans.len(),
            rate: Some(orphans.len() as f64 / n as f64),
            fix: Some(format!("MATCH (n) WHERE id(n) IN [{}] DETACH DELETE n", id_list(&sample))),
            sample,
        });
    }
    for (label, (groups, nodes)) in &dup_groups {
        let sample = dup_sample.remove(*label).unwrap_or_default();
        findings.push(Finding {
            severity: 1,
            kind: "duplicate_names",
            label: Some((*label).to_string()),
            detail: format!("{groups} names (case-insensitive) shared by {nodes} nodes"),
            count: *groups,
            rate: None,
            fix: quoted(label).map(|q| {
                format!(
                    "MATCH (n:{q}) WHERE id(n) IN [{}] RETURN id(n) AS id, n.name AS name ORDER BY toLower(n.name), id",
                    id_list(&sample)
                )
            }),
            sample,
        });
    }
    for (label, st) in &labels {
        let Some((key, c)) = st.common() else { continue };
        let missing = st.total - c;
        if missing == 0 {
            continue;
        }
        let rate = missing as f64 / st.total as f64;
        let sample = missing_sample.remove(label.as_str()).unwrap_or_default();
        findings.push(Finding {
            severity: if rate >= 0.05 { 1 } else { 2 },
            kind: "missing_property",
            label: Some(label.clone()),
            detail: format!("{missing} of {} nodes lack `{key}`, the most common property of {label}", st.total),
            count: missing,
            rate: Some(rate),
            fix: quoted(label).zip(quote_ident(key, "property").ok()).map(|(q, _)| {
                format!(
                    "MATCH (n:{q}) WHERE id(n) IN [{}] RETURN id(n) AS id, keys(n) AS present, '{}' AS missing_key",
                    id_list(&sample),
                    key.replace('\\', "\\\\").replace('\'', "\\'")
                )
            }),
            sample,
        });
    }
    let total_deg: f64 = deg.iter().map(|&d| f64::from(d)).sum();
    let mean = if n > 0 { total_deg / n as f64 } else { 0.0 };
    let var = if n > 0 {
        deg.iter().map(|&d| (f64::from(d) - mean).powi(2)).sum::<f64>() / n as f64
    } else {
        0.0
    };
    let sigma = var.sqrt();
    let threshold = mean + 3.0 * sigma;
    if sigma > 0.0 {
        let supers: Vec<usize> = (0..n).filter(|&i| f64::from(deg[i]) > threshold).collect();
        if !supers.is_empty() {
            let scores: Vec<f64> = deg.iter().map(|&d| f64::from(d)).collect();
            let sample: Vec<u64> = top_by_score(&scores, k)
                .into_iter()
                .filter(|&i| f64::from(deg[i]) > threshold)
                .map(|i| ids[i].0)
                .collect();
            findings.push(Finding {
                severity: 1,
                kind: "super_nodes",
                label: None,
                detail: format!(
                    "degree above mean + 3 sigma = {:.3} (mean {:.3}, sigma {:.3})",
                    threshold, mean, sigma
                ),
                count: supers.len(),
                rate: None,
                fix: Some(format!(
                    "MATCH (n)-[r]-() WHERE id(n) IN [{}] RETURN id(n) AS id, count(r) AS degree ORDER BY degree DESC",
                    id_list(&sample)
                )),
                sample,
            });
        }
    }
    let g = Projection::from_edges(n, &edges, false).map_err(algo_err)?;
    let wcc = weakly_connected_components(&g);
    let sizes = component_sizes(&wcc);
    let mut hist: BTreeMap<usize, usize> = BTreeMap::new();
    for (_, s) in &sizes {
        *hist.entry(*s).or_default() += 1;
    }
    let nontrivial = sizes.iter().filter(|c| c.1 > 1).count();
    if nontrivial > 1 {
        let largest = sizes.iter().filter(|c| c.1 > 1).max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)));
        let (big_label, big_size) = largest.copied().unwrap_or((0, 0));
        let in_small = |i: usize| {
            let l = wcc[i];
            l != big_label && sizes.binary_search_by_key(&l, |c| c.0).is_ok_and(|ix| sizes[ix].1 > 1)
        };
        let sample: Vec<u64> = (0..n).filter(|&i| in_small(i)).take(k).map(|i| ids[i].0).collect();
        findings.push(Finding {
            severity: 2,
            kind: "fragmented",
            label: None,
            detail: format!("{nontrivial} components with more than one node; the largest has {big_size}"),
            count: nontrivial - 1,
            rate: None,
            fix: Some(format!(
                "MATCH (n) WHERE id(n) IN [{}] RETURN id(n) AS id, labels(n) AS labels, n.name AS name",
                id_list(&sample)
            )),
            sample,
        });
    }
    if no_name > 0 {
        findings.push(Finding {
            severity: 0,
            kind: "memory_missing_name",
            label: Some(MEMORY_LABEL.into()),
            detail: "Memory entities without a string name cannot be found by name".into(),
            count: no_name,
            rate: None,
            fix: Some(format!(
                "MATCH (e:{MEMORY_LABEL}) WHERE id(e) IN [{}] RETURN id(e) AS id, properties(e) AS props",
                id_list(&no_name_sample)
            )),
            sample: no_name_sample,
        });
    }
    if no_obs > 0 {
        findings.push(Finding {
            severity: 1,
            kind: "memory_no_observations",
            label: Some(MEMORY_LABEL.into()),
            detail: "Memory entities with no observations".into(),
            count: no_obs,
            rate: None,
            fix: Some(format!(
                "MATCH (e:{MEMORY_LABEL}) WHERE id(e) IN [{}] RETURN e.name AS name, e.type AS type",
                id_list(&no_obs_sample)
            )),
            sample: no_obs_sample,
        });
    }
    findings.sort_by(|a, b| {
        a.severity
            .cmp(&b.severity)
            .then(a.kind.cmp(b.kind))
            .then(a.label.cmp(&b.label))
    });

    let mut ftab = Tab::new(
        "findings",
        &["severity", "kind", "label", "detail", "count", "rate", "sample", "fix"],
    );
    let mut per_sev = [0usize; 3];
    for f in &findings {
        per_sev[f.severity as usize] += 1;
        ftab.rows.push(vec![
            json!(severity_name(f.severity)),
            json!(f.kind),
            f.label.clone().map_or(Json::Null, Json::from),
            json!(f.detail),
            json!(f.count),
            f.rate.map_or(Json::Null, r6),
            json!(f.sample),
            f.fix.clone().map_or(Json::Null, Json::from),
        ]);
    }
    let mut ltab = Tab::new("labels", &["label", "nodes", "common_property", "missing", "missing_rate"]);
    for (label, st) in &labels {
        let (key, missing) = match st.common() {
            Some((k, c)) => (json!(k), st.total - c),
            None => (Json::Null, 0),
        };
        ltab.rows.push(vec![
            json!(label),
            json!(st.total),
            key,
            json!(missing),
            r6(missing as f64 / st.total as f64),
        ]);
    }
    let mut ttab = Tab::new("rel_types", &["type", "count"]);
    let mut by_count: Vec<(&String, &u64)> = types.iter().collect();
    by_count.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    for (t, c) in by_count.into_iter().take(50) {
        ttab.rows.push(vec![json!(t), json!(c)]);
    }
    let mut ctab = Tab::new("components", &["size", "components"]);
    for (size, count) in hist.iter().rev().take(20) {
        ctab.rows.push(vec![json!(size), json!(count)]);
    }
    let density = if n > 1 { edges.len() as f64 / (n as f64 * (n as f64 - 1.0)) } else { 0.0 };

    let mut out = Out::default();
    out.put("n", n);
    out.put("edges", edges.len());
    out.put("density", r6(density));
    out.put("mean_degree", r6(mean));
    out.put("degree_sigma", r6(sigma));
    out.put("component_count", sizes.len());
    out.put("orphans", orphans.len());
    out.put("finding_count", findings.len());
    out.put("errors", per_sev[0]);
    out.put("warnings", per_sev[1]);
    out.put("infos", per_sev[2]);
    out.tabs.extend([ftab, ltab, ttab, ctab]);
    Ok(out.render(scope.format))
}
