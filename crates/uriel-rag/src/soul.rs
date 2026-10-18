use std::collections::{BTreeMap, BTreeSet, HashMap};

use uriel_core::NodeId;

use crate::bm25::idf;
use crate::chunk::{is_doc_line, is_signature, Lang};
use crate::index::{rust_defs, HOP_DECAY};
use crate::tokenize::terms;
use crate::{prop_line, prop_str, sha_hex, Hit, HitVia, Level, LadderPack, Rag, RagError};

const POOL_MAX: usize = 2048;
const NOISE_START: f64 = 0.2;
const NOISE_DECAY: f64 = 0.5;
const STABLE_ROUNDS: usize = 2;
const W_DOC: f64 = 0.4;
const W_DEF: f64 = 0.2;
const W_IDF: f64 = 0.4;
const DEF_CAP: usize = 4;
const L3_ESTIMATE: usize = 32;
const HEAD_RESERVE_DIV: usize = 5;
const GROW_STEPS: usize = 8;
const SEED_TAG: &str = "uriel-soul-seed-v1";
const GOLDEN: u64 = 0x9E37_79B9_7F4A_7C15;

struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(GOLDEN);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

#[derive(Debug, Clone)]
pub struct SoulOptions {
    pub budget_tokens: usize,
    pub focus: Option<String>,
    pub max_rounds: usize,
}

