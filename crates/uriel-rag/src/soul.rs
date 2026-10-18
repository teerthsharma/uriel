use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use sha2::{Digest, Sha256};

use uriel_core::{Node, NodeId, Props, Value};

use crate::bm25::idf;
use crate::chunk::{is_doc_line, is_signature, Lang};
use crate::index::{rust_defs, HOP_DECAY};
use crate::tokenize::terms;
use crate::{hex_digest, lock, prop_line, prop_str, sha_hex, Hit, HitVia, LadderPack, Level, Rag, RagError};

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
const CACHE_VERSION: &str = "soul-cache-v1";
const CACHE_DIV: u64 = 8;
const MAX_GUIDES: usize = 8;
const NODE_OVERHEAD: u64 = 96;
const GUIDE: &str = "Soul";
const SECTION: &str = "SoulSection";
const FEATURE: &str = "SoulFeat";
const HAS_SECTION: &str = "HAS_SECTION";
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

impl SoulSection {
    pub fn render(&self) -> String {
        let head = format!(
            "// {}:{}-{} #{}",
            self.file, self.start_line, self.end_line, self.chunk_id
        );
        if self.level == Level::L3 {
            format!("{head} {}
", self.text)
        } else {
            format!("{head}
{}
", self.text)
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SoulCache {
    pub hit: bool,
    pub rounds_run: usize,
    pub features_computed: usize,
    pub features_reused: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SoulCacheStats {
    pub bytes: u64,
    pub cap: u64,
    pub guides: usize,
    pub sections: usize,
    pub features: usize,
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
    pub cache: SoulCache,
}

#[derive(Debug, Clone)]
pub(crate) struct Row {
    pub id: u64,
    pub file: Arc<str>,
    pub start: u32,
    pub end: u32,
    pub salience: f64,
}

pub(crate) struct Scan {
    pub rows: Vec<Row>,
    pub fingerprint: String,
    computed: usize,
    reused: usize,
    fresh: Vec<(Arc<str>, Feature)>,
    words: Vec<Arc<str>>,
    seen: HashSet<String>,
    existing: HashMap<String, (NodeId, u64)>,
    hashes: HashMap<u64, String>,
}

impl Scan {
    fn hash_of(&self, id: u64) -> &str {
        self.hashes.get(&id).map_or("", String::as_str)
    }
}

#[derive(Clone)]
struct Feature {
    doc: f64,
    defs: f64,
    terms: Arc<[u32]>,
}

#[derive(Default)]
struct Interner {
    ids: HashMap<Arc<str>, u32>,
    words: Vec<Arc<str>>,
}

impl Interner {
    fn id(&mut self, word: &str) -> u32 {
        if let Some(&id) = self.ids.get(word) {
            return id;
        }
        let shared: Arc<str> = Arc::from(word);
        let id = u32::try_from(self.words.len()).unwrap_or(u32::MAX);
        self.words.push(shared.clone());
        self.ids.insert(shared, id);
        id
    }
}

fn props_bytes(props: &Props) -> u64 {
    NODE_OVERHEAD
        + props
            .iter()
            .map(|(k, v)| {
                k.len() as u64
                    + match v {
                        Value::String(s) => s.len() as u64,
                        _ => 8,
                    }
            })
            .sum::<u64>()
}

fn feature_props(key: &str, f: &Feature, words: &[Arc<str>]) -> Props {
    let mut p = Props::new();
    p.insert("key".into(), Value::from(key));
    p.insert("doc".into(), Value::Float(f.doc));
    p.insert("defs".into(), Value::Float(f.defs));
    let joined: Vec<&str> = f.terms.iter().filter_map(|&t| words.get(t as usize).map(|w| &**w)).collect();
    p.insert("terms".into(), Value::from(joined.join(" ").as_str()));
    p
}

fn feature_of(node: &Node, interner: &mut Interner) -> Option<(String, Feature)> {
    let (Some(Value::String(key)), Some(Value::Float(doc)), Some(Value::Float(defs)), Some(Value::String(t))) = (
        node.props.get("key"),
        node.props.get("doc"),
        node.props.get("defs"),
        node.props.get("terms"),
    ) else {
        return None;
    };
    Some((
        key.clone(),
        Feature {
            doc: *doc,
            defs: *defs,
            terms: t.split(' ').filter(|w| !w.is_empty()).map(|w| interner.id(w)).collect::<Vec<u32>>().into(),
        },
    ))
}

fn str_of(node: &Node, key: &str) -> Option<String> {
    match node.props.get(key) {
        Some(Value::String(s)) => Some(s.clone()),
        _ => None,
    }
}

fn int_of(node: &Node, key: &str) -> Option<i64> {
    match node.props.get(key) {
        Some(Value::Int(i)) => Some(*i),
        _ => None,
    }
}

fn params_key(opts: &SoulOptions) -> String {
    sha_hex(
        format!(
            "{CACHE_VERSION}|{}|{}|{}",
            opts.budget_tokens,
            opts.max_rounds,
            opts.focus.as_deref().unwrap_or("")
        )
        .as_bytes(),
    )
}

struct Raw {
    id: u64,
    file: Arc<str>,
    start: u32,
    end: u32,
    hash: String,
    feat: Feature,
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
                file: self.pool[i].file.to_string(),
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
    fn soul_cache_cap(&self) -> u64 {
        self.index_cap / CACHE_DIV
    }

    pub fn soul_cache_stats(&self) -> Result<SoulCacheStats, RagError> {
        let read = self.graph.begin_read()?;
        let mut stats = SoulCacheStats {
            cap: self.soul_cache_cap(),
            ..SoulCacheStats::default()
        };
        for (label, count) in [
            (GUIDE, &mut stats.guides),
            (SECTION, &mut stats.sections),
            (FEATURE, &mut stats.features),
        ] {
            for node in read.nodes_by_label(label)? {
                stats.bytes += props_bytes(&node?.props);
                *count += 1;
            }
        }
        Ok(stats)
    }

    fn load_guide(&self, gkey: &str) -> Result<Option<Soul>, RagError> {
        let read = self.graph.begin_read()?;
        let want = Value::from(gkey);
        let Some(guide) = read.nodes_by_prop(GUIDE, "gkey", &want)?.next().transpose()? else {
            return Ok(None);
        };
        let mut stored: Vec<(i64, SoulSection)> = Vec::new();
        for edge in read.out_edges(guide.id, Some(HAS_SECTION))? {
            let Some(n) = read.get_node(edge?.other)? else {
                return Ok(None);
            };
            let Some(item) = section_of(&n) else {
                return Ok(None);
            };
            stored.push(item);
        }
        stored.sort_by_key(|(ord, _)| *ord);
        let usize_of = |k: &str| int_of(&guide, k).and_then(|v| usize::try_from(v).ok());
        let parsed = (|| {
            Some(Soul {
                sections: stored.into_iter().map(|(_, s)| s).collect(),
                text: str_of(&guide, "text")?,
                tokens: usize_of("tokens")?,
                budget_tokens: usize_of("budget")?,
                omitted: usize_of("omitted")?,
                rounds: usize_of("rounds")?,
                seed: u64::from_str_radix(&str_of(&guide, "seed")?, 16).ok()?,
                digest: str_of(&guide, "digest")?,
                cache: SoulCache {
                    hit: true,
                    ..SoulCache::default()
                },
            })
        })();
        Ok(parsed)
    }

    fn store_guide(&self, scan: &Scan, gkey: &str, pkey: &str, soul: &Soul) {
        if self.graph.is_read_only() {
            return;
        }
        let _gate = lock(&self.gate);
        let _ = self.write_cache(scan, gkey, pkey, soul);
    }

    fn write_cache(&self, scan: &Scan, gkey: &str, pkey: &str, soul: &Soul) -> Result<(), RagError> {
        let cap = self.soul_cache_cap();
        let read = self.graph.begin_read()?;
        let mut tx = self.graph.begin_write()?;
        let mut used: u64 = 0;
        for (key, (id, bytes)) in &scan.existing {
            if scan.seen.contains(key) {
                used += bytes;
            } else {
                tx.detach_delete_node(*id)?;
            }
        }
        let mut kept: Vec<(i64, NodeId, u64)> = Vec::new();
        let mut next_seq = 1i64;
        for g in read.nodes_by_label(GUIDE)? {
            let g = g?;
            let seq = int_of(&g, "seq").unwrap_or(0);
            next_seq = next_seq.max(seq + 1);
            let bytes = u64::try_from(int_of(&g, "bytes").unwrap_or(0)).unwrap_or(0);
            if str_of(&g, "pkey").as_deref() == Some(pkey) {
                drop_guide(&read, &mut tx, g.id)?;
            } else {
                kept.push((seq, g.id, bytes));
            }
        }
        kept.sort();
        while kept.len() >= MAX_GUIDES {
            let (_, id, _) = kept.remove(0);
            drop_guide(&read, &mut tx, id)?;
        }
        used += kept.iter().map(|(_, _, b)| *b).sum::<u64>();
        let mut sections: Vec<Props> = Vec::with_capacity(soul.sections.len());
        for (ord, s) in soul.sections.iter().enumerate() {
            sections.push(section_props(ord, s, scan.hash_of(s.chunk_id)));
        }
        let mut guide = Props::new();
        guide.insert("gkey".into(), Value::from(gkey));
        guide.insert("pkey".into(), Value::from(pkey));
        guide.insert("seq".into(), Value::Int(next_seq));
        guide.insert("tokens".into(), Value::Int(soul.tokens as i64));
        guide.insert("budget".into(), Value::Int(soul.budget_tokens as i64));
        guide.insert("omitted".into(), Value::Int(soul.omitted as i64));
        guide.insert("rounds".into(), Value::Int(soul.rounds as i64));
        guide.insert("seed".into(), Value::from(format!("{:x}", soul.seed).as_str()));
        guide.insert("digest".into(), Value::from(soul.digest.as_str()));
        guide.insert("text".into(), Value::from(soul.text.as_str()));
        guide.insert("bytes".into(), Value::Int(0));
        let guide_bytes = props_bytes(&guide) + sections.iter().map(props_bytes).sum::<u64>();
        if used + guide_bytes <= cap {
            guide.insert("bytes".into(), Value::Int(guide_bytes as i64));
            let gid = tx.create_node(&[GUIDE], guide)?;
            for p in sections {
                let sid = tx.create_node(&[SECTION], p)?;
                tx.create_rel(gid, sid, HAS_SECTION, Props::new())?;
            }
            used += guide_bytes;
        }
        for (key, feat) in &scan.fresh {
            let props = feature_props(key, feat, &scan.words);
            let bytes = props_bytes(&props);
            if used + bytes <= cap {
                tx.create_node(&[FEATURE], props)?;
                used += bytes;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub(crate) fn salience_scan(&self) -> Result<Scan, RagError> {
        let read = self.graph.begin_read()?;
        let mut interner = Interner::default();
        let mut stored: HashMap<String, (NodeId, u64, Feature)> = HashMap::new();
        for node in read.nodes_by_label(FEATURE)? {
            let node = node?;
            if let Some((key, feat)) = feature_of(&node, &mut interner) {
                stored.insert(key, (node.id, props_bytes(&node.props), feat));
            }
        }
        let mut raws: Vec<Raw> = Vec::new();
        let mut df: Vec<u32> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        let mut local: HashMap<Arc<str>, Feature> = HashMap::new();
        let mut fresh: Vec<(Arc<str>, Feature)> = Vec::new();
        let mut files: HashMap<String, Arc<str>> = HashMap::new();
        let (mut computed, mut reused) = (0usize, 0usize);
        for node in read.nodes_by_label("Chunk")? {
            let node = node?;
            let file_name = prop_str(&node, "file")?;
            let file = match files.get(file_name) {
                Some(f) => f.clone(),
                None => {
                    let f: Arc<str> = Arc::from(file_name);
                    files.insert(file_name.to_string(), f.clone());
                    f
                }
            };
            let text = prop_str(&node, "text")?;
            let mem = prop_str(&node, "mem")?;
            let lang = Lang::from_path(&file);
            let hash = match node.props.get("hash") {
                Some(Value::String(h)) => h.clone(),
                _ => sha_hex(text.as_bytes()),
            };
            let key = sha_hex(format!("{hash}|{lang:?}|{mem}").as_bytes());
            let feat = if let Some((_, _, f)) = stored.get(&key) {
                reused += 1;
                f.clone()
            } else if let Some(f) = local.get(key.as_str()) {
                reused += 1;
                f.clone()
            } else {
                computed += 1;
                let unique: BTreeSet<String> = terms(mem).into_iter().collect();
                let f = Feature {
                    doc: doc_density(text, lang),
                    defs: definition_weight(text, lang),
                    terms: unique.iter().map(|t| interner.id(t)).collect(),
                };
                let shared: Arc<str> = Arc::from(key.as_str());
                local.insert(shared.clone(), f.clone());
                fresh.push((shared, f.clone()));
                f
            };
            for &t in feat.terms.iter() {
                let t = t as usize;
                if df.len() <= t {
                    df.resize(t + 1, 0);
                }
                df[t] += 1;
            }
            if stored.contains_key(&key) {
                seen.insert(key);
            }
            raws.push(Raw {
                id: node.id.0,
                start: prop_line(&node, "start")?,
                end: prop_line(&node, "end")?,
                hash,
                feat,
                file,
            });
        }
        let existing: HashMap<String, (NodeId, u64)> =
            stored.into_iter().map(|(k, (id, b, _))| (k, (id, b))).collect();
        raws.sort_by(|a, b| (&a.file, a.start, a.end, a.id).cmp(&(&b.file, b.start, b.end, b.id)));
        let n = raws.len() as f64;
        let mass: Vec<f64> = raws
            .iter()
            .map(|r| {
                let sum: f64 = r
                    .feat
                    .terms
                    .iter()
                    .map(|&t| idf(n, f64::from(df.get(t as usize).copied().unwrap_or(1))))
                    .sum();
                (1.0 + sum).ln()
            })
            .collect();
        let docs = normalised(&raws.iter().map(|r| r.feat.doc).collect::<Vec<_>>());
        let defs = normalised(&raws.iter().map(|r| r.feat.defs).collect::<Vec<_>>());
        let mass = normalised(&mass);
        let total = W_DOC + W_DEF + W_IDF;
        let mut hasher = Sha256::new();
        let mut hashes: HashMap<u64, String> = HashMap::with_capacity(raws.len());
        let mut rows: Vec<Row> = Vec::with_capacity(raws.len());
        for (i, r) in raws.into_iter().enumerate() {
            hasher.update(
                format!("{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\n", r.file, r.start, r.end, r.id, r.hash)
                    .as_bytes(),
            );
            rows.push(Row {
                id: r.id,
                file: r.file,
                start: r.start,
                end: r.end,
                salience: (W_DOC * docs[i] + W_DEF * defs[i] + W_IDF * mass[i]) / total,
            });
            hashes.insert(r.id, r.hash);
        }
        Ok(Scan {
            rows,
            fingerprint: hex_digest(hasher.finalize()),
            computed,
            reused,
            fresh,
            words: interner.words,
            seen,
            existing,
            hashes,
        })
    }

    pub fn soul(&self, opts: &SoulOptions) -> Result<Soul, RagError> {
        let scan = self.salience_scan()?;
        let pkey = params_key(opts);
        let gkey = sha_hex(format!("{pkey}|{}", scan.fingerprint).as_bytes());
        if let Some(hit) = self.load_guide(&gkey)? {
            return Ok(hit);
        }
        let mut soul = self.refine(&scan, opts)?;
        soul.cache.features_computed = scan.computed;
        soul.cache.features_reused = scan.reused;
        self.store_guide(&scan, &gkey, &pkey, &soul);
        Ok(soul)
    }

    fn refine(&self, scan: &Scan, opts: &SoulOptions) -> Result<Soul, RagError> {
        let seed = seed_of(&scan.fingerprint);
        let mut pool: Vec<Row> = scan
            .rows
            .iter()
            .filter(|r| opts.focus.as_deref().is_none_or(|p| r.file.starts_with(p)))
            .cloned()
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
            cache: SoulCache {
                rounds_run: rounds,
                ..SoulCache::default()
            },
        })
    }
}

fn drop_guide(
    read: &uriel_core::ReadTx,
    tx: &mut uriel_core::WriteTx,
    id: NodeId,
) -> Result<(), RagError> {
    for edge in read.out_edges(id, Some(HAS_SECTION))? {
        tx.detach_delete_node(edge?.other)?;
    }
    tx.detach_delete_node(id)?;
    Ok(())
}

fn section_props(ord: usize, s: &SoulSection, fingerprint: &str) -> Props {
    let mut p = Props::new();
    p.insert("ord".into(), Value::Int(ord as i64));
    p.insert("chunk".into(), Value::Int(s.chunk_id as i64));
    p.insert("fp".into(), Value::from(fingerprint));
    p.insert("file".into(), Value::from(s.file.as_str()));
    p.insert("start".into(), Value::Int(i64::from(s.start_line)));
    p.insert("end".into(), Value::Int(i64::from(s.end_line)));
    p.insert("level".into(), Value::Int(i64::from(s.level.number())));
    p.insert("score".into(), Value::Float(f64::from(s.score)));
    p.insert("text".into(), Value::from(s.text.as_str()));
    p
}

fn section_of(n: &Node) -> Option<(i64, SoulSection)> {
    let Some(Value::Float(score)) = n.props.get("score") else {
        return None;
    };
    Some((
        int_of(n, "ord")?,
        SoulSection {
            chunk_id: u64::try_from(int_of(n, "chunk")?).ok()?,
            file: str_of(n, "file")?,
            start_line: u32::try_from(int_of(n, "start")?).ok()?,
            end_line: u32::try_from(int_of(n, "end")?).ok()?,
            level: Level::from_number(u64::try_from(int_of(n, "level")?).ok()?)?,
            score: *score as f32,
            text: str_of(n, "text")?,
        },
    ))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use uriel_core::{Graph, MemoryBudget};

    use crate::{IndexOptions, Rag, RagConfig};

    use super::SoulOptions;

    fn write_files(dir: &std::path::Path, n: usize) {
        for i in 0..n {
            let body = format!(
                "/// Stage {i} folds ledger frames into digest windows for the cursor {i}.\npub fn fold_stage_{i}(input: &[u8]) -> usize {{\n    input.len() + {i}\n}}\n"
            );
            fs::write(dir.join(format!("s{i}.rs")), body).expect("write");
        }
    }

    fn rag_with_index_cap(dir: &std::path::Path, index_cap: u64) -> Rag {
        let graph = Graph::open_in_memory(MemoryBudget::mib(64)).expect("graph");
        let rag = Rag::open_capped(graph, RagConfig::default(), index_cap).expect("rag");
        rag.index_dir(dir, &IndexOptions::default()).expect("index");
        rag
    }

    #[test]
    fn the_cache_never_grows_past_its_share_of_the_rag_budget_and_never_changes_the_guide() {
        let dir = tempfile::tempdir().expect("tmp");
        write_files(dir.path(), 60);
        let opts = SoulOptions {
            budget_tokens: 300,
            ..SoulOptions::default()
        };
        let roomy = rag_with_index_cap(dir.path(), 64 << 20).soul(&opts).expect("soul");
        let mut sizes = Vec::new();
        for cap in [2_000u64, 8_000, 24_000, 80_000] {
            let rag = rag_with_index_cap(dir.path(), cap * super::CACHE_DIV);
            let first = rag.soul(&opts).expect("soul");
            let second = rag.soul(&opts).expect("soul");
            let stats = rag.soul_cache_stats().expect("stats");
            assert_eq!(stats.cap, cap);
            assert!(stats.bytes <= cap, "cap {cap}: {stats:?}");
            assert_eq!(first.text, roomy.text, "cap {cap}");
            assert_eq!(second.text, roomy.text, "cap {cap}");
            sizes.push(stats.bytes);
        }
        assert!(sizes[0] < sizes[3], "{sizes:?}");
        assert!(sizes.iter().all(|b| *b > 0), "{sizes:?}");
    }

    #[test]
    fn a_cache_too_small_for_the_guide_never_hits_but_still_answers() {
        let dir = tempfile::tempdir().expect("tmp");
        write_files(dir.path(), 60);
        let rag = rag_with_index_cap(dir.path(), 500 * super::CACHE_DIV);
        let a = rag.soul(&SoulOptions::default()).expect("soul");
        let b = rag.soul(&SoulOptions::default()).expect("soul");
        assert!(!a.cache.hit && !b.cache.hit);
        assert_eq!(a.text, b.text);
        assert_eq!(rag.soul_cache_stats().expect("stats").guides, 0);
    }
}
