use std::path::PathBuf;

use uriel_core::{ConstraintKind, NodeId, Props, Value};
use uriel_rag::{Hit, HitVia, IndexOptions, Level, Rag, SearchOptions, MEMORY_PREFIX};
use serde_json::{json, Value as Json};

use crate::compact::{encode_pack, Pack, PackHit};
use crate::config::{Format, Terse};
use crate::session::Seen;
use crate::terse;
use crate::tools::{
    format_arg, format_prop, no_args, object, opt_bool, opt_strings, opt_usize, req_str, str_list,
    str_prop, Context, ToolDef, ToolResult,
};

const DEFAULT_K: usize = 8;
const DEFAULT_BUDGET: usize = 2000;

fn id_list_schema(desc: &str) -> Json {
    json!({"type": "array", "items": {"type": "integer", "minimum": 0}, "description": desc})
}

fn budget_prop() -> Json {
    json!({"type": "integer", "minimum": 1,
        "description": "Token budget of the returned memories (tokens are ceil(chars / 4)). Default 2000."})
}

fn k_prop() -> Json {
    json!({"type": "integer", "minimum": 1, "description": "Number of direct hits. Default 8."})
}

fn rag_index_schema() -> Json {
    object(
        json!({"path": str_prop("Directory to index, absolute or relative to the project root. Default: the project root.")}),
        &[],
    )
}

fn rag_search_schema() -> Json {
    object(
        json!({
            "query": str_prop("What to look for."),
            "k": k_prop(),
            "budget_tokens": budget_prop(),
            "expand_hops": {"type": "integer", "minimum": 0, "maximum": 2,
                "description": "Graph expansion hops (adjacent chunks, definitions): 0 to 2. Default 1."},
            "format": format_prop(),
            "ladder": ladder_prop(),
            "fresh": fresh_prop()
        }),
        &["query"],
    )
}

fn ladder_prop() -> Json {
    json!({"type": "boolean",
        "description": "Resolution ladder: the top hit at L1 (about 1/4 of the chunk), hits 2 to 4 at L2 (compressed memory), the rest as one-line L3 gists, degraded to fit budget_tokens. Default: true with format=compact, false with format=json. Zoom a hit with rag_expand(chunk_id, level)."})
}

fn fresh_prop() -> Json {
    json!({"type": "boolean",
        "description": "Return the text of every hit even if it was already delivered in this session. With format=compact a repeat hit is just `#<chunk_id>` (flag `r`); a chunk whose text changed since comes back with its text and flag `~`."})
}

fn rag_expand_schema() -> Json {
    object(
        json!({
            "chunk_id": {"type": "integer", "minimum": 0,
                "description": "Chunk id from rag_search or seal_recall."},
            "level": {"type": "integer", "minimum": 0, "maximum": 3,
                "description": "Resolution: 0 the full original (default), 1 about a quarter of it, 2 the compressed memory, 3 a one-line gist."}
        }),
        &["chunk_id"],
    )
}

fn rag_feedback_schema() -> Json {
    object(
        json!({
            "used": id_list_schema("Chunk ids that helped; their utility prior moves up."),
            "unused": id_list_schema("Chunk ids that were retrieved but not used; their utility prior moves down.")
        }),
        &[],
    )
}

fn remember_schema() -> Json {
    object(
        json!({
            "text": str_prop("The finding or decision to remember."),
            "tags": str_list("Free-form tags, indexed with the text."),
            "sources": id_list_schema("Chunk ids the finding rests on; stored as CITES edges.")
        }),
        &["text"],
    )
}

fn recall_schema() -> Json {
    object(
        json!({
            "query": str_prop("What to recall."),
            "k": k_prop(),
            "budget_tokens": budget_prop(),
            "format": format_prop(),
            "ladder": ladder_prop(),
            "fresh": fresh_prop()
        }),
        &["query"],
    )
}

