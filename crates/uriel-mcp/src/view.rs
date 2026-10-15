use uriel_rag::{Embedder, HashEmbedder, Hit, HitVia, Rag, SearchOptions};
use uriel_topo::persistence::Metric;
use uriel_topo::schedule::{schedule_row, select_row, Blocks, ScheduleParams};
use serde_json::{json, Value as Json};

use crate::compact::{encode_pack, proxy_tokens, Pack, PackHit};
use crate::config::Format;
use crate::tools::{
    format_arg, format_prop, object, req_str, str_prop, Context, ToolDef, ToolResult,
};

const MEMORY_PREFIX: &str = "memory/";
const DEFAULT_K: u64 = 12;
const MAX_K: u64 = 256;
const SINK_BLOCKS: usize = 1;
const LOCAL_WINDOW: usize = 2;
const MIN_RELEVANT: usize = 2;
const EMBED_DIM: usize = 256;

fn view_schema() -> Json {
    object(
        json!({
            "question": str_prop("What the context is for."),
            "budget_tokens": {"type": "integer", "minimum": 1,
                "description": "Hard token budget of the whole returned view (tokens are ceil(chars / 4))."},
            "focus": str_prop("Optional text the RELEVANT selection is matched against. Default: the question."),
            "k": {"type": "integer", "minimum": 1, "maximum": 256,
                "description": "Candidate chunks fetched for the question. Default 12."},
            "salient_k": {"type": "integer", "minimum": 0, "maximum": 256,
                "description": "Extra topologically salient blocks. Default 0: the salient term lost to top-k in the C3 evaluation."},
            "format": format_prop()
        }),
        &["question", "budget_tokens"],
    )
}

pub const TOOLS: &[ToolDef] = &[ToolDef {
    name: "view",
    title: "Build a budgeted context view",
    description: "Build a context view for a question under a hard token budget: RAG candidates are ordered into blocks, selected by the uriel-topo schedule (the first block as sink, the last two as local window, the blocks nearest to focus as relevant), rendered through the resolution ladder (top hit L1, next three L2, the rest one-line L3 gists) and never over budget. Compact by default with a citation per hit (`>file:start-end#chunk_id`, same encoding as rag_search); format=json returns text, hits and counts. Deterministic and independent of the session: the same arguments return the same bytes.",
    writes: false,
    schema: view_schema,
}];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Params {
    pub sink: usize,
    pub local: usize,
    pub relevant: usize,
    pub salient: usize,
}

pub(crate) fn choose(
    centroids: &[f64],
    dim: usize,
    tokens: Vec<usize>,
    query: &[f64],
    p: &Params,
    budget: usize,
) -> Result<Vec<usize>, String> {
    let n = tokens.len();
    if n == 0 {
        return Ok(Vec::new());
    }
    let blocks = Blocks::new((0..n as u64).collect(), centroids.to_vec(), dim, tokens)
        .map_err(|e| e.to_string())?;
    let params = ScheduleParams {
        sink_blocks: p.sink,
        local_window: p.local,
        salient_k: p.salient,
        relevant_m: p.relevant,
        token_budget: Some(budget),
        metric: Metric::Cosine,
    };
    if p.salient > 0 {
        schedule_row(&blocks, n - 1, &params, Some(query)).map_err(|e| e.to_string())
    } else {
        select_row(&blocks, n - 1, &params, Some(query), &vec![0.0; n]).map_err(|e| e.to_string())
    }
}

fn bounded(args: &Json, key: &str, default: u64, min: u64, max: u64) -> Result<usize, String> {
    let n = match args.get(key) {
        None | Some(Json::Null) => default,
        Some(v) => v
            .as_u64()
            .filter(|n| (min..=max).contains(n))
            .ok_or_else(|| format!("'{key}' must be an integer from {min} to {max}"))?,
    };
    usize::try_from(n).map_err(|_| format!("'{key}' is too large"))
}

fn budget_arg(args: &Json) -> Result<usize, String> {
    let n = args
        .get("budget_tokens")
        .and_then(Json::as_u64)
        .filter(|n| *n >= 1)
        .ok_or_else(|| {
            "'budget_tokens' is required and must be an integer of at least 1".to_string()
        })?;
    Ok(usize::try_from(n).unwrap_or(usize::MAX))
}

fn focus_arg(args: &Json) -> Result<Option<&str>, String> {
    match args.get("focus") {
        None | Some(Json::Null) => Ok(None),
        Some(Json::String(s)) if s.trim().is_empty() => Ok(None),
        Some(Json::String(s)) => Ok(Some(s)),
        Some(_) => Err("'focus' must be a string".into()),
    }
}