impl Default for SoulOptions {
    fn default() -> Self {
        SoulOptions {
            budget_tokens: 1200,
            focus: None,
            max_rounds: 8,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SoulSection {
    pub chunk_id: u64,
    pub file: String,
    pub start_line: u32,
    pub end_line: u32,
    pub level: Level,
    pub score: f32,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct Soul {
    pub sections: Vec<SoulSection>,
    pub text: String,
    pub tokens: usize,
    pub budget_tokens: usize,
    pub omitted: usize,
    pub rounds: usize,
    pub seed: u64,
    pub digest: String,
}

#[derive(Debug, Clone)]
pub(crate) struct Row {
    pub id: u64,
    pub file: String,
    pub start: u32,
    pub end: u32,
    pub salience: f64,
}

pub(crate) struct Scan {
    pub rows: Vec<Row>,
    pub fingerprint: String,
}

struct Raw {
    id: u64,
    file: String,
    start: u32,
    end: u32,
    hash: String,
    doc: f64,
    defs: f64,
    terms: Vec<String>,
}

fn doc_density(text: &str, lang: Lang) -> f64 {
    if matches!(lang, Lang::Markdown | Lang::Text) {
        return 1.0;
    }
    let (mut doc, mut total) = (0usize, 0usize);
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        total += 1;
        doc += usize::from(is_doc_line(line, lang));
    }
    if total == 0 {
        0.0
    } else {
        doc as f64 / total as f64
    }
}

fn definition_weight(text: &str, lang: Lang) -> f64 {
    let n = if lang == Lang::Rust {
        rust_defs(text).len()
    } else {
        text.lines().filter(|l| is_signature(l, lang)).count()
    };
    n.min(DEF_CAP) as f64 / DEF_CAP as f64
}

fn normalised(values: &[f64]) -> Vec<f64> {
    let max = values.iter().copied().fold(0.0f64, f64::max);
    values
        .iter()
        .map(|v| if max > 0.0 { v / max } else { 0.0 })
        .collect()
}

fn seed_of(fingerprint: &str) -> u64 {
    let digest = sha_hex(format!("{SEED_TAG}:{fingerprint}").as_bytes());
    u64::from_str_radix(&digest[..16], 16).unwrap_or(GOLDEN)
}

fn signature(pack: &LadderPack) -> Vec<(u64, u8)> {
    pack.hits.iter().map(|h| (h.chunk_id, h.level.number())).collect()
}

struct Diffusion<'a> {
    rag: &'a Rag,
    pool: Vec<Row>,
    memories: Vec<String>,
    budget: usize,
}

impl Diffusion<'_> {
    fn hits(&self, scores: &[f64]) -> Vec<Hit> {
        let mut order: Vec<usize> = (0..self.pool.len()).collect();
        order.sort_by(|&a, &b| {
            scores[b]
                .total_cmp(&scores[a])
                .then(self.pool[a].id.cmp(&self.pool[b].id))
        });
        order
            .into_iter()
            .map(|i| Hit {
                chunk_id: self.pool[i].id,
                file: self.pool[i].file.clone(),
                start_line: self.pool[i].start,
                end_line: self.pool[i].end,
                memory: self.memories[i].clone(),
                score: scores[i] as f32,
                via: HitVia::Direct,
            })
            .collect()
    }

    fn pack(&self, scores: &[f64]) -> Result<LadderPack, RagError> {
        let hits = self.hits(scores);
        let reserve = self.budget / HEAD_RESERVE_DIV;
        let mut window = ((self.budget - reserve) / L3_ESTIMATE).clamp(1, hits.len().max(1));
        let mut pack = self.rag.ladder_pack(hits[..window.min(hits.len())].to_vec(), self.budget)?;
        for _ in 0..GROW_STEPS {
            let slack = self.budget.saturating_sub(pack.tokens);
            if pack.omitted > 0 || window >= hits.len() || slack < reserve + L3_ESTIMATE / 2 {
                break;
            }
            window = (window + ((slack - reserve) / L3_ESTIMATE / 2).max(1)).min(hits.len());
            pack = self.rag.ladder_pack(hits[..window].to_vec(), self.budget)?;
        }
        Ok(pack)
    }
}

impl Rag {
    pub(crate) fn salience_scan(&self) -> Result<Scan, RagError> {
        let read = self.graph.begin_read()?;
        let mut raws: Vec<Raw> = Vec::new();
        let mut df: HashMap<String, u32> = HashMap::new();
        for node in read.nodes_by_label("Chunk")? {
            let node = node?;
            let file = prop_str(&node, "file")?.to_string();
            let text = prop_str(&node, "text")?;
            let mem = prop_str(&node, "mem")?;
            let lang = Lang::from_path(&file);
            let unique: BTreeSet<String> = terms(mem).into_iter().collect();
            let unique: Vec<String> = unique.into_iter().collect();
            for t in &unique {
                *df.entry(t.clone()).or_insert(0) += 1;
            }
            let hash = match node.props.get("hash") {
                Some(uriel_core::Value::String(h)) => h.clone(),
                _ => sha_hex(text.as_bytes()),
            };
            raws.push(Raw {
                id: node.id.0,
                start: prop_line(&node, "start")?,
                end: prop_line(&node, "end")?,
                hash,
                doc: doc_density(text, lang),
                defs: definition_weight(text, lang),
                terms: unique,
                file,
            });
        }
        raws.sort_by(|a, b| (&a.file, a.start, a.end, a.id).cmp(&(&b.file, b.start, b.end, b.id)));
        let n = raws.len() as f64;
        let mass: Vec<f64> = raws
            .iter()
            .map(|r| {
                let sum: f64 = r
                    .terms
                    .iter()
                    .map(|t| idf(n, f64::from(df.get(t).copied().unwrap_or(1))))
                    .sum();
                (1.0 + sum).ln()
            })
            .collect();
        let docs = normalised(&raws.iter().map(|r| r.doc).collect::<Vec<_>>());
        let defs = normalised(&raws.iter().map(|r| r.defs).collect::<Vec<_>>());
        let mass = normalised(&mass);
        let total = W_DOC + W_DEF + W_IDF;
        let mut fingerprint_input = String::new();
        let rows: Vec<Row> = raws
            .iter()
            .enumerate()
            .map(|(i, r)| {
                fingerprint_input.push_str(&format!(
                    "{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\n",
                    r.file, r.start, r.end, r.id, r.hash
                ));
                Row {
                    id: r.id,
                    file: r.file.clone(),
                    start: r.start,
                    end: r.end,
                    salience: (W_DOC * docs[i] + W_DEF * defs[i] + W_IDF * mass[i]) / total,
                }
            })
            .collect();
        Ok(Scan {
            rows,
            fingerprint: sha_hex(fingerprint_input.as_bytes()),
        })
    }

