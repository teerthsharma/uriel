use serde_json::{json, Map, Value as Json};
use uriel_algo::{Orientation, Projection};
use uriel_core::{Node, NodeId, ReadTx, Value};
use uriel_query::project::{project, ProjectError, Projected, Spec};

use crate::compact::{encode_table, Table};
use crate::config::Format;
use crate::memory::quote_ident;
use crate::tools::{format_arg, opt_strings, opt_usize, Context};

pub const DEFAULT_MAX_NODES: usize = 50_000;
pub const HARD_MAX_NODES: usize = 500_000;
pub const MAX_EDGES: usize = 4_000_000;
pub const MAX_K: usize = 200;
const NAME_KEYS: &[&str] = &["name", "title", "path", "file", "id", "key"];

pub fn typed(kind: &str, fields: Json) -> String {
    let mut m = Map::new();
    m.insert("error".into(), Json::String(kind.into()));
    if let Json::Object(extra) = fields {
        m.extend(extra);
    }
    Json::Object(m).to_string()
}

pub fn bad_arg(arg: &str, message: impl std::fmt::Display) -> String {
    typed("bad_argument", json!({"argument": arg, "message": message.to_string()}))
}

pub fn not_found(what: &str, detail: Json) -> String {
    typed("not_found", json!({"what": what, "detail": detail}))
}

pub fn reject_unknown(args: &Json, allowed: &[&str]) -> Result<(), String> {
    if let Json::Object(m) = args {
        for k in m.keys() {
            if !allowed.contains(&k.as_str()) {
                return Err(bad_arg(k, format!("unknown argument; allowed: {}", allowed.join(", "))));
            }
        }
    }
    Ok(())
}

pub fn ranged(args: &Json, key: &str, default: usize, lo: usize, hi: usize) -> Result<usize, String> {
    let v = opt_usize(args, key, default).map_err(|m| bad_arg(key, m))?;
    if v < lo || v > hi {
        return Err(bad_arg(key, format!("must be between {lo} and {hi}, got {v}")));
    }
    Ok(v)
}

pub fn choice<'a>(args: &'a Json, key: &str, allowed: &[&'a str], default: &'a str) -> Result<&'a str, String> {
    match args.get(key) {
        None | Some(Json::Null) => Ok(default),
        Some(Json::String(s)) => allowed
            .iter()
            .copied()
            .find(|a| *a == s)
            .ok_or_else(|| bad_arg(key, format!("must be one of {}, got \"{s}\"", allowed.join(", ")))),
        Some(_) => Err(bad_arg(key, "must be a string")),
    }
}

pub fn identifiers(args: &Json, key: &str) -> Result<Vec<String>, String> {
    let items = opt_strings(args, key).map_err(|m| bad_arg(key, m))?;
    for s in &items {
        quote_ident(s, key).map_err(|m| bad_arg(key, m))?;
    }
    Ok(items)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Out,
    In,
    Both,
}

impl Dir {
    pub fn orientation(self) -> Orientation {
        match self {
            Dir::Out => Orientation::Natural,
            Dir::In => Orientation::Reverse,
            Dir::Both => Orientation::Undirected,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Dir::Out => "out",
            Dir::In => "in",
            Dir::Both => "both",
        }
    }
}

pub fn direction(args: &Json, default: Dir) -> Result<Dir, String> {
    match choice(args, "direction", &["out", "in", "both"], default.name())? {
        "in" => Ok(Dir::In),
        "both" => Ok(Dir::Both),
        _ => Ok(Dir::Out),
    }
}

pub struct Scope {
    pub labels: Vec<String>,
    pub rel_types: Vec<String>,
    pub max_nodes: usize,
    pub format: Format,
}

impl Scope {
    pub fn parse(ctx: &Context, args: &Json) -> Result<Scope, String> {
        Ok(Scope {
            labels: identifiers(args, "labels")?,
            rel_types: identifiers(args, "rel_types")?,
            max_nodes: ranged(args, "max_nodes", DEFAULT_MAX_NODES, 1, HARD_MAX_NODES)?,
            format: format_arg(args, ctx.format).map_err(|m| bad_arg("format", m))?,
        })
    }

    pub fn spec<'a>(&'a self, weight: Option<&'a str>) -> Spec<'a> {
        Spec {
            labels: &self.labels,
            rel_types: &self.rel_types,
            weight,
            max_nodes: self.max_nodes,
            max_edges: MAX_EDGES,
        }
    }
}

