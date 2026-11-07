use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value as Json};

use crate::compact::{encode_table, Table};
use crate::config::{Format, Mode, Terse};
use crate::db::{Db, Open};
use crate::services::Services;
use crate::session::Session;
use crate::placement::PlaceState;
use crate::{agent_tools, dispatch_tools, graph_tools, guard, house_tools, ledger_tools, memory, place_tools, rag_tools, research_tools, soul, terse, view};
use uriel_house::House;
use uriel_query::{AccessMode, QueryStats};
use uriel_rag::Rag;

pub type ToolResult = Result<String, String>;

pub struct Context {
    pub db: Db,
    pub read_only: bool,
    pub mode: Mode,
    pub terse: Terse,
    pub meter: terse::Meter,
    pub services: Services,
    pub format: Format,
    pub session: Session,
    pub guard: guard::Guard,
    pub token_limit: usize,
    pub placement: PlaceState,
    pub agents: agent_tools::Hub,
}

impl Context {
    pub fn db(&self) -> Result<Arc<Open>, String> {
        self.db.get()
    }

    pub fn rag(&self) -> Result<Arc<Rag>, String> {
        let db = self.db()?;
        self.services.rag(&db)
    }

    pub fn house(&self) -> Result<Arc<House>, String> {
        let db = self.db()?;
        self.services.house(&db)
    }
}

pub struct ToolDef {
    pub name: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub writes: bool,
    pub schema: fn() -> Json,
}

pub(crate) fn str_prop(desc: &str) -> Json {
    json!({"type": "string", "description": desc})
}

pub(crate) fn str_list(desc: &str) -> Json {
    json!({"type": "array", "items": {"type": "string"}, "description": desc})
}

pub(crate) fn object(props: Json, required: &[&str]) -> Json {
    json!({
        "type": "object",
        "properties": props,
        "required": required,
        "additionalProperties": false
    })
}

pub(crate) fn format_prop() -> Json {
    json!({"type": "string", "enum": ["compact", "json"],
        "description": "Result encoding: compact (fewer tokens: header once, one line per row) or json (exact v1). Default: the server's --format, compact unless set."})
}

fn cypher_schema() -> Json {
    object(
        json!({
            "query": str_prop("The Cypher query to execute."),
            "params": {
                "type": "object",
                "description": "Parameters to pass to the Cypher query ($name placeholders).",
                "additionalProperties": true
            },
            "format": format_prop()
        }),
        &["query"],
    )
}

pub(crate) fn no_args() -> Json {
    object(json!({}), &[])
}

fn entity_schema() -> Json {
    json!({
        "type": "object",
        "properties": {
            "name": str_prop("Unique name of the entity."),
            "type": str_prop("Type of the entity; becomes a node label."),
            "observations": str_list("Facts about the entity.")
        },
        "required": ["name", "type"],
        "additionalProperties": false
    })
}

fn relation_item() -> Json {
    json!({
        "type": "object",
        "properties": {
            "source": str_prop("Name of the source entity."),
            "target": str_prop("Name of the target entity."),
            "relationType": str_prop("Relationship type.")
        },
        "required": ["source", "target", "relationType"],
        "additionalProperties": false
    })
}

fn observation_item(desc: &str) -> Json {
    json!({
        "type": "object",
        "properties": {
            "entityName": str_prop("Name of the entity."),
            "observations": str_list(desc)
        },
        "required": ["entityName", "observations"],
        "additionalProperties": false
    })
}

fn create_entities_schema() -> Json {
    object(
        json!({"entities": {"type": "array", "items": entity_schema(), "description": "Entities to create or update."}}),
        &["entities"],
    )
}

fn create_relations_schema() -> Json {
    object(
        json!({"relations": {"type": "array", "items": relation_item(), "description": "Relations to create."}}),
        &["relations"],
    )
}

fn add_observations_schema() -> Json {
    object(
        json!({"observations": {"type": "array", "items": observation_item("Observations to add."), "description": "Observations to add per entity."}}),
        &["observations"],
    )
}

fn delete_entities_schema() -> Json {
    object(
        json!({"entityNames": str_list("Names of the entities to delete, with their relations.")}),
        &["entityNames"],
    )
}

fn delete_observations_schema() -> Json {
    object(
        json!({"deletions": {"type": "array", "items": observation_item("Observations to remove."), "description": "Observations to delete per entity."}}),
        &["deletions"],
    )
}

fn delete_relations_schema() -> Json {
    object(
        json!({"relations": {"type": "array", "items": relation_item(), "description": "Relations to delete."}}),
        &["relations"],
    )
}

fn search_schema() -> Json {
    object(
        json!({"query": str_prop("Case-insensitive text matched against entity names, types and observations.")}),
        &["query"],
    )
}