    pub fn soul(&self, opts: &SoulOptions) -> Result<Soul, RagError> {
        let scan = self.salience_scan()?;
        let seed = seed_of(&scan.fingerprint);
        let mut pool: Vec<Row> = scan
            .rows
            .into_iter()
            .filter(|r| opts.focus.as_deref().is_none_or(|p| r.file.starts_with(p)))
            .collect();
        pool.sort_by(|a, b| b.salience.total_cmp(&a.salience).then(a.id.cmp(&b.id)));
        pool.truncate(POOL_MAX);
        let read = self.graph.begin_read()?;
        let mut memories = Vec::with_capacity(pool.len());
        for row in &pool {
            let node = read
                .get_node(NodeId(row.id))?
                .ok_or(RagError::NotFound(row.id))?;
            memories.push(prop_str(&node, "mem")?.to_string());
        }
        let ids: Vec<u64> = pool.iter().map(|r| r.id).collect();
        let slot: BTreeMap<u64, usize> = ids.iter().enumerate().map(|(i, id)| (*id, i)).collect();
        let mut adj: Vec<Vec<usize>> = vec![Vec::new(); pool.len()];
        for (a, b) in self.edges_among(&ids)? {
            if let (Some(&i), Some(&j)) = (slot.get(&a), slot.get(&b)) {
                adj[i].push(j);
                adj[j].push(i);
            }
        }
        let base: Vec<f64> = pool.iter().map(|r| r.salience).collect();
        let work = Diffusion {
            rag: self,
            pool,
            memories,
            budget: opts.budget_tokens,
        };
        let mut scores = base.clone();
        let mut pack = work.pack(&scores)?;
        let mut last = signature(&pack);
        let (mut stable, mut rounds) = (0usize, 0usize);
        let mut rng = SplitMix64::new(seed);
        for t in 0..opts.max_rounds {
            let mut next = base.clone();
            for (i, ns) in adj.iter().enumerate() {
                let inflow: f64 = ns.iter().map(|&j| scores[j] / adj[j].len() as f64).sum();
                next[i] += f64::from(HOP_DECAY) * inflow;
            }
            let top = next.iter().copied().fold(0.0f64, f64::max);
            let amp = NOISE_START * NOISE_DECAY.powi(t as i32) * top;
            for v in &mut next {
                *v = (*v + amp * (rng.next_f64() - 0.5)).max(0.0);
            }
            scores = next;
            pack = work.pack(&scores)?;
            rounds = t + 1;
            let now = signature(&pack);
            if now == last {
                stable += 1;
                if stable >= STABLE_ROUNDS {
                    break;
                }
            } else {
                stable = 0;
                last = now;
            }
        }
        let by_id: HashMap<u64, f64> = work
            .pool
            .iter()
            .zip(&scores)
            .map(|(r, s)| (r.id, *s))
            .collect();
        let sections = pack
            .hits
            .iter()
            .map(|h| SoulSection {
                chunk_id: h.chunk_id,
                file: h.file.clone(),
                start_line: h.start_line,
                end_line: h.end_line,
                level: h.level,
                score: by_id.get(&h.chunk_id).copied().unwrap_or(0.0) as f32,
                text: h.text.clone(),
            })
            .collect();
        Ok(Soul {
            sections,
            tokens: pack.tokens,
            budget_tokens: pack.budget_tokens,
            omitted: work.pool.len() - pack.hits.len(),
            rounds,
            seed,
            digest: sha_hex(pack.text.as_bytes()),
            text: pack.text,
        })
    }
}