pub const TOOLS: &[ToolDef] = &[
    ToolDef {
        name: "rag_index",
        title: "Index a directory",
        description: "Index a directory into the seal RAG: chunks, compressed memories and embeddings stored in the seal database. Unchanged files are skipped; deleted files are pruned.",
        writes: true,
        schema: rag_index_schema,
    },
    ToolDef {
        name: "rag_search",
        title: "Search the RAG",
        description: "Hybrid BM25 + vector search over compressed memories, packed under a token budget. Compact v3 by default: `#<hits>` (and ` omit=<n>` when the budget dropped hits), then per hit a citation line `file:start-end#chunk_id[flags]` followed on the same line by a one-line text, or by `+n` and n text lines. `@a=prefix` lines abbreviate paths as `@a rest` (a space inside a path is `\\s`; a path that starts with `@` or `'` or is all digits is written with a leading `'`). A file is written in full once; a later hit in another, already written file cites it by its number `3:` (files count from 0 in order of first appearance), and a hit in the same file as the previous hit leaves the file out; `start+n` means end = start + n, a bare `start` means end = start, and a start written `+d`/`-d` is relative to the end of the previous hit in the same file. The top hit is shown at L1, hits 2 to 4 at L2, the rest as one-line L3 gists (flag `L1`/`L2`; none means L3); flags `g` graph neighbour, `m` remembered; a line `=g` marks every following hit a graph neighbour until a line `=`. A chunk already delivered in this session comes back as a bare citation with flag `r` (`~` if it changed since); pass fresh=true to get the text again. Zoom one hit with rag_expand(chunk_id, level). format=json returns the v1 JSON.",
        writes: false,
        schema: rag_search_schema,
    },
    ToolDef {
        name: "rag_expand",
        title: "Expand a chunk",
        description: "Return one chunk found by rag_search or seal_recall at a chosen resolution: level 0 (default) the full original text, 1 about a quarter of it, 2 the compressed memory, 3 a one-line gist.",
        writes: false,
        schema: rag_expand_schema,
    },
    ToolDef {
        name: "rag_feedback",
        title: "Give retrieval feedback",
        description: "Tell the RAG which retrieved chunks were used and which were not. Used chunks rank higher next time, unused ones lower (a bounded utility prior).",
        writes: true,
        schema: rag_feedback_schema,
    },
    ToolDef {
        name: "seal_remember",
        title: "Remember a finding",
        description: "Store a finding as a Decision node linked to its source chunks and index it, so it is retrievable with seal_recall and rag_search.",
        writes: true,
        schema: remember_schema,
    },
    ToolDef {
        name: "seal_recall",
        title: "Recall findings",
        description: "Search remembered findings first, then the general index. Returns cited memories with chunk ids; remembered ones are flagged. Same compact v3 encoding, ladder and session dedup as rag_search.",
        writes: false,
        schema: recall_schema,
    },
    ToolDef {
        name: "session_reset",
        title: "Reset the session",
        description: "Forget which chunks were already delivered in this session, so rag_search and seal_recall return their text again instead of bare `#<chunk_id>` references.",
        writes: false,
        schema: no_args,
    },
];

fn via_name(via: HitVia) -> &'static str {
    match via {
        HitVia::Direct => "direct",
        HitVia::GraphNeighbour => "graph",
    }
}

fn hit_json(hit: &Hit) -> Json {
    json!({
        "chunk_id": hit.chunk_id,
        "file": hit.file,
        "start_line": hit.start_line,
        "end_line": hit.end_line,
        "score": hit.score,
        "via": via_name(hit.via),
        "remembered": hit.file.starts_with(MEMORY_PREFIX),
    })
}

