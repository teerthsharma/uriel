use std::collections::BTreeMap;

use uriel_algo::{bucket_islands, island_labels, Projection};
use uriel_rag::{Embedder, HashEmbedder, Hit, HitVia, LadderPack, Rag, SearchOptions};
use uriel_topo::persistence::Metric;
use uriel_topo::schedule::{schedule_row, select_row, Blocks, ScheduleParams};
use serde_json::{json, Value as Json};

use crate::compact::{encode_pack, proxy_tokens, Pack, PackHit};
use crate::config::Format;
use crate::tools::{
    format_arg, format_prop, object, opt_bool, req_str, str_prop, Context, ToolDef, ToolResult,
};

const MEMORY_PREFIX: &str = "memory/";
const DEFAULT_K: u64 = 12;
const MAX_K: u64 = 256;
const SINK_BLOCKS: usize = 1;
const LOCAL_WINDOW: usize = 2;
const MIN_RELEVANT: usize = 2;
const EMBED_DIM: usize = 256;
const ISLANDS_DEFAULT: bool = false;
const RESIDUAL_MAX_ISLAND: usize = 1;
const ISLAND_BOUNDS: [usize; 1] = [RESIDUAL_MAX_ISLAND];
const WEIGHT_SCALE: f32 = 1_000_000.0;

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
            "islands": {"type": "boolean",
                "description": "Group the candidates into islands (connected through next/previous chunk and Rust definition-to-mention links) and render one budgeted section per island, ordered by the island holding the best hit. A single island renders exactly as without. Default off."},
            "format": format_prop()
        }),
        &["question", "budget_tokens"],
    )
}

pub const TOOLS: &[ToolDef] = &[ToolDef {
    name: "view",
    title: "Build a budgeted context view",
    description: "Build a context view for a question under a hard token budget: RAG candidates are ordered into blocks, selected by the uriel-topo schedule (the first block as sink, the last two as local window, the blocks nearest to focus as relevant), rendered through the resolution ladder (top hit L1, next three L2, the rest one-line L3 gists) and never over budget. Compact by default with a citation per hit (`>file:start-end#chunk_id`, same encoding as rag_search); format=json returns text, hits and counts. With islands=true the candidates are partitioned into connected islands and rendered as one section per island. Deterministic and independent of the session: the same arguments return the same bytes.",
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

fn listed(pack: &LadderPack) -> Vec<Json> {
    pack.hits
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
        .collect()
}

fn json_view(rag: &Rag, hits: &[Hit], budget: usize, candidates: usize) -> Result<String, String> {
    let pack = rag
        .ladder_pack(hits.to_vec(), budget)
        .map_err(|e| e.to_string())?;
    Ok(json!({
        "text": pack.text,
        "hits": listed(&pack),
        "tokens": pack.tokens,
        "budget_tokens": budget,
        "omitted": candidates - pack.hits.len(),
        "candidates": candidates,
        "selected": hits.len(),
    })
    .to_string())
}

struct Island {
    label: u32,
    hits: Vec<Hit>,
}

fn ranked(hits: &[Hit]) -> Vec<Hit> {
    let mut v = hits.to_vec();
    v.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then(a.chunk_id.cmp(&b.chunk_id))
    });
    v
}

fn partition(rag: &Rag, ranked: &[Hit]) -> Result<Vec<Island>, String> {
    let ids: Vec<u64> = ranked.iter().map(|h| h.chunk_id).collect();
    let edges = rag.edges_among(&ids).map_err(|e| e.to_string())?;
    let node_of: BTreeMap<u64, u32> = ids
        .iter()
        .enumerate()
        .filter_map(|(i, id)| Some((*id, u32::try_from(i).ok()?)))
        .collect();
    let pairs: Vec<(u32, u32)> = edges
        .iter()
        .filter_map(|(a, b)| Some((*node_of.get(a)?, *node_of.get(b)?)))
        .collect();
    let projection =
        Projection::from_edges(ranked.len(), &pairs, false).map_err(|e| e.to_string())?;
    let labels = island_labels(&projection);
    let buckets = bucket_islands(&labels, &ISLAND_BOUNDS);
    let mut members: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
    for (node, label) in labels.iter().enumerate() {
        members.entry(*label).or_default().push(node);
    }
    let take = |nodes: &[usize]| -> Vec<Hit> { nodes.iter().map(|&n| ranked[n].clone()).collect() };
    let mut islands: Vec<Island> = buckets[1..]
        .iter()
        .flatten()
        .map(|label| Island {
            label: *label,
            hits: take(&members[label]),
        })
        .collect();
    if let Some(&first) = buckets[0].first() {
        let nodes: Vec<usize> = buckets[0].iter().map(|&l| l as usize).collect();
        islands.push(Island {
            label: first,
            hits: take(&nodes),
        });
    }
    islands.sort_by_key(|i| i.label);
    Ok(islands)
}

