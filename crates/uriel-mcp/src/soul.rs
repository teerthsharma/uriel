use serde_json::{json, Value as Json};
use uriel_rag::{Soul, SoulOptions, SoulSection};

use crate::config::Format;
use crate::tools::{format_arg, format_prop, object, str_prop, Context, ToolDef, ToolResult};

const DEFAULT_BUDGET: u64 = 1200;
const MAX_BUDGET: u64 = 100_000;
const EMPTY: &str = "#0";

fn soul_schema() -> Json {
    object(
        json!({
            "budget_tokens": {"type": "integer", "minimum": 1, "maximum": MAX_BUDGET,
                "description": "Hard token budget of the guide (tokens are ceil(chars / 4)). Default 1200."},
            "focus": str_prop("Optional path prefix; only files under it enter the guide."),
            "sections": {"type": "array", "items": {"type": "integer", "minimum": 0},
                "description": "Chunk ids of guide sections to return alone, in guide order, so the guide can be read piecewise. Every id must be a section of the guide for the same budget_tokens and focus."},
            "format": format_prop()
        }),
        &[],
    )
}

pub const TOOLS: &[ToolDef] = &[ToolDef {
    name: "soul",
    title: "Soul of the idea guide",
    description: "Return a small reference guide to the indexed codebase, the soul of the idea: many short sections ranked by query-free salience (documentation density, definitions, rare vocabulary) and refined by seeded deterministic score diffusion along next-chunk and definition-to-mention links, then packed under a hard token budget through the resolution ladder (top sections at higher resolution, the rest one-line L3 gists). Each section carries its address `// file:start-end #chunk_id`; rag_expand with that chunk_id returns the exact original. Compact by default (the guide text, most important section first); format=json returns the sections, counts, seed, digest and `cached`. `sections` returns only the listed sections. No model, no GPU: the same index and arguments return the same bytes, and a computed guide is cached in the graph (not written on a --read-only server).",
    writes: false,
    schema: soul_schema,
}];

pub fn call(ctx: &Context, name: &str, args: &Json) -> ToolResult {
    match name {
        "soul" => soul(ctx, args),
        other => Err(format!("unknown tool '{other}'")),
    }
}

fn budget_arg(args: &Json) -> Result<usize, String> {
    let n = match args.get("budget_tokens") {
        None | Some(Json::Null) => DEFAULT_BUDGET,
        Some(v) => v
            .as_u64()
            .filter(|n| (1..=MAX_BUDGET).contains(n))
            .ok_or_else(|| format!("'budget_tokens' must be an integer from 1 to {MAX_BUDGET}"))?,
    };
    usize::try_from(n).map_err(|_| "'budget_tokens' is too large".to_string())
}

fn focus_arg(args: &Json) -> Result<Option<String>, String> {
    match args.get("focus") {
        None | Some(Json::Null) => Ok(None),
        Some(Json::String(s)) if s.trim().is_empty() => Ok(None),
        Some(Json::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err("'focus' must be a string".into()),
    }
}

fn sections_arg(args: &Json) -> Result<Vec<u64>, String> {
    match args.get("sections") {
        None | Some(Json::Null) => Ok(Vec::new()),
        Some(Json::Array(items)) => items
            .iter()
            .map(|v| {
                v.as_u64()
                    .ok_or_else(|| "'sections' must be an array of chunk ids (integers)".to_string())
            })
            .collect(),
        Some(_) => Err("'sections' must be an array of chunk ids (integers)".into()),
    }
}

fn listed(s: &SoulSection) -> Json {
    json!({
        "chunk_id": s.chunk_id,
        "file": s.file,
        "start_line": s.start_line,
        "end_line": s.end_line,
        "level": s.level.number(),
        "score": s.score,
        "text": s.text,
    })
}

fn pick(soul: &Soul, ids: &[u64], budget: usize) -> Result<Vec<SoulSection>, String> {
    if ids.is_empty() {
        return Ok(soul.sections.clone());
    }
    for id in ids {
        if !soul.sections.iter().any(|s| s.chunk_id == *id) {
            return Err(format!(
                "section {id} is not in the guide for budget_tokens={budget} and this focus; call soul without 'sections' to list them"
            ));
        }
    }
    Ok(soul
        .sections
        .iter()
        .filter(|s| ids.contains(&s.chunk_id))
        .cloned()
        .collect())
}

fn soul(ctx: &Context, args: &Json) -> ToolResult {
    let format = format_arg(args, ctx.format)?;
    let budget = budget_arg(args)?;
    let focus = focus_arg(args)?;
    let ids = sections_arg(args)?;
    let rag = ctx.services.rag(ctx.engine.graph())?;
    let guide = rag
        .soul(&SoulOptions {
            budget_tokens: budget,
            focus,
            ..SoulOptions::default()
        })
        .map_err(|e| e.to_string())?;
    let (chosen, changed) = crate::terse::prose_sections(ctx, pick(&guide, &ids, budget)?);
    let text: String = if ids.is_empty() && !changed {
        guide.text.clone()
    } else {
        chosen.iter().map(SoulSection::render).collect()
    };
    match format {
        Format::Compact => Ok(if text.is_empty() { EMPTY.to_string() } else { text }),
        Format::Json => crate::prefix::lead_json(ctx, json!({
            "tokens": text.chars().count().div_ceil(4),
            "text": text,
            "sections": chosen.iter().map(listed).collect::<Vec<_>>(),
            "budget_tokens": budget,
            "omitted": guide.omitted,
            "rounds": guide.rounds,
            "seed": format!("{:016x}", guide.seed),
            "digest": guide.digest,
            "cached": guide.cache.hit,
        })
        .to_string(),
        ),
    }
}