fn pack(hits: Vec<Hit>, budget: usize) -> Json {
    let total = hits.len();
    let mut used = 0usize;
    let mut text = String::new();
    let mut kept: Vec<Json> = Vec::new();
    for hit in &hits {
        let piece = format!(
            "// {}:{}-{} #{}\n{}\n",
            hit.file, hit.start_line, hit.end_line, hit.chunk_id, hit.memory
        );
        let cost = piece.chars().count().div_ceil(4);
        if used + cost <= budget {
            used += cost;
            text.push_str(&piece);
            kept.push(hit_json(hit));
        }
    }
    json!({
        "text": text,
        "hits": kept,
        "tokens": text.chars().count().div_ceil(4),
        "budget_tokens": budget,
        "omitted": total - kept.len(),
    })
}

fn proxy(text: &str) -> usize {
    text.chars().count().div_ceil(4)
}

type Delivered = (u64, String, u8);

struct Shown {
    hit: Hit,
    level: Level,
    body: String,
}

struct Delivery {
    format: Format,
    ladder: bool,
    fresh: bool,
    budget: usize,
}

impl Delivery {
    fn from_args(ctx: &Context, args: &Json, budget: usize) -> Result<Delivery, String> {
        let format = format_arg(args, ctx.format)?;
        Ok(Delivery {
            format,
            ladder: opt_bool(args, "ladder")?.unwrap_or(format == Format::Compact),
            fresh: opt_bool(args, "fresh")?.unwrap_or(false),
            budget,
        })
    }
}

fn ladder_shown(rag: &Rag, hits: &[Hit], budget: usize) -> Result<(Vec<Shown>, usize), String> {
    let pack = rag
        .ladder_pack(hits.to_vec(), budget)
        .map_err(|e| e.to_string())?;
    let shown = pack
        .hits
        .into_iter()
        .zip(hits)
        .map(|(l, h)| Shown {
            hit: h.clone(),
            level: l.level,
            body: l.text,
        })
        .collect();
    Ok((shown, pack.omitted))
}

fn memory_shown(hits: &[Hit], budget: usize) -> (Vec<Shown>, usize) {
    let mut used = 0usize;
    let mut kept = Vec::new();
    for hit in hits {
        let piece = format!(
            "// {}:{}-{} #{}\n{}\n",
            hit.file, hit.start_line, hit.end_line, hit.chunk_id, hit.memory
        );
        let cost = proxy(&piece);
        if used + cost <= budget {
            used += cost;
            kept.push(Shown {
                hit: hit.clone(),
                level: Level::L2,
                body: hit.memory.clone(),
            });
        }
    }
    let omitted = hits.len() - kept.len();
    (kept, omitted)
}

fn to_pack(
    ctx: &Context,
    rag: &Rag,
    shown: &[Shown],
    fresh: bool,
) -> Result<(Vec<PackHit>, Vec<Delivered>), String> {
    let mut hits = Vec::with_capacity(shown.len());
    let mut deliveries = Vec::new();
    for s in shown {
        let fp = rag
            .chunk_fingerprint(s.hit.chunk_id)
            .map_err(|e| e.to_string())?;
        let seen = match (&fp, fresh) {
            (Some(fp), false) => ctx.session.check(s.hit.chunk_id, fp, s.level.detail()),
            _ => Seen::New,
        };
        let reference = seen == Seen::Same;
        if let (false, Some(fp)) = (reference, fp) {
            deliveries.push((s.hit.chunk_id, fp, s.level.detail()));
        }
        hits.push(PackHit {
            chunk_id: s.hit.chunk_id,
            file: if reference { String::new() } else { s.hit.file.clone() },
            start: if reference { 0 } else { s.hit.start_line },
            end: if reference { 0 } else { s.hit.end_line },
            level: s.level.number(),
            graph: s.hit.via == HitVia::GraphNeighbour,
            remembered: s.hit.file.starts_with(MEMORY_PREFIX),
            changed: seen == Seen::Changed,
            body: if reference { None } else { Some(s.body.clone()) },
        });
    }
    Ok((hits, deliveries))
}