fn as_f64(v: Vec<f32>) -> Vec<f64> {
    v.into_iter().map(f64::from).collect()
}

fn select(
    hits: &[Hit],
    question: &str,
    focus: Option<&str>,
    k: usize,
    salient: usize,
    budget: usize,
) -> Result<Vec<Hit>, String> {
    let mut order: Vec<usize> = (0..hits.len()).collect();
    order.sort_by(|&a, &b| {
        (&hits[a].file, hits[a].start_line, hits[a].chunk_id).cmp(&(
            &hits[b].file,
            hits[b].start_line,
            hits[b].chunk_id,
        ))
    });
    let embedder = HashEmbedder::new(EMBED_DIM);
    let mut centroids = Vec::with_capacity(order.len() * EMBED_DIM);
    let mut tokens = Vec::with_capacity(order.len());
    for &i in &order {
        let h = &hits[i];
        centroids.extend(as_f64(embedder.embed(&h.memory)));
        tokens.push(proxy_tokens(&format!(
            ">{}:{}-{} #{}\n{}\n",
            h.file, h.start_line, h.end_line, h.chunk_id, h.memory
        )));
    }
    let query = as_f64(embedder.embed(focus.unwrap_or(question)));
    let params = Params {
        sink: SINK_BLOCKS,
        local: LOCAL_WINDOW,
        relevant: MIN_RELEVANT.max(k / 4),
        salient,
    };
    let chosen = choose(&centroids, EMBED_DIM, tokens, &query, &params, budget)?;
    let mut ranks: Vec<usize> = chosen.into_iter().map(|b| order[b]).collect();
    ranks.sort_unstable();
    Ok(ranks.into_iter().map(|r| hits[r].clone()).collect())
}

fn pack_hit(h: uriel_rag::LadderHit) -> PackHit {
    PackHit {
        chunk_id: h.chunk_id,
        remembered: h.file.starts_with(MEMORY_PREFIX),
        graph: h.via == HitVia::GraphNeighbour,
        file: h.file,
        start: h.start_line,
        end: h.end_line,
        level: h.level.number(),
        changed: false,
        body: Some(h.text),
    }
}

fn compact(rag: &Rag, hits: &[Hit], budget: usize, candidates: usize) -> Result<String, String> {
    let mut inner = budget;
    loop {
        let pack = rag
            .ladder_pack(hits.to_vec(), inner)
            .map_err(|e| e.to_string())?;
        let kept = pack.hits.len();
        if kept == 0 {
            return Ok("#0".to_string());
        }
        let text = encode_pack(&Pack {
            hits: pack.hits.into_iter().map(pack_hit).collect(),
            omitted: candidates - kept,
        });
        let over = proxy_tokens(&text).saturating_sub(budget);
        if over == 0 {
            return Ok(text);
        }
        inner = inner.saturating_sub(over);
    }
}

fn json_view(rag: &Rag, hits: &[Hit], budget: usize, candidates: usize) -> Result<String, String> {
    let pack = rag
        .ladder_pack(hits.to_vec(), budget)
        .map_err(|e| e.to_string())?;
    let listed: Vec<Json> = pack
        .hits
        .iter()
        .map(|h| {
            json!({
                "chunk_id": h.chunk_id,
                "file": h.file,
                "start_line": h.start_line,
                "end_line": h.end_line,
                "score": h.score,
                "level": h.level.number(),
            })
        })
        .collect();
    Ok(json!({
        "text": pack.text,
        "hits": listed,
        "tokens": pack.tokens,
        "budget_tokens": budget,
        "omitted": candidates - pack.hits.len(),
        "candidates": candidates,
        "selected": hits.len(),
    })
    .to_string())
}

pub fn call(ctx: &Context, name: &str, args: &Json) -> ToolResult {
    match name {
        "view" => view(ctx, args),
        other => Err(format!("unknown tool '{other}'")),
    }
}