pub const SCOPE_KEYS: &[&str] = &["labels", "rel_types", "max_nodes", "format"];

pub fn too_large(e: ProjectError, read: &ReadTx, scope: &Scope) -> String {
    match e {
        ProjectError::TooLarge { what, count, limit } => {
            let mut counts: Vec<(String, u64)> = read
                .labels()
                .unwrap_or_default()
                .into_iter()
                .filter_map(|l| read.count_nodes_with_label(&l).ok().map(|c| (l, c)))
                .collect();
            counts.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
            let smaller: Vec<Json> = counts
                .iter()
                .filter(|(_, c)| *c as usize <= limit)
                .rev()
                .take(5)
                .map(|(l, c)| json!({"label": l, "nodes": c}))
                .collect();
            let hint = if what == "nodes" {
                format!(
                    "pass labels (and rel_types) to select a smaller subgraph, or raise max_nodes up to {HARD_MAX_NODES}"
                )
            } else {
                "pass rel_types to select fewer relationship types".to_string()
            };
            typed(
                "too_large",
                json!({
                    "what": what,
                    "count": count,
                    "limit": limit,
                    "max_nodes": scope.max_nodes,
                    "hint": hint,
                    "labels_that_fit": smaller,
                }),
            )
        }
        ProjectError::Core(c) => c.to_string(),
        ProjectError::Algo(a) => a.to_string(),
    }
}

pub fn snapshot(ctx: &Context) -> Result<ReadTx, String> {
    ctx.db()?.engine.graph().begin_read().map_err(|e| e.to_string())
}

pub fn project_in(read: &ReadTx, scope: &Scope, weight: Option<&str>) -> Result<Projected, String> {
    project(read, &scope.spec(weight)).map_err(|e| too_large(e, read, scope))
}

pub fn build(p: &Projected, o: Orientation, weighted: bool) -> Result<Projection, String> {
    p.projection(o, weighted).map_err(|e| match e {
        uriel_algo::Error::NegativeWeight { .. } | uriel_algo::Error::InvalidWeight(_) => {
            typed("bad_weight", json!({"message": e.to_string()}))
        }
        other => other.to_string(),
    })
}

pub fn display_name(node: &Node) -> String {
    for k in NAME_KEYS {
        match node.props.get(*k) {
            Some(Value::String(s)) if !s.is_empty() => return s.clone(),
            Some(Value::Int(i)) => return i.to_string(),
            _ => {}
        }
    }
    String::new()
}

pub struct Who {
    pub id: u64,
    pub label: String,
    pub name: String,
}

impl Who {
    pub fn cells(&self) -> [Json; 3] {
        [json!(self.id), json!(self.label), json!(self.name)]
    }
}

pub fn who(read: &ReadTx, id: NodeId) -> Who {
    match read.get_node(id) {
        Ok(Some(n)) => Who {
            id: id.0,
            label: n.labels.iter().cloned().collect::<Vec<_>>().join(":"),
            name: display_name(&n),
        },
        _ => Who {
            id: id.0,
            label: String::new(),
            name: String::new(),
        },
    }
}

pub fn r6(x: f64) -> Json {
    if x.is_finite() {
        json!((x * 1e6).round() / 1e6)
    } else {
        Json::Null
    }
}

pub fn r4(x: f64) -> Json {
    if x.is_finite() {
        json!((x * 1e4).round() / 1e4)
    } else {
        Json::Null
    }
}

pub struct Tab {
    pub name: &'static str,
    pub cols: Vec<&'static str>,
    pub rows: Vec<Vec<Json>>,
}

impl Tab {
    pub fn new(name: &'static str, cols: &[&'static str]) -> Tab {
        Tab {
            name,
            cols: cols.to_vec(),
            rows: Vec::new(),
        }
    }

    fn table(&self) -> Table {
        Table {
            columns: self.cols.iter().map(|c| c.to_string()).collect(),
            rows: self.rows.clone(),
        }
    }

    fn json_rows(&self) -> Json {
        Json::Array(
            self.rows
                .iter()
                .map(|r| {
                    Json::Object(
                        self.cols
                            .iter()
                            .zip(r)
                            .map(|(c, v)| (c.to_string(), v.clone()))
                            .collect(),
                    )
                })
                .collect(),
        )
    }
}