fn remembered_bodies(ctx: &Context, shown: &mut [Shown]) -> (usize, usize) {
    let mut totals = (0, 0);
    if ctx.terse != Terse::Prose {
        return totals;
    }
    for item in shown.iter_mut().filter(|s| s.hit.file.starts_with(MEMORY_PREFIX)) {
        let out = terse::lite_text(ctx, &item.body);
        totals.0 += proxy(&item.body);
        totals.1 += proxy(&out);
        item.body = out;
    }
    totals
}

fn deliver(ctx: &Context, hits: Vec<Hit>, d: &Delivery) -> ToolResult {
    let rag = rag(ctx)?;
    let ids: Vec<u64> = hits.iter().map(|h| h.chunk_id).collect();
    ctx.placement.observe_chunks(&ids);
    match (d.format, d.ladder) {
        (Format::Json, false) => Ok(pack(terse::remembered(ctx, hits), d.budget).to_string()),
        (Format::Json, true) => {
            let (mut shown, omitted) = ladder_shown(rag, &hits, d.budget)?;
            ctx.meter.add_pair(remembered_bodies(ctx, &mut shown));
            Ok(ladder_json(&shown, omitted, d.budget).to_string())
        }
        (Format::Compact, ladder) => {
            let hits = if ladder { hits } else { terse::remembered(ctx, hits) };
            let mut inner = d.budget;
            loop {
                let (shown, omitted, saved) = if ladder {
                    let (mut shown, omitted) = ladder_shown(rag, &hits, inner)?;
                    let saved = remembered_bodies(ctx, &mut shown);
                    (shown, omitted, saved)
                } else {
                    let (shown, omitted) = memory_shown(&hits, inner);
                    (shown, omitted, (0, 0))
                };
                let (entries, deliveries) = to_pack(ctx, rag, &shown, d.fresh)?;
                let text = encode_pack(&Pack {
                    hits: entries,
                    omitted,
                });
                let over = proxy(&text).saturating_sub(d.budget);
                if over > 0 && shown.is_empty() {
                    return Ok("#0".to_string());
                }
                if over == 0 || inner == 0 {
                    for (id, fp, detail) in deliveries {
                        ctx.session.record(id, &fp, detail);
                    }
                    ctx.meter.add_pair(saved);
                    return Ok(text);
                }
                inner = inner.saturating_sub(over);
            }
        }
    }
}

fn ladder_json(shown: &[Shown], omitted: usize, budget: usize) -> Json {
    let mut text = String::new();
    let mut kept: Vec<Json> = Vec::new();
    for s in shown {
        let head = format!(
            "// {}:{}-{} #{}",
            s.hit.file, s.hit.start_line, s.hit.end_line, s.hit.chunk_id
        );
        if s.level == Level::L3 {
            text.push_str(&format!("{head} {}\n", s.body));
        } else {
            text.push_str(&format!("{head}\n{}\n", s.body));
        }
        let mut j = hit_json(&s.hit);
        j["level"] = json!(s.level.number());
        kept.push(j);
    }
    json!({
        "text": text,
        "hits": kept,
        "tokens": proxy(&text),
        "budget_tokens": budget,
        "omitted": omitted,
    })
}

fn rag(ctx: &Context) -> Result<&Rag, String> {
    ctx.services.rag(ctx.engine.graph())
}

fn positive(args: &Json, key: &str, default: usize) -> Result<usize, String> {
    let n = opt_usize(args, key, default)?;
    if n == 0 {
        return Err(format!("'{key}' must be at least 1"));
    }
    Ok(n)
}

fn id_list(args: &Json, key: &str) -> Result<Vec<u64>, String> {
    match args.get(key) {
        None | Some(Json::Null) => Ok(Vec::new()),
        Some(Json::Array(items)) => items
            .iter()
            .map(|v| {
                v.as_u64()
                    .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
                    .ok_or_else(|| format!("'{key}' must be an array of chunk ids"))
            })
            .collect(),
        Some(_) => Err(format!("'{key}' must be an array of chunk ids")),
    }
}