fn view(ctx: &Context, args: &Json) -> ToolResult {
    let format = format_arg(args, ctx.format)?;
    let question = req_str(args, "question")?;
    let budget = budget_arg(args)?;
    let focus = focus_arg(args)?;
    let k = bounded(args, "k", DEFAULT_K, 1, MAX_K)?;
    let salient = bounded(args, "salient_k", 0, 0, MAX_K)?;
    let rag = ctx.services.rag(ctx.engine.graph())?;
    let candidates = rag
        .search(
            question,
            &SearchOptions {
                k,
                budget_tokens: budget,
                expand_hops: 0,
                use_utility: true,
            },
        )
        .map_err(|e| e.to_string())?;
    let total = candidates.len();
    if total == 0 {
        return Ok(match format {
            Format::Compact => "#0".to_string(),
            Format::Json => json_view(rag, &[], budget, 0)?,
        });
    }
    let chosen = select(&candidates, question, focus, k, salient, budget)?;
    match format {
        Format::Compact => compact(rag, &chosen, budget, total),
        Format::Json => json_view(rag, &chosen, budget, total),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: Params = Params {
        sink: 1,
        local: 2,
        relevant: 1,
        salient: 0,
    };

    fn unit_blocks(n: usize) -> Vec<f64> {
        let mut v = vec![0.0; n * n];
        for i in 0..n {
            v[i * n + i] = 1.0;
        }
        v
    }

    fn pick(query_block: usize, p: &Params, tokens: Vec<usize>, budget: usize) -> Vec<usize> {
        let n = tokens.len();
        let blocks = unit_blocks(n);
        let query = blocks[query_block * n..(query_block + 1) * n].to_vec();
        choose(&blocks, n, tokens, &query, p, budget).expect("choose")
    }

    #[test]
    fn selection_is_sink_local_and_relevant_blocks() {
        assert_eq!(pick(2, &BASE, vec![1; 6], 100), vec![0, 2, 4, 5]);
    }

    #[test]
    fn the_focus_decides_which_block_is_relevant() {
        assert_eq!(pick(3, &BASE, vec![1; 6], 100), vec![0, 3, 4, 5]);
        assert_eq!(pick(1, &BASE, vec![1; 6], 100), vec![0, 1, 4, 5]);
    }

    #[test]
    fn a_zero_salient_k_adds_nothing_beyond_sink_local_and_relevant() {
        let none = Params { salient: 0, ..BASE };
        assert_eq!(pick(2, &none, vec![1; 6], 100).len(), 4);
    }

    #[test]
    fn a_salient_k_only_ever_adds_blocks() {
        let base = pick(2, &BASE, vec![1; 8], 100);
        let more = pick(2, &Params { salient: 3, ..BASE }, vec![1; 8], 100);
        assert!(base.iter().all(|b| more.contains(b)), "{base:?} {more:?}");
        assert!(more.len() >= base.len());
    }

    #[test]
    fn the_budget_drops_the_lowest_priority_blocks_first() {
        assert_eq!(pick(2, &BASE, vec![10; 6], 25), vec![0, 5]);
        assert_eq!(pick(2, &BASE, vec![10; 6], 30), vec![0, 4, 5]);
        assert_eq!(pick(2, &BASE, vec![10; 6], 9), Vec::<usize>::new());
    }

    #[test]
    fn a_single_block_is_its_own_sink_and_local_window() {
        assert_eq!(pick(0, &BASE, vec![3], 10), vec![0]);
    }

    #[test]
    fn no_blocks_select_nothing() {
        let q = vec![0.0; 4];
        assert_eq!(choose(&[], 4, Vec::new(), &q, &BASE, 10), Ok(Vec::new()));
    }

    #[test]
    fn a_query_of_the_wrong_width_is_an_error_not_a_panic() {
        let blocks = unit_blocks(3);
        assert!(choose(&blocks, 3, vec![1; 3], &[1.0], &BASE, 10).is_err());
    }

    #[test]
    fn a_non_finite_centroid_is_an_error_not_a_panic() {
        let mut blocks = unit_blocks(3);
        blocks[1] = f64::NAN;
        assert!(choose(&blocks, 3, vec![1; 3], &[1.0, 0.0, 0.0], &BASE, 10).is_err());
    }

    #[test]
    fn budget_and_k_arguments_are_validated() {
        assert_eq!(budget_arg(&json!({"budget_tokens": 5})), Ok(5));
        assert_eq!(
            budget_arg(&json!({"budget_tokens": u64::MAX})),
            Ok(usize::MAX)
        );
        for bad in [
            json!({}),
            json!({"budget_tokens": 0}),
            json!({"budget_tokens": -1}),
            json!({"budget_tokens": "5"}),
        ] {
            assert!(budget_arg(&bad).is_err(), "{bad}");
        }
        assert_eq!(bounded(&json!({}), "k", 12, 1, 256), Ok(12));
        assert!(bounded(&json!({"k": 0}), "k", 12, 1, 256).is_err());
        assert!(bounded(&json!({"k": 257}), "k", 12, 1, 256).is_err());
    }

    #[test]
    fn a_blank_focus_is_absent_and_a_non_string_focus_is_refused() {
        assert_eq!(focus_arg(&json!({})), Ok(None));
        assert_eq!(focus_arg(&json!({"focus": "  "})), Ok(None));
        assert_eq!(focus_arg(&json!({"focus": "x"})), Ok(Some("x")));
        assert!(focus_arg(&json!({"focus": 3})).is_err());
    }
}