#[derive(Default)]
pub struct Out {
    pub head: Vec<(&'static str, Json)>,
    pub tabs: Vec<Tab>,
}

impl Out {
    pub fn put(&mut self, key: &'static str, v: impl Into<Json>) {
        self.head.push((key, v.into()));
    }

    pub fn render(&self, format: Format) -> String {
        match format {
            Format::Json => {
                let mut m = Map::new();
                for (k, v) in &self.head {
                    m.insert((*k).into(), v.clone());
                }
                for t in &self.tabs {
                    m.insert(t.name.into(), t.json_rows());
                }
                Json::Object(m).to_string()
            }
            Format::Compact => {
                let head = Table {
                    columns: self.head.iter().map(|(k, _)| k.to_string()).collect(),
                    rows: vec![self.head.iter().map(|(_, v)| v.clone()).collect()],
                };
                let mut text = encode_table(&head);
                for t in &self.tabs {
                    text.push_str("\n## ");
                    text.push_str(t.name);
                    text.push('\n');
                    text.push_str(&encode_table(&t.table()));
                }
                text
            }
        }
    }
}

pub fn resolve(read: &ReadTx, spec: &Json, which: &str) -> Result<NodeId, String> {
    match spec {
        Json::Number(n) => {
            let id = n
                .as_u64()
                .ok_or_else(|| bad_arg(which, "a node id must be a non-negative integer"))?;
            exists(read, NodeId(id), which)
        }
        Json::String(name) if !name.is_empty() => {
            let hits = matching(read, "Memory", "name", &Value::String(name.clone()))?;
            one(hits, which, json!({"memory_entity": name}))
        }
        Json::Object(m) => {
            for k in m.keys() {
                if !["id", "label", "key", "value"].contains(&k.as_str()) {
                    return Err(bad_arg(which, format!("unknown field '{k}'; use id, or label+key+value")));
                }
            }
            if let Some(id) = m.get("id") {
                let id = id
                    .as_u64()
                    .ok_or_else(|| bad_arg(which, "'id' must be a non-negative integer"))?;
                return exists(read, NodeId(id), which);
            }
            let field = |k: &str| {
                m.get(k)
                    .ok_or_else(|| bad_arg(which, format!("matcher needs '{k}' (or give 'id')")))
            };
            let label = field("label")?
                .as_str()
                .ok_or_else(|| bad_arg(which, "'label' must be a string"))?;
            let key = field("key")?
                .as_str()
                .ok_or_else(|| bad_arg(which, "'key' must be a string"))?;
            quote_ident(label, "label").map_err(|e| bad_arg(which, e))?;
            quote_ident(key, "key").map_err(|e| bad_arg(which, e))?;
            let value = Value::from_json(field("value")?).map_err(|e| bad_arg(which, e))?;
            let hits = matching(read, label, key, &value)?;
            one(hits, which, json!({"label": label, "key": key, "value": value.to_json()}))
        }
        _ => Err(bad_arg(
            which,
            "give a node id, a Memory entity name, or {label, key, value}",
        )),
    }
}

fn exists(read: &ReadTx, id: NodeId, which: &str) -> Result<NodeId, String> {
    match read.get_node(id) {
        Ok(Some(_)) => Ok(id),
        Ok(None) => Err(not_found(which, json!({"id": id.0}))),
        Err(e) => Err(e.to_string()),
    }
}

fn matching(read: &ReadTx, label: &str, key: &str, value: &Value) -> Result<Vec<NodeId>, String> {
    let mut hits = Vec::new();
    for n in read.nodes_by_prop(label, key, value).map_err(|e| e.to_string())? {
        hits.push(n.map_err(|e| e.to_string())?.id);
        if hits.len() > 5 {
            break;
        }
    }
    Ok(hits)
}

fn one(hits: Vec<NodeId>, which: &str, matcher: Json) -> Result<NodeId, String> {
    match hits.as_slice() {
        [] => Err(not_found(which, matcher)),
        [id] => Ok(*id),
        many => Err(typed(
            "ambiguous",
            json!({
                "what": which,
                "detail": matcher,
                "matches": many.iter().map(|i| i.0).collect::<Vec<_>>(),
                "hint": "give a node id or a more specific matcher"
            }),
        )),
    }
}

pub fn top_by_score(scores: &[f64], k: usize) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..scores.len()).collect();
    idx.sort_by(|&a, &b| scores[b].total_cmp(&scores[a]).then(a.cmp(&b)));
    idx.truncate(k);
    idx
}