pub fn call(ctx: &Context, name: &str, args: &Json) -> ToolResult {
    match name {
        "rag_index" => {
            let out = rag_index(ctx, args);
            ctx.services.forget_scaffold();
            out
        }
        "rag_search" => rag_search(ctx, args),
        "rag_expand" => rag_expand(ctx, args),
        "rag_feedback" => rag_feedback(ctx, args),
        "seal_remember" => {
            let out = seal_remember(ctx, args);
            ctx.services.forget_scaffold();
            out
        }
        "seal_recall" => seal_recall(ctx, args),
        "session_reset" => Ok(json!({"cleared": ctx.session.reset()}).to_string()),
        other => Err(format!("unknown tool '{other}'")),
    }
}

fn rag_index(ctx: &Context, args: &Json) -> ToolResult {
    let dir: PathBuf = match args.get("path") {
        None | Some(Json::Null) => ctx.services.root.clone(),
        Some(Json::String(s)) if !s.trim().is_empty() => {
            let p = PathBuf::from(s);
            if p.is_absolute() {
                p
            } else {
                ctx.services.root.join(p)
            }
        }
        Some(_) => return Err("'path' must be a non-empty string".into()),
    };
    if !dir.is_dir() {
        return Err(format!("{} is not a directory", dir.display()));
    }
    let r = rag(ctx)?
        .index_dir(&dir, &IndexOptions::default())
        .map_err(|e| e.to_string())?;
    Ok(json!({
        "path": dir.display().to_string(),
        "files_scanned": r.files_scanned,
        "files_indexed": r.files_indexed,
        "files_unchanged": r.files_unchanged,
        "files_skipped": r.files_skipped,
        "files_removed": r.files_removed,
        "chunks_added": r.chunks_added,
        "chunks_removed": r.chunks_removed,
    })
    .to_string())
}

fn rag_search(ctx: &Context, args: &Json) -> ToolResult {
    let query = req_str(args, "query")?;
    let opts = SearchOptions {
        k: positive(args, "k", DEFAULT_K)?,
        budget_tokens: positive(args, "budget_tokens", DEFAULT_BUDGET)?,
        expand_hops: opt_usize(args, "expand_hops", 1)?,
        use_utility: true,
    };
    let delivery = Delivery::from_args(ctx, args, opts.budget_tokens)?;
    let hits = rag(ctx)?.search(query, &opts).map_err(|e| e.to_string())?;
    deliver(ctx, hits, &delivery)
}

fn chunk_id(args: &Json, key: &str) -> Result<u64, String> {
    args.get(key)
        .and_then(|v| v.as_u64().or_else(|| v.as_str()?.trim().parse().ok()))
        .ok_or_else(|| format!("'{key}' is required and must be a chunk id (integer)"))
}

fn rag_expand(ctx: &Context, args: &Json) -> ToolResult {
    let id = chunk_id(args, "chunk_id")?;
    let level = match args.get("level") {
        None | Some(Json::Null) => Level::L0,
        Some(v) => v
            .as_u64()
            .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
            .and_then(Level::from_number)
            .ok_or_else(|| "'level' must be an integer from 0 to 3".to_string())?,
    };
    let rag = rag(ctx)?;
    let text = rag.expand_level(id, level).map_err(|e| e.to_string())?;
    ctx.placement.observe_chunks(&[id]);
    if let Some(fp) = rag.chunk_fingerprint(id).map_err(|e| e.to_string())? {
        ctx.session.record(id, &fp, level.detail());
    }
    Ok(text)
}

fn rag_feedback(ctx: &Context, args: &Json) -> ToolResult {
    let used = id_list(args, "used")?;
    let unused = id_list(args, "unused")?;
    if used.is_empty() && unused.is_empty() {
        return Err("give at least one chunk id in 'used' or 'unused'".into());
    }
    rag(ctx)?
        .feedback(&used, &unused)
        .map_err(|e| e.to_string())?;
    Ok(json!({"used": used.len(), "unused": unused.len()}).to_string())
}

