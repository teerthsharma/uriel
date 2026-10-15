use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::measure::collect_rs;
use crate::persistence::Metric;
use crate::rng::SplitMix64;
use crate::schedule::{prefix_salience, select_row, Blocks, ScheduleParams};

pub const SINK_BLOCKS: usize = 1;
pub const LOCAL_WINDOW: usize = 2;
pub const MIN_DOC_WORDS: usize = 6;
pub const WIN_SIGMAS: f64 = 2.0;

#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    pub vector: Vec<f64>,
    pub needles: Vec<usize>,
}

#[derive(Debug, Clone)]
pub struct Context {
    pub blocks: Blocks,
    pub queries: Vec<Query>,
}

impl Context {
    pub fn total_tokens(&self) -> usize {
        self.blocks.tokens.iter().sum()
    }
}

#[derive(Debug, Clone)]
pub struct SynthConfig {
    pub n_blocks: usize,
    pub dim: usize,
    pub topics: usize,
    pub noise_blocks: usize,
    pub needles: usize,
    pub query_alpha: f64,
    pub near_dup_prob: f64,
    pub topic_switch_prob: f64,
}

impl Default for SynthConfig {
    fn default() -> Self {
        Self {
            n_blocks: 1000,
            dim: 64,
            topics: 16,
            noise_blocks: 50,
            needles: 20,
            query_alpha: 0.25,
            near_dup_prob: 0.3,
            topic_switch_prob: 0.05,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Topic,
    Needle,
    Noise,
}

fn gauss(rng: &mut SplitMix64, d: usize) -> Vec<f64> {
    (0..d).map(|_| rng.normal()).collect()
}

fn normalise(v: &mut [f64]) {
    let n = v.iter().map(|x| x * x).sum::<f64>().sqrt();
    if n > 0.0 {
        for x in v.iter_mut() {
            *x /= n;
        }
    }
}

fn unit_gauss(rng: &mut SplitMix64, d: usize) -> Vec<f64> {
    let mut v = gauss(rng, d);
    normalise(&mut v);
    v
}

fn perturbed(base: &[f64], scale: f64, rng: &mut SplitMix64) -> Vec<f64> {
    let d = base.len();
    let s = scale / (d as f64).sqrt();
    let mut v: Vec<f64> = base.iter().map(|&b| b + s * rng.normal()).collect();
    normalise(&mut v);
    v
}

fn check_prob(name: &str, p: f64) -> Result<()> {
    if !p.is_finite() || !(0.0..=1.0).contains(&p) {
        return Err(Error::InvalidParameter(format!(
            "{name} {p} must be in [0, 1]"
        )));
    }
    Ok(())
}

fn check_synth(cfg: &SynthConfig) -> Result<()> {
    if cfg.dim < 2 {
        return Err(Error::InvalidParameter("dim must be at least 2".into()));
    }
    if cfg.topics == 0 {
        return Err(Error::InvalidParameter("topics must be at least 1".into()));
    }
    check_prob("query_alpha", cfg.query_alpha)?;
    check_prob("near_dup_prob", cfg.near_dup_prob)?;
    check_prob("topic_switch_prob", cfg.topic_switch_prob)?;
    let planted = cfg.needles.checked_add(cfg.noise_blocks);
    if planted.is_none_or(|p| p >= cfg.n_blocks) {
        return Err(Error::InvalidParameter(format!(
            "{} needles and {} noise blocks leave no topic block among {} blocks",
            cfg.needles, cfg.noise_blocks, cfg.n_blocks
        )));
    }
    Ok(())
}

pub fn synthetic_context(cfg: &SynthConfig, seed: u64) -> Result<Context> {
    check_synth(cfg)?;
    let n = cfg.n_blocks;
    let d = cfg.dim;
    let mut rng = SplitMix64::new(seed);
    let centers: Vec<Vec<f64>> = (0..cfg.topics).map(|_| unit_gauss(&mut rng, d)).collect();
    let mut slots: Vec<usize> = (0..n).collect();
    rng.shuffle(&mut slots);
    let mut kinds = vec![Kind::Topic; n];
    for &s in &slots[..cfg.needles] {
        kinds[s] = Kind::Needle;
    }
    for &s in &slots[cfg.needles..cfg.needles + cfg.noise_blocks] {
        kinds[s] = Kind::Noise;
    }
    let mut topic = rng.below(cfg.topics);
    let mut previous: Option<Vec<f64>> = None;
    let mut centroids: Vec<f64> = Vec::with_capacity(n * d);
    let mut tokens: Vec<usize> = Vec::with_capacity(n);
    let mut rows: Vec<Vec<f64>> = Vec::with_capacity(n);
    for kind in kinds.iter().copied() {
        tokens.push(64 + rng.below(129));
        let row = match kind {
            Kind::Needle | Kind::Noise => unit_gauss(&mut rng, d),
            Kind::Topic => {
                let switch = rng.next_f64() < cfg.topic_switch_prob;
                let next_topic = rng.below(cfg.topics);
                let dup = rng.next_f64() < cfg.near_dup_prob;
                if switch {
                    topic = next_topic;
                    previous = None;
                }
                let row = match (&previous, dup) {
                    (Some(p), true) => perturbed(p, 0.02, &mut rng),
                    _ => perturbed(&centers[topic], 0.5, &mut rng),
                };
                previous = Some(row.clone());
                row
            }
        };
        centroids.extend_from_slice(&row);
        rows.push(row);
    }
    let mut queries = Vec::with_capacity(cfg.needles);
    let alpha = cfg.query_alpha;
    let ortho = (1.0 - alpha * alpha).max(0.0).sqrt();
    for (i, kind) in kinds.iter().enumerate() {
        if *kind != Kind::Needle {
            continue;
        }
        let mut u = rows[i].clone();
        normalise(&mut u);
        let mut w = gauss(&mut rng, d);
        let proj: f64 = w.iter().zip(&u).map(|(a, b)| a * b).sum();
        for (x, b) in w.iter_mut().zip(&u) {
            *x -= proj * b;
        }
        normalise(&mut w);
        let vector: Vec<f64> = u
            .iter()
            .zip(&w)
            .map(|(a, b)| alpha * a + ortho * b)
            .collect();
        queries.push(Query {
            vector,
            needles: vec![i],
        });
    }
    let blocks = Blocks::new((0..n as u64).collect(), centroids, d, tokens)?;
    Ok(Context { blocks, queries })
}

#[derive(Debug, Clone)]
pub struct HashEmbedder {
    pub dim: usize,
}

const STOPWORDS: [&str; 24] = [
    "the", "an", "of", "to", "in", "is", "and", "or", "for", "with", "on", "by", "as", "at", "it",
    "its", "this", "that", "be", "are", "each", "from", "was", "has",
];

fn stem(w: &str) -> String {
    if w.len() > 4 && w.ends_with("ies") {
        format!("{}y", &w[..w.len() - 3])
    } else if w.len() > 3 && w.ends_with('s') && !w.ends_with("ss") {
        w[..w.len() - 1].to_string()
    } else {
        w.to_string()
    }
}

fn split_camel(run: &[char], out: &mut Vec<String>) {
    let mut start = 0;
    for i in 1..run.len() {
        let (p, c) = (run[i - 1], run[i]);
        let lower_to_upper = (p.is_lowercase() || p.is_ascii_digit()) && c.is_uppercase();
        let acronym_end = p.is_uppercase()
            && c.is_uppercase()
            && run.get(i + 1).is_some_and(|n| n.is_lowercase());
        if lower_to_upper || acronym_end {
            out.push(run[start..i].iter().collect());
            start = i;
        }
    }
    out.push(run[start..].iter().collect());
}

fn terms(text: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut run: Vec<char> = Vec::new();
    for c in text.chars().chain(std::iter::once(' ')) {
        if c.is_alphanumeric() {
            run.push(c);
        } else if !run.is_empty() {
            split_camel(&run, &mut parts);
            run.clear();
        }
    }
    parts
        .into_iter()
        .map(|p| p.to_lowercase())
        .filter(|p| p.chars().count() >= 2 && !STOPWORDS.contains(&p.as_str()))
        .map(|p| stem(&p))
        .collect()
}

fn hash_term(t: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in t.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h ^= h >> 33;
    h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
    h ^= h >> 33;
    h
}

impl HashEmbedder {
    pub fn embed(&self, text: &str) -> Vec<f64> {
        let mut v = vec![0.0; self.dim];
        if self.dim == 0 {
            return v;
        }
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for t in terms(text) {
            *counts.entry(t).or_insert(0) += 1;
        }
        for (t, c) in counts {
            let h = hash_term(&t);
            let slot = (h % self.dim as u64) as usize;
            let sign = if (h >> 40) & 1 == 0 { 1.0 } else { -1.0 };
            v[slot] += sign * (1.0 + (c as f64).ln());
        }
        normalise(&mut v);
        v
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocItem {
    pub doc: String,
    pub line: usize,
}

pub fn extract_doc_queries(source: &str) -> (String, Vec<DocItem>) {
    let mut kept: Vec<&str> = Vec::new();
    let mut items: Vec<DocItem> = Vec::new();
    let mut pending: Vec<String> = Vec::new();
    for line in source.lines() {
        let t = line.trim_start();
        if let Some(rest) = t.strip_prefix("///") {
            if !rest.starts_with('/') {
                pending.push(rest.trim().to_string());
                continue;
            }
        }
        if t.starts_with("//!") {
            continue;
        }
        if t.starts_with("#[") {
            kept.push(line);
            continue;
        }
        if t.is_empty() {
            pending.clear();
        } else if !pending.is_empty() {
            let doc = pending.join(" ");
            if doc.split_whitespace().count() >= MIN_DOC_WORDS {
                items.push(DocItem {
                    doc,
                    line: kept.len(),
                });
            }
            pending.clear();
        }
        kept.push(line);
    }
    let mut stripped = kept.join("\n");
    if !kept.is_empty() {
        stripped.push('\n');
    }
    (stripped, items)
}

pub fn repo_context(root: &Path, lines_per_block: usize, dim: usize) -> Result<Context> {
    if lines_per_block == 0 {
        return Err(Error::InvalidParameter(
            "lines_per_block must be at least 1".into(),
        ));
    }
    if dim == 0 {
        return Err(Error::InvalidParameter("dim must be at least 1".into()));
    }
    let crates = root.join("crates");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&crates)
        .map_err(|e| Error::Io(format!("{}: {e}", crates.display())))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    dirs.sort();
    let mut files: Vec<PathBuf> = Vec::new();
    for d in dirs {
        collect_rs(&d.join("src"), &mut files)?;
    }
    files.sort();
    let embedder = HashEmbedder { dim };
    let mut centroids: Vec<f64> = Vec::new();
    let mut tokens: Vec<usize> = Vec::new();
    let mut queries: Vec<Query> = Vec::new();
    let mut count = 0usize;
    for f in &files {
        let text =
            std::fs::read_to_string(f).map_err(|e| Error::Io(format!("{}: {e}", f.display())))?;
        let (stripped, items) = extract_doc_queries(&text);
        let lines: Vec<&str> = stripped.lines().collect();
        let mut chunk_block: Vec<Option<usize>> = Vec::new();
        for chunk in lines.chunks(lines_per_block) {
            let body = chunk.join("\n");
            let v = embedder.embed(&body);
            if v.iter().all(|&x| x == 0.0) {
                chunk_block.push(None);
                continue;
            }
            centroids.extend_from_slice(&v);
            tokens.push(body.len().div_ceil(4).max(1));
            chunk_block.push(Some(count));
            count += 1;
        }
        for item in items {
            let Some(Some(b)) = chunk_block.get(item.line / lines_per_block).copied() else {
                continue;
            };
            let v = embedder.embed(&item.doc);
            if v.iter().all(|&x| x == 0.0) {
                continue;
            }
            queries.push(Query {
                vector: v,
                needles: vec![b],
            });
        }
    }
    if count == 0 {
        return Err(Error::InvalidParameter(format!(
            "no source blocks under {}",
            crates.display()
        )));
    }
    let blocks = Blocks::new((0..count as u64).collect(), centroids, dim, tokens)?;
    Ok(Context { blocks, queries })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arm {
    Recency,
    TopK,
    Random,
    Topological,
    TopologicalNoRelevant,
    TopologicalNoSalient,
}

impl Arm {
    pub const ALL: [Arm; 6] = [
        Arm::Recency,
        Arm::TopK,
        Arm::Random,
        Arm::Topological,
        Arm::TopologicalNoRelevant,
        Arm::TopologicalNoSalient,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Arm::Recency => "recency",
            Arm::TopK => "topk",
            Arm::Random => "random",
            Arm::Topological => "topological",
            Arm::TopologicalNoRelevant => "topo-no-relevant",
            Arm::TopologicalNoSalient => "topo-no-salient",
        }
    }
}

fn greedy(ctx: &Context, order: impl Iterator<Item = usize>, budget: usize) -> Vec<usize> {
    let mut used = 0usize;
    let mut out = Vec::new();
    for i in order {
        match used.checked_add(ctx.blocks.tokens[i]) {
            Some(t) if t <= budget => {
                used = t;
                out.push(i);
            }
            _ => break,
        }
    }
    out.sort_unstable();
    out
}

fn cosine(a: &[f64], b: &[f64]) -> f64 {
    let dot = |x: &[f64], y: &[f64]| -> f64 { x.iter().zip(y).map(|(p, q)| p * q).sum() };
    let norm = |x: &[f64]| dot(x, x).sqrt().max(1e-300);
    dot(a, b) / (norm(a) * norm(b))
}

pub fn select(
    arm: Arm,
    ctx: &Context,
    q: &Query,
    budget: usize,
    salience: &[f64],
    seed: u64,
) -> Result<Vec<usize>> {
    let n = ctx.blocks.len();
    if n == 0 {
        return Ok(Vec::new());
    }
    match arm {
        Arm::Recency => Ok(greedy(ctx, (0..n).rev(), budget)),
        Arm::Random => {
            let mut order: Vec<usize> = (0..n).collect();
            SplitMix64::new(seed).shuffle(&mut order);
            Ok(greedy(ctx, order.into_iter(), budget))
        }
        Arm::TopK => {
            let d = ctx.blocks.dim;
            if q.vector.len() != d {
                return Err(Error::Shape(format!(
                    "query has {} values, blocks have dim {d}",
                    q.vector.len()
                )));
            }
            let mut sims: Vec<(f64, usize)> = (0..n)
                .map(|i| {
                    (
                        cosine(&q.vector, &ctx.blocks.centroids[i * d..(i + 1) * d]),
                        i,
                    )
                })
                .collect();
            sims.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
            Ok(greedy(ctx, sims.into_iter().map(|s| s.1), budget))
        }
        Arm::Topological | Arm::TopologicalNoRelevant | Arm::TopologicalNoSalient => {
            let mean = ctx.total_tokens().max(1) as f64 / n as f64;
            let slots = (budget as f64 / mean).floor() as usize;
            let (relevant_m, salient_k) = match arm {
                Arm::Topological => ((slots / 2).max(1), usize::MAX),
                Arm::TopologicalNoRelevant => (0, usize::MAX),
                _ => (usize::MAX, 0),
            };
            let p = ScheduleParams {
                sink_blocks: SINK_BLOCKS,
                local_window: LOCAL_WINDOW,
                salient_k,
                relevant_m,
                token_budget: Some(budget),
                metric: Metric::Cosine,
            };
            select_row(&ctx.blocks, n - 1, &p, Some(&q.vector), salience)
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Table {
    pub fracs: Vec<f64>,
    pub arms: Vec<Arm>,
    pub samples: Vec<Vec<Vec<f64>>>,
}

fn mean_of(v: &[f64]) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.iter().sum::<f64>() / v.len() as f64
}

fn se_of(v: &[f64]) -> f64 {
    let k = v.len();
    if k < 2 {
        return 0.0;
    }
    let m = mean_of(v);
    let var = v.iter().map(|x| (x - m) * (x - m)).sum::<f64>() / (k - 1) as f64;
    (var / k as f64).sqrt()
}

impl Table {
    pub fn mean(&self, a: usize, f: usize) -> f64 {
        mean_of(&self.samples[a][f])
    }

    pub fn se(&self, a: usize, f: usize) -> f64 {
        se_of(&self.samples[a][f])
    }
}

fn check_fracs(fracs: &[f64]) -> Result<()> {
    for &f in fracs {
        check_prob("budget fraction", f)?;
    }
    Ok(())
}

fn recall(sel: &[usize], needles: &[usize]) -> f64 {
    let hits = needles
        .iter()
        .filter(|n| sel.binary_search(n).is_ok())
        .count();
    hits as f64 / needles.len() as f64
}

pub fn context_recall(
    ctx: &Context,
    fracs: &[f64],
    arms: &[Arm],
    random_seeds: usize,
) -> Result<Table> {
    check_fracs(fracs)?;
    if random_seeds == 0 {
        return Err(Error::InvalidParameter(
            "random_seeds must be at least 1".into(),
        ));
    }
    let n = ctx.blocks.len();
    if n == 0 {
        return Err(Error::InvalidParameter("context has no blocks".into()));
    }
    ctx.blocks.validate()?;
    let salience = prefix_salience(&ctx.blocks, n - 1, Metric::Cosine)?;
    let total = ctx.total_tokens();
    let mut samples: Vec<Vec<Vec<f64>>> = vec![vec![Vec::new(); fracs.len()]; arms.len()];
    for (fi, &frac) in fracs.iter().enumerate() {
        let budget = (frac * total as f64) as usize;
        for (qi, q) in ctx.queries.iter().enumerate() {
            if q.needles.is_empty() {
                continue;
            }
            for (ai, &arm) in arms.iter().enumerate() {
                let reps = if arm == Arm::Random { random_seeds } else { 1 };
                let mut acc = 0.0;
                for k in 0..reps {
                    let seed = ((qi as u64) << 20) ^ k as u64 ^ 0xA5A5;
                    let sel = select(arm, ctx, q, budget, &salience, seed)?;
                    acc += recall(&sel, &q.needles);
                }
                samples[ai][fi].push(acc / reps as f64);
            }
        }
    }
    Ok(Table {
        fracs: fracs.to_vec(),
        arms: arms.to_vec(),
        samples,
    })
}

pub const SYNTH_RANDOM_SEEDS: usize = 5;

pub fn run_synthetic(
    cfg: &SynthConfig,
    contexts: usize,
    fracs: &[f64],
    arms: &[Arm],
) -> Result<Table> {
    if contexts == 0 {
        return Err(Error::InvalidParameter(
            "contexts must be at least 1".into(),
        ));
    }
    let mut samples: Vec<Vec<Vec<f64>>> = vec![vec![Vec::new(); fracs.len()]; arms.len()];
    for c in 0..contexts {
        let ctx = synthetic_context(cfg, c as u64 + 1)?;
        let t = context_recall(&ctx, fracs, arms, SYNTH_RANDOM_SEEDS)?;
        for (ai, per_arm) in samples.iter_mut().enumerate() {
            for (fi, cell) in per_arm.iter_mut().enumerate() {
                cell.push(t.mean(ai, fi));
            }
        }
    }
    Ok(Table {
        fracs: fracs.to_vec(),
        arms: arms.to_vec(),
        samples,
    })
}

#[derive(Debug, Clone, PartialEq)]
pub struct Verdict {
    pub frac: f64,
    pub margin: f64,
    pub se: f64,
    pub win: bool,
}

pub fn verdict(t: &Table) -> Vec<Verdict> {
    let Some(topo) = t.arms.iter().position(|&a| a == Arm::Topological) else {
        return Vec::new();
    };
    let baselines: Vec<usize> = t
        .arms
        .iter()
        .enumerate()
        .filter(|(_, &a)| a == Arm::Recency || a == Arm::TopK)
        .map(|(i, _)| i)
        .collect();
    if baselines.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(t.fracs.len());
    for (f, &frac) in t.fracs.iter().enumerate() {
        let mut best = baselines[0];
        for &b in &baselines[1..] {
            if t.mean(b, f).total_cmp(&t.mean(best, f)).is_gt() {
                best = b;
            }
        }
        let margin = t.mean(topo, f) - t.mean(best, f);
        let diffs: Vec<f64> = t.samples[topo][f]
            .iter()
            .zip(&t.samples[best][f])
            .map(|(a, b)| a - b)
            .collect();
        let se = se_of(&diffs);
        out.push(Verdict {
            frac,
            margin,
            se,
            win: margin > 0.0 && margin > WIN_SIGMAS * se,
        });
    }
    out
}