fn find_by_name_schema() -> Json {
    object(
        json!({"names": str_list("Entity names to look up.")}),
        &["names"],
    )
}

pub const TOOLS: &[ToolDef] = &[
    ToolDef {
        name: "get_neo4j_schema",
        title: "Get graph schema",
        description: "Return the graph schema (node labels, relationship types, properties, indexes, constraints) as JSON.",
        writes: false,
        schema: no_args,
    },
    ToolDef {
        name: "read_neo4j_cypher",
        title: "Run read Cypher",
        description: "Execute a read-only Cypher query. Rows come back compact by default: a `#<rows>` line, the column names once, then one tab-separated line per row (optional `@a=prefix` lines abbreviate repeated strings as `@a rest`; empty cell = null; strings that look like numbers are prefixed with '). Pass format=json for the JSON rows. Write clauses are rejected.",
        writes: false,
        schema: cypher_schema,
    },
    ToolDef {
        name: "write_neo4j_cypher",
        title: "Run write Cypher",
        description: "Execute a Cypher query that modifies the graph and return the update counters as JSON.",
        writes: true,
        schema: cypher_schema,
    },
    ToolDef {
        name: "create_entities",
        title: "Create entities",
        description: "Create memory entities (nodes labelled Memory plus their type) with observations. Existing names are updated and observations merged.",
        writes: true,
        schema: create_entities_schema,
    },
    ToolDef {
        name: "create_relations",
        title: "Create relations",
        description: "Create directed relations between existing memory entities.",
        writes: true,
        schema: create_relations_schema,
    },
    ToolDef {
        name: "add_observations",
        title: "Add observations",
        description: "Add observations to existing memory entities.",
        writes: true,
        schema: add_observations_schema,
    },
    ToolDef {
        name: "delete_entities",
        title: "Delete entities",
        description: "Delete memory entities and all their relations.",
        writes: true,
        schema: delete_entities_schema,
    },
    ToolDef {
        name: "delete_observations",
        title: "Delete observations",
        description: "Delete specific observations from memory entities.",
        writes: true,
        schema: delete_observations_schema,
    },
    ToolDef {
        name: "delete_relations",
        title: "Delete relations",
        description: "Delete specific relations between memory entities.",
        writes: true,
        schema: delete_relations_schema,
    },
    ToolDef {
        name: "read_graph",
        title: "Read memory graph",
        description: "Return the whole memory graph: all entities and relations.",
        writes: false,
        schema: no_args,
    },
    ToolDef {
        name: "search_memories",
        title: "Search memories",
        description: "Search memory entities by text in names, types and observations; returns matches and the relations among them.",
        writes: false,
        schema: search_schema,
    },
    ToolDef {
        name: "find_memories_by_name",
        title: "Find memories by name",
        description: "Fetch memory entities by exact name, with the relations among them.",
        writes: false,
        schema: find_by_name_schema,
    },
];

pub fn all() -> impl Iterator<Item = &'static ToolDef> {
    TOOLS
        .iter()
        .chain(rag_tools::TOOLS.iter())
        .chain(house_tools::TOOLS.iter())
        .chain(research_tools::TOOLS.iter())
        .chain(place_tools::TOOLS.iter())
        .chain(view::TOOLS.iter())
        .chain(soul::TOOLS.iter())
        .chain(terse::TOOLS.iter())
        .chain(agent_tools::TOOLS.iter())
        .chain(ledger_tools::TOOLS.iter())
        .chain(guard::TOOLS.iter())
        .chain(dispatch_tools::TOOLS.iter())
        .chain(graph_tools::TOOLS.iter())
}

pub fn house_only(name: &str) -> bool {
    name.starts_with("house_")
        || name.starts_with("research_")
        || name.starts_with("genius_")
        || name.starts_with("dispatch_")
}

pub fn visible(
    read_only: bool,
    mode: Mode,
    terse: Terse,
) -> impl Iterator<Item = &'static ToolDef> {
    all().filter(move |t| {
        !(read_only && t.writes)
            && (mode == Mode::House || !house_only(t.name))
            && (terse != Terse::Off || t.name != terse::STATS)
    })
}

pub fn list_json(read_only: bool, mode: Mode) -> Json {
    list_json_with(read_only, mode, Terse::Off)
}

pub fn list_json_with(read_only: bool, mode: Mode, terse: Terse) -> Json {
    let tools: Vec<Json> = visible(read_only, mode, terse)
        .map(|t| {
            json!({
                "name": t.name,
                "title": t.title,
                "description": t.description,
                "inputSchema": (t.schema)(),
                "annotations": {
                    "title": t.title,
                    "readOnlyHint": !t.writes,
                    "openWorldHint": false
                }
            })
        })
        .collect();
    json!({ "tools": tools })
}