fn next_finding_number(ctx: &Context) -> Result<u64, String> {
    let tx = ctx.engine.graph().begin_read().map_err(|e| e.to_string())?;
    let mut max = 0u64;
    for n in tx.nodes_by_label("Decision").map_err(|e| e.to_string())? {
        let n = n.map_err(|e| e.to_string())?;
        if let Some(Value::String(key)) = n.props.get("key") {
            if let Some(num) = key.strip_prefix("finding:").and_then(|s| s.parse().ok()) {
                max = max.max(num);
            }
        }
    }
    Ok(max + 1)
}

fn ensure_decision_constraint(ctx: &Context) -> Result<(), String> {
    let graph = ctx.engine.graph();
    let has = graph
        .list_constraints()
        .map_err(|e| e.to_string())?
        .iter()
        .any(|c| c.kind == ConstraintKind::Unique && c.label == "Decision" && c.key == "key");
    if !has {
        graph
            .create_unique_constraint("Decision", "key")
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn seal_remember(ctx: &Context, args: &Json) -> ToolResult {
    let text = req_str(args, "text")?;
    let tags = opt_strings(args, "tags")?;
    let sources = id_list(args, "sources")?;
    let rag = rag(ctx)?;
    let graph = ctx.engine.graph();
    {
        let tx = graph.begin_read().map_err(|e| e.to_string())?;
        for id in &sources {
            let is_chunk = tx
                .get_node(NodeId(*id))
                .map_err(|e| e.to_string())?
                .is_some_and(|n| n.labels.contains("Chunk"));
            if !is_chunk {
                return Err(format!("source {id} is not a chunk id"));
            }
        }
    }
    ensure_decision_constraint(ctx)?;
    let number = next_finding_number(ctx)?;
    let key = format!("finding:{number}");
    let source_id = format!("{MEMORY_PREFIX}finding-{number}.md");
    let mut props = Props::new();
    props.insert("key".into(), Value::String(key.clone()));
    props.insert("kind".into(), Value::String("finding".into()));
    props.insert("text".into(), Value::String(text.to_string()));
    props.insert("source_id".into(), Value::String(source_id.clone()));
    props.insert(
        "tags".into(),
        Value::List(tags.iter().cloned().map(Value::String).collect()),
    );
    let mut tx = graph.begin_write().map_err(|e| e.to_string())?;
    let node = tx
        .create_node(&["Decision"], props)
        .map_err(|e| e.to_string())?;
    for id in &sources {
        tx.create_rel(node, NodeId(*id), "CITES", Props::new())
            .map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    let mut body = text.to_string();
    if !tags.is_empty() {
        body.push_str("\ntags: ");
        body.push_str(&tags.join(", "));
    }
    rag.index_text(&source_id, &body)
        .map_err(|e| format!("{key} was stored but could not be indexed: {e}"))?;
    Ok(json!({"id": key, "source_id": source_id, "sources": sources.len()}).to_string())
}

fn seal_recall(ctx: &Context, args: &Json) -> ToolResult {
    let query = req_str(args, "query")?;
    let k = positive(args, "k", DEFAULT_K)?;
    let budget = positive(args, "budget_tokens", DEFAULT_BUDGET)?;
    let delivery = Delivery::from_args(ctx, args, budget)?;
    let wide = SearchOptions {
        k: (k * 4).max(32),
        budget_tokens: budget,
        expand_hops: 0,
        use_utility: true,
    };
    let hits = rag(ctx)?.search(query, &wide).map_err(|e| e.to_string())?;
    let (remembered, general): (Vec<Hit>, Vec<Hit>) = hits
        .into_iter()
        .partition(|h| h.file.starts_with(MEMORY_PREFIX));
    let ordered: Vec<Hit> = remembered.into_iter().chain(general).take(k).collect();
    deliver(ctx, ordered, &delivery)
}