fn weight(island: &Island) -> u64 {
    island
        .hits
        .iter()
        .map(|h| ((h.score.max(0.0) * WEIGHT_SCALE) as u64).max(1))
        .sum()
}

pub(crate) fn split_budget(budget: usize, weights: &[u64]) -> Vec<usize> {
    let total: u128 = weights.iter().map(|&w| u128::from(w)).sum();
    if total == 0 {
        return vec![0; weights.len()];
    }
    let mut shares: Vec<usize> = Vec::with_capacity(weights.len());
    let mut remainders: Vec<(u128, usize)> = Vec::with_capacity(weights.len());
    for (i, &w) in weights.iter().enumerate() {
        let scaled = budget as u128 * u128::from(w);
        shares.push((scaled / total) as usize);
        remainders.push((scaled % total, i));
    }
    let leftover = budget - shares.iter().sum::<usize>();
    remainders.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    for &(_, i) in remainders.iter().take(leftover) {
        shares[i] += 1;
    }
    shares
}

fn render_islands(
    rag: &Rag,
    islands: &[Island],
    req: &ViewRequest,
    total: usize,
) -> Result<String, String> {
    let weights: Vec<u64> = islands.iter().map(weight).collect();
    let shares = split_budget(req.budget, &weights);
    let mut chosen: Vec<Vec<Hit>> = Vec::with_capacity(islands.len());
    for (island, share) in islands.iter().zip(&shares) {
        chosen.push(select(
            &island.hits,
            req.question,
            req.focus,
            req.k,
            req.salient,
            *share,
        )?);
    }
    match req.format {
        Format::Compact => {
            let mut out = String::new();
            for ((island, hits), share) in islands.iter().zip(&chosen).zip(&shares) {
                let section = compact(rag, hits, *share, island.hits.len())?;
                if section != "#0" {
                    out.push_str(&section);
                }
            }
            if out.is_empty() {
                out.push_str("#0");
            }
            Ok(out)
        }
        Format::Json => {
            let (mut text, mut hits_json) = (String::new(), Vec::new());
            let (mut tokens, mut kept, mut selected) = (0usize, 0usize, 0usize);
            let mut sections: Vec<Json> = Vec::with_capacity(islands.len());
            for ((island, hits), share) in islands.iter().zip(&chosen).zip(&shares) {
                let pack = rag
                    .ladder_pack(hits.clone(), *share)
                    .map_err(|e| e.to_string())?;
                sections.push(json!({
                    "label": island.label,
                    "candidates": island.hits.len(),
                    "budget_tokens": share,
                    "selected": hits.len(),
                    "kept": pack.hits.len(),
                    "tokens": pack.tokens,
                }));
                text.push_str(&pack.text);
                hits_json.extend(listed(&pack));
                tokens += pack.tokens;
                kept += pack.hits.len();
                selected += hits.len();
            }
            Ok(json!({
                "text": text,
                "hits": hits_json,
                "tokens": tokens,
                "budget_tokens": req.budget,
                "omitted": total - kept,
                "candidates": total,
                "selected": selected,
                "islands": sections,
            })
            .to_string())
        }
    }
}

pub fn call(ctx: &Context, name: &str, args: &Json) -> ToolResult {
    match name {
        "view" => view(ctx, args),
        other => Err(format!("unknown tool '{other}'")),
    }
}