pub fn list_json_for(ctx: &Context) -> Json {
    if !ctx.guard.adaptive() {
        return list_json_with(ctx.read_only, ctx.mode, ctx.terse);
    }
    let tools: Vec<Json> = visible(ctx.read_only, ctx.mode, ctx.terse)
        .filter(|t| ctx.guard.is_listed(t.name))
        .map(|t| {
            json!({
                "name": t.name,
                "title": t.title,
                "description": t.description,
                "inputSchema": slim((t.schema)()),
                "annotations": {"readOnlyHint": !t.writes, "openWorldHint": false}
            })
        })
        .collect();
    json!({ "tools": tools })
}

const LONG_DESCRIPTION: usize = 160;

fn first_sentence(desc: &str) -> String {
    if desc.chars().count() <= LONG_DESCRIPTION {
        return desc.to_string();
    }
    let head: String = desc.chars().take(LONG_DESCRIPTION).collect();
    match head.rfind(". ") {
        Some(i) if i > 20 => head[..=i].to_string(),
        _ => head.trim_end().to_string(),
    }
}

pub(crate) fn slim(mut schema: Json) -> Json {
    match &mut schema {
        Json::Object(map) => {
            if map.get("additionalProperties") == Some(&Json::Bool(false)) {
                map.remove("additionalProperties");
            }
            if map.get("required").and_then(Json::as_array).is_some_and(Vec::is_empty) {
                map.remove("required");
            }
            if let Some(Json::String(d)) = map.get_mut("description") {
                *d = first_sentence(d);
            }
            if let Some(Json::Object(props)) = map.get_mut("properties") {
                if let Some(Json::Object(f)) = props.get_mut("format") {
                    f.insert("description".into(), Json::String("compact (default) or json".into()));
                }
            }
            for v in map.values_mut() {
                *v = slim(std::mem::take(v));
            }
        }
        Json::Array(items) => {
            for v in items {
                *v = slim(std::mem::take(v));
            }
        }
        _ => {}
    }
    schema
}

pub fn call(ctx: &Context, name: &str, args: &Json) -> Option<ToolResult> {
    let outcome = dispatch(ctx, name, args)?;
    Some(outcome.map(|text| terse::shape(ctx, name, text)))
}

fn dispatch(ctx: &Context, name: &str, args: &Json) -> Option<ToolResult> {
    let def = all().find(|t| t.name == name)?;
    if def.name == terse::STATS && ctx.terse == Terse::Off {
        return None;
    }
    if house_only(def.name) && ctx.mode != Mode::House {
        return Some(Err(json!({
            "error": "wrong_mode",
            "tool": name,
            "mode": ctx.mode.name(),
            "requires": "house",
            "hint": "restart the server with --mode house (or URIEL_MODE=house)"
        })
        .to_string()));
    }
    if def.writes && ctx.read_only {
        return Some(Err(format!(
            "tool '{name}' is disabled: the server was started with --read-only"
        )));
    }
    let args = match args {
        Json::Null => &Json::Null,
        Json::Object(_) => args,
        _ => return Some(Err("arguments must be an object".into())),
    };
    Some(match name {
        "get_neo4j_schema" => ctx.db().map(|db| db.engine.schema_json().to_string()),
        "read_neo4j_cypher" => run_cypher(ctx, args, AccessMode::Read),
        "write_neo4j_cypher" => run_cypher(ctx, args, AccessMode::Write),
        n if n.starts_with("rag_") || n.starts_with("seal_") || n.starts_with("session_") => {
            rag_tools::call(ctx, n, args)
        }
        guard::MORE | guard::USAGE | guard::ENABLE => guard::call(ctx, name, args, ctx.token_limit),
        n if n.starts_with("dispatch_") => dispatch_tools::call(ctx, n, args),
        n if n.starts_with("ledger_") => ledger_tools::call(ctx, n, args),
        n if n.starts_with("graph_") => graph_tools::call(ctx, n, args),
        n if n.starts_with("genius_") || n.starts_with("fleet_") => agent_tools::call(ctx, n, args),
        n if n.starts_with("house_") => house_tools::call(ctx, n, args),
        n if n.starts_with("research_") => research_tools::call(ctx, n, args),
        "view" => view::call(ctx, name, args),
        "soul" => soul::call(ctx, name, args),
        terse::STATS => terse::stats(ctx),
        "calibrate" | "why_hot" | "pin" | "unpin" | "replay" | "tier_stats" => {
            place_tools::call(ctx, name, args)
        }
        _ => memory::call(ctx, name, args),
    })
}

pub fn stats_json(s: &QueryStats) -> Json {
    json!({
        "nodes_created": s.nodes_created,
        "nodes_deleted": s.nodes_deleted,
        "relationships_created": s.rels_created,
        "relationships_deleted": s.rels_deleted,
        "properties_set": s.properties_set,
        "labels_added": s.labels_added,
        "labels_removed": s.labels_removed,
        "indexes_added": s.indexes_added,
        "constraints_added": s.constraints_added,
        "from_cache": s.from_cache
    })
}