pub struct ViewRequest<'a> {
    pub question: &'a str,
    pub focus: Option<&'a str>,
    pub k: usize,
    pub salient: usize,
    pub budget: usize,
    pub format: Format,
    pub islands: bool,
}

pub fn render_view(rag: &Rag, candidates: &[Hit], req: &ViewRequest) -> Result<String, String> {
    let total = candidates.len();
    if total == 0 {
        return Ok(match req.format {
            Format::Compact => "#0".to_string(),
            Format::Json => json_view(rag, &[], req.budget, 0)?,
        });
    }
    if req.islands {
        let ranked = ranked(candidates);
        let islands = partition(rag, &ranked)?;
        if islands.len() > 1 {
            return render_islands(rag, &islands, req, total);
        }
        return render_single(rag, &ranked, req, total);
    }
    render_single(rag, candidates, req, total)
}

fn render_single(
    rag: &Rag,
    candidates: &[Hit],
    req: &ViewRequest,
    total: usize,
) -> Result<String, String> {
    let chosen = select(
        candidates,
        req.question,
        req.focus,
        req.k,
        req.salient,
        req.budget,
    )?;
    match req.format {
        Format::Compact => compact(rag, &chosen, req.budget, total),
        Format::Json => json_view(rag, &chosen, req.budget, total),
    }
}

pub fn run_view(rag: &Rag, req: &ViewRequest) -> Result<String, String> {
    let candidates = rag
        .search(
            req.question,
            &SearchOptions {
                k: req.k,
                budget_tokens: req.budget,
                expand_hops: 0,
                use_utility: true,
            },
        )
        .map_err(|e| e.to_string())?;
    render_view(rag, &candidates, req)
}

fn view(ctx: &Context, args: &Json) -> ToolResult {
    let format = format_arg(args, ctx.format)?;
    let question = req_str(args, "question")?;
    let budget = budget_arg(args)?;
    let focus = focus_arg(args)?;
    let k = bounded(args, "k", DEFAULT_K, 1, MAX_K)?;
    let salient = bounded(args, "salient_k", 0, 0, MAX_K)?;
    let islands = opt_bool(args, "islands")?.unwrap_or(ISLANDS_DEFAULT);
    let rag = ctx.services.rag(ctx.engine.graph())?;
    run_view(
        rag,
        &ViewRequest {
            question,
            focus,
            k,
            salient,
            budget,
            format,
            islands,
        },
    )
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
    fn budget_shares_sum_to_the_budget_for_any_weights() {
        let mut state = 0x9E37_79B9_7F4A_7C15_u64;
        let mut next = || {
            state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            state >> 33
        };
        for _ in 0..500 {
            let n = 1 + (next() % 9) as usize;
            let weights: Vec<u64> = (0..n).map(|_| 1 + next() % 50_000).collect();
            let budget = (next() % 5_000) as usize;
            let shares = split_budget(budget, &weights);
            assert_eq!(shares.iter().sum::<usize>(), budget, "{weights:?} {budget}");
        }
    }

    #[test]
    fn budget_shares_follow_the_weights_and_break_ties_towards_the_lower_island() {
        assert_eq!(split_budget(10, &[1, 1, 1]), vec![4, 3, 3]);
        assert_eq!(split_budget(100, &[1, 3]), vec![25, 75]);
        assert_eq!(split_budget(7, &[2, 2]), vec![4, 3]);
        assert_eq!(split_budget(5, &[0, 0]), vec![0, 0]);
        assert_eq!(split_budget(0, &[3, 4]), vec![0, 0]);
        assert_eq!(split_budget(usize::MAX, &[1, 1]).iter().sum::<usize>(), usize::MAX);
    }

    #[test]
    fn a_blank_focus_is_absent_and_a_non_string_focus_is_refused() {
        assert_eq!(focus_arg(&json!({})), Ok(None));
        assert_eq!(focus_arg(&json!({"focus": "  "})), Ok(None));
        assert_eq!(focus_arg(&json!({"focus": "x"})), Ok(Some("x")));
        assert!(focus_arg(&json!({"focus": 3})).is_err());
    }
}