fn parse_params(args: &Json) -> Result<BTreeMap<String, uriel_core::Value>, String> {
    let mut out = BTreeMap::new();
    match args.get("params") {
        None | Some(Json::Null) => {}
        Some(Json::Object(m)) => {
            for (k, v) in m {
                let v = uriel_core::Value::from_json(v)
                    .map_err(|e| format!("invalid parameter '{k}': {e}"))?;
                out.insert(k.clone(), v);
            }
        }
        Some(_) => return Err("'params' must be an object".into()),
    }
    Ok(out)
}

fn run_cypher(ctx: &Context, args: &Json, mode: AccessMode) -> ToolResult {
    let query = args
        .get("query")
        .and_then(Json::as_str)
        .filter(|q| !q.trim().is_empty())
        .ok_or_else(|| "'query' is required and must be a non-empty string".to_string())?;
    let params = parse_params(args)?;
    let result = ctx
        .db()?
        .engine
        .execute(query, &params, mode)
        .map_err(|e| e.to_string())?;
    Ok(match mode {
        AccessMode::Read => {
            let rows = result.to_json_rows();
            ctx.placement.observe_rows(&rows);
            let (rows, omitted) = cap_rows(rows);
            let note = format!(
                "+{omitted} rows not shown (cap {}); add LIMIT or SKIP",
                guard::MAX_ROWS
            );
            match (format_arg(args, ctx.format)?, &rows) {
                (Format::Compact, Json::Array(items)) => {
                    let body = match Table::from_rows(items) {
                        Some(table) => encode_table(&table),
                        None => rows.to_string(),
                    };
                    if omitted > 0 {
                        format!("{body}\n{note}")
                    } else {
                        body
                    }
                }
                _ if omitted > 0 => json!({"rows": rows, "omitted": omitted, "note": note}).to_string(),
                _ => rows.to_string(),
            }
        }
        AccessMode::Write => stats_json(&result.stats).to_string(),
    })
}

fn cap_rows(rows: Json) -> (Json, usize) {
    match rows {
        Json::Array(mut items) if items.len() > guard::MAX_ROWS => {
            let omitted = items.len() - guard::MAX_ROWS;
            items.truncate(guard::MAX_ROWS);
            (Json::Array(items), omitted)
        }
        other => (other, 0),
    }
}

pub(crate) fn format_arg(args: &Json, default: Format) -> Result<Format, String> {
    match args.get("format") {
        None | Some(Json::Null) => Ok(default),
        Some(Json::String(s)) => Format::parse(s)
            .ok_or_else(|| format!("'format' must be \"compact\" or \"json\", got \"{s}\"")),
        Some(_) => Err("'format' must be \"compact\" or \"json\"".into()),
    }
}

pub(crate) fn opt_bool(args: &Json, key: &str) -> Result<Option<bool>, String> {
    match args.get(key) {
        None | Some(Json::Null) => Ok(None),
        Some(Json::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(format!("'{key}' must be a boolean")),
    }
}

pub fn read_rows(
    ctx: &Context,
    query: &str,
    params: &BTreeMap<String, uriel_core::Value>,
) -> Result<Vec<Json>, String> {
    let r = ctx
        .db()?
        .engine
        .execute(query, params, AccessMode::Read)
        .map_err(|e| e.to_string())?;
    match r.to_json_rows() {
        Json::Array(rows) => Ok(rows),
        other => Err(format!("unexpected row encoding: {other}")),
    }
}

pub fn write_query(
    ctx: &Context,
    query: &str,
    params: &BTreeMap<String, uriel_core::Value>,
) -> Result<QueryStats, String> {
    ctx.db()?
        .engine
        .execute(query, params, AccessMode::Write)
        .map(|r| r.stats)
        .map_err(|e| e.to_string())
}

pub(crate) fn opt_usize(args: &Json, key: &str, default: usize) -> Result<usize, String> {
    match args.get(key) {
        None | Some(Json::Null) => Ok(default),
        Some(v) => v
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .ok_or_else(|| format!("'{key}' must be a non-negative integer")),
    }
}

pub(crate) fn req_str<'a>(args: &'a Json, key: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(Json::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| format!("'{key}' is required and must be a non-empty string"))
}

pub(crate) fn opt_strings(args: &Json, key: &str) -> Result<Vec<String>, String> {
    match args.get(key) {
        None | Some(Json::Null) => Ok(Vec::new()),
        Some(Json::Array(items)) => items
            .iter()
            .map(|v| {
                v.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| format!("'{key}' must be an array of strings"))
            })
            .collect(),
        Some(_) => Err(format!("'{key}' must be an array of strings")),
    }
}
