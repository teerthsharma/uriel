use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use uriel_core::Graph;
use serde_json::Value as Json;

use crate::bm25::Bm25Index;
use crate::chunk::{chunk_text, rust_item_start, ChunkOptions, Lang};
use crate::{walk, IndexOptions, Rag, RagConfig, RagError, SearchOptions};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvalQuery {
    pub query: String,
    pub file: String,
    pub signature_line: u32,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuerySet {
    pub corpus_sha256: String,
    pub files: usize,
    pub queries: Vec<EvalQuery>,
}

pub const FROZEN_SUFFIX: &str = ".frozen";
const MAX_QUERIES: usize = 100_000;

static MATERIALIZED: AtomicU64 = AtomicU64::new(0);

pub struct FrozenCorpus {
    root: PathBuf,
    files: usize,
}

impl FrozenCorpus {
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn files(&self) -> usize {
        self.files
    }
}

impl Drop for FrozenCorpus {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn frozen_files(dir: &Path) -> Result<Vec<(String, PathBuf)>, RagError> {
    let mut files = Vec::new();
    walk(dir, dir, &[], &mut files)?;
    files.sort();
    Ok(files)
}

pub fn corpus_digest(dir: &Path) -> Result<(String, usize), RagError> {
    let files = frozen_files(dir)?;
    let mut bytes: Vec<u8> = Vec::new();
    for (rel, abs) in &files {
        let content = fs::read(abs).map_err(crate::io_err(abs))?;
        bytes.extend_from_slice(&(rel.len() as u64).to_le_bytes());
        bytes.extend_from_slice(rel.as_bytes());
        bytes.extend_from_slice(&(content.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&content);
    }
    Ok((crate::sha_hex(&bytes), files.len()))
}

pub fn materialize(dir: &Path) -> Result<FrozenCorpus, RagError> {
    let files = frozen_files(dir)?;
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let root = std::env::temp_dir().join(format!(
        "uriel-rag-frozen-{}-{nanos}-{}",
        std::process::id(),
        MATERIALIZED.fetch_add(1, Ordering::Relaxed)
    ));
    let corpus = FrozenCorpus {
        root: root.clone(),
        files: files.len(),
    };
    fs::create_dir_all(&root).map_err(crate::io_err(&root))?;
    for (rel, abs) in &files {
        let Some(name) = rel.strip_suffix(FROZEN_SUFFIX) else {
            return Err(crate::corrupt(format!("{rel} lacks the {FROZEN_SUFFIX} suffix")));
        };
        if name.split('/').any(|c| c.is_empty() || c == "." || c == "..") {
            return Err(crate::corrupt(format!("unsafe corpus path {rel}")));
        }
        let dest = name.split('/').fold(root.clone(), |p, c| p.join(c));
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(crate::io_err(parent))?;
        }
        let content = fs::read(abs).map_err(crate::io_err(abs))?;
        fs::write(&dest, content).map_err(crate::io_err(&dest))?;
    }
    Ok(corpus)
}

fn bad_queries(msg: impl Into<String>) -> RagError {
    crate::corrupt(format!("query file: {}", msg.into()))
}

fn json_str(v: &Json, key: &str) -> Result<String, RagError> {
    match v.get(key) {
        Some(Json::String(s)) if !s.is_empty() => Ok(s.clone()),
        _ => Err(bad_queries(format!("`{key}` must be a non-empty string"))),
    }
}

fn json_u32(v: &Json, key: &str) -> Result<u32, RagError> {
    v.get(key)
        .and_then(Json::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| *n >= 1)
        .ok_or_else(|| bad_queries(format!("`{key}` must be a line number")))
}

pub fn parse_queries(text: &str) -> Result<QuerySet, RagError> {
    let root: Json = serde_json::from_str(text).map_err(|e| bad_queries(e.to_string()))?;
    let corpus_sha256 = json_str(&root, "corpus_sha256")?;
    if corpus_sha256.len() != 64 || !corpus_sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(bad_queries("`corpus_sha256` must be 64 hex digits"));
    }
    let files = root
        .get("files")
        .and_then(Json::as_u64)
        .and_then(|n| usize::try_from(n).ok())
        .ok_or_else(|| bad_queries("`files` must be a count"))?;
    let items = root
        .get("queries")
        .and_then(Json::as_array)
        .ok_or_else(|| bad_queries("`queries` must be an array"))?;
    if items.len() > MAX_QUERIES {
        return Err(bad_queries("too many queries"));
    }
    let mut queries = Vec::with_capacity(items.len());
    for item in items {
        queries.push(EvalQuery {
            query: json_str(item, "query")?,
            file: json_str(item, "file")?,
            signature_line: json_u32(item, "signature_line")?,
            name: json_str(item, "name")?,
        });
    }
    Ok(QuerySet {
        corpus_sha256,
        files,
        queries,
    })
}

pub fn queries_json(set: &QuerySet) -> String {
    let queries: Vec<Json> = set
        .queries
        .iter()
        .map(|q| {
            serde_json::json!({
                "query": q.query,
                "file": q.file,
                "signature_line": q.signature_line,
                "name": q.name,
            })
        })
        .collect();
    let doc = serde_json::json!({
        "corpus_sha256": set.corpus_sha256,
        "files": set.files,
        "queries": queries,
    });
    let mut text = serde_json::to_string_pretty(&doc).unwrap_or_default();
    text.push('\n');
    text
}

pub fn generate_queries(dir: &Path, n: usize) -> Result<QuerySet, RagError> {
    let (corpus_sha256, files) = corpus_digest(dir)?;
    let corpus = materialize(dir)?;
    let candidates = candidate_queries(corpus.root())?;
    Ok(QuerySet {
        corpus_sha256,
        files,
        queries: sample(&candidates, n),
    })
}

pub fn run_frozen(corpus_dir: &Path, queries_path: &Path) -> Result<EvalReport, RagError> {
    let text = fs::read_to_string(queries_path).map_err(crate::io_err(queries_path))?;
    let set = parse_queries(&text)?;
    let (digest, files) = corpus_digest(corpus_dir)?;
    if digest != set.corpus_sha256 || files != set.files {
        return Err(crate::corrupt(format!(
            "frozen corpus drifted: {files} files {digest}, query file expects {} files {}",
            set.files, set.corpus_sha256
        )));
    }
    let corpus = materialize(corpus_dir)?;
    run_on(corpus.root(), &set.queries)
}
#[derive(Debug, Clone)]
pub struct EvalReport {
    pub files: usize,
    pub chunks: usize,
    pub candidates: usize,
    pub queries: usize,
    pub baseline_recall_at_5: f64,
    pub baseline_recall_at_10: f64,
    pub rag_recall_at_5: f64,
    pub rag_recall_at_10: f64,
    pub ladder_recall_at_5: f64,
    pub ladder_recall_at_10: f64,
    pub compression_ratio: f64,
    pub text_bytes: u64,
    pub memory_bytes: u64,
    pub rag_index_bytes: u64,
    pub baseline_index_bytes: u64,
}

impl fmt::Display for EvalReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "corpus: {} files, {} chunks; {} candidate functions, {} queries",
            self.files, self.chunks, self.candidates, self.queries
        )?;
        writeln!(f)?;
        writeln!(f, "| system | recall@5 | recall@10 |")?;
        writeln!(f, "|---|---|---|")?;
        writeln!(
            f,
            "| BM25 over full chunk text (baseline) | {:.3} | {:.3} |",
            self.baseline_recall_at_5, self.baseline_recall_at_10
        )?;
        writeln!(
            f,
            "| uriel-rag hybrid over compressed memories | {:.3} | {:.3} |",
            self.rag_recall_at_5, self.rag_recall_at_10
        )?;
        writeln!(
            f,
            "| same hits through pack_ladder (presentation only) | {:.3} | {:.3} |",
            self.ladder_recall_at_5, self.ladder_recall_at_10
        )?;
        writeln!(f)?;
        writeln!(
            f,
            "compression ratio (text bytes / memory bytes): {:.2}x",
            self.compression_ratio
        )?;
        writeln!(
            f,
            "bytes: full text {}, memories {}, uriel-rag index {}, baseline index {}",
            self.text_bytes, self.memory_bytes, self.rag_index_bytes, self.baseline_index_bytes
        )
    }
}

fn strip_backticks(s: &str) -> String {
    let mut out = String::new();
    let mut inside = false;
    for c in s.chars() {
        if c == '`' {
            inside = !inside;
            out.push(' ');
        } else if !inside {
            out.push(c);
        }
    }
    out
}

pub fn strip_identifiers(doc: &str, fn_name: &str) -> Option<String> {
    let name_parts: Vec<String> = fn_name
        .split('_')
        .filter(|p| p.chars().count() >= 3)
        .map(str::to_lowercase)
        .collect();
    let lowered_name = fn_name.to_lowercase();
    let mut words = Vec::new();
    for tok in strip_backticks(doc).split_whitespace() {
        let w = tok.trim_matches(|c: char| !c.is_alphanumeric());
        if w.is_empty() {
            continue;
        }
        if tok.contains('_')
            || tok.contains("::")
            || tok.contains('/')
            || tok.contains('<')
            || tok.contains('>')
            || tok.contains('#')
            || tok.contains('(')
            || tok.contains('[')
        {
            continue;
        }
        let tail_upper = w.chars().skip(1).any(char::is_uppercase);
        if tail_upper {
            continue;
        }
        let lw = w.to_lowercase();
        if lw == lowered_name || name_parts.contains(&lw) {
            continue;
        }
        words.push(w.to_string());
    }
    (words.len() >= 6).then(|| words.join(" "))
}

fn documented_functions(text: &str) -> Vec<(u32, String, String)> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let t = lines[i].trim_start();
        if !(t.starts_with("///") && !t.starts_with("////")) {
            i += 1;
            continue;
        }
        let mut doc = Vec::new();
        let mut in_fence = false;
        while i < lines.len() {
            let t = lines[i].trim_start();
            let Some(body) = t.strip_prefix("///") else {
                break;
            };
            if body.trim_start().starts_with("```") {
                in_fence = !in_fence;
            } else if !in_fence {
                doc.push(body.trim());
            }
            i += 1;
        }
        while i < lines.len() && lines[i].trim_start().starts_with("#[") {
            i += 1;
        }
        if i < lines.len() && rust_item_start(lines[i]) {
            let sig = lines[i];
            if let Some(pos) = sig.find("fn ") {
                let name: String = sig[pos + 3..]
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                if !name.is_empty() {
                    out.push((
                        u32::try_from(i + 1).unwrap_or(u32::MAX),
                        name,
                        doc.join(" "),
                    ));
                }
            }
        }
    }
    out
}

fn corpus_chunks(root: &Path) -> Result<Vec<(String, crate::chunk::RawChunk)>, RagError> {
    let mut files = Vec::new();
    walk(root, root, crate::SKIP_DIRS, &mut files)?;
    let mut out = Vec::new();
    for (rel, abs) in files {
        let Ok(meta) = fs::metadata(&abs) else {
            continue;
        };
        if meta.len() > RagConfig::default().max_file_bytes {
            continue;
        }
        let Ok(bytes) = fs::read(&abs) else {
            continue;
        };
        if bytes.iter().take(8000).any(|b| *b == 0) {
            continue;
        }
        let Ok(text) = String::from_utf8(bytes) else {
            continue;
        };
        for c in chunk_text(&text, Lang::from_path(&rel), &ChunkOptions::default()) {
            out.push((rel.clone(), c));
        }
    }
    Ok(out)
}

pub fn candidate_queries(root: &Path) -> Result<Vec<EvalQuery>, RagError> {
    let mut files = Vec::new();
    walk(root, root, crate::SKIP_DIRS, &mut files)?;
    let mut out = Vec::new();
    for (rel, abs) in files {
        if !rel.ends_with(".rs") {
            continue;
        }
        let Ok(text) = fs::read_to_string(&abs) else {
            continue;
        };
        for (line, name, doc) in documented_functions(&text) {
            if let Some(query) = strip_identifiers(&doc, &name) {
                out.push(EvalQuery {
                    query,
                    file: rel.clone(),
                    signature_line: line,
                    name,
                });
            }
        }
    }
    out.sort_by(|a, b| (&a.file, a.signature_line).cmp(&(&b.file, b.signature_line)));
    Ok(out)
}

pub fn sample(candidates: &[EvalQuery], n: usize) -> Vec<EvalQuery> {
    if candidates.len() <= n {
        return candidates.to_vec();
    }
    (0..n)
        .map(|i| candidates[i * candidates.len() / n].clone())
        .collect()
}

fn run_on(root: &Path, queries: &[EvalQuery]) -> Result<EvalReport, RagError> {
    let candidates = candidate_queries(root)?;

    let chunks = corpus_chunks(root)?;
    let mut baseline = Bm25Index::new();
    for (_, c) in &chunks {
        baseline.add(&c.text);
    }
    let contains = |slot: u32, q: &EvalQuery| -> bool {
        chunks.get(slot as usize).is_some_and(|(f, c)| {
            *f == q.file && c.start_line <= q.signature_line && q.signature_line <= c.end_line
        })
    };

    let rag = Rag::open(Graph::open_in_memory(uriel_core::MemoryBudget::mib(80))?, RagConfig::default())?;
    rag.index_dir(root, &IndexOptions::default())?;
    let stats = rag.stats()?;
    let opts = SearchOptions {
        k: 10,
        expand_hops: 0,
        use_utility: false,
        ..SearchOptions::default()
    };

    let (mut b5, mut b10, mut r5, mut r10) = (0usize, 0usize, 0usize, 0usize);
    let (mut l5, mut l10) = (0usize, 0usize);
    for q in queries {
        let base = baseline.search(&q.query, 10);
        let pos = base.iter().position(|(slot, _)| contains(*slot, q));
        b5 += usize::from(pos.is_some_and(|p| p < 5));
        b10 += usize::from(pos.is_some());
        let hits = rag.search(&q.query, &opts)?;
        let pos = hits.iter().position(|h| {
            h.file == q.file && h.start_line <= q.signature_line && q.signature_line <= h.end_line
        });
        r5 += usize::from(pos.is_some_and(|p| p < 5));
        r10 += usize::from(pos.is_some());
        let pack = rag.pack_ladder(&q.query, &opts)?;
        let pos = pack.hits.iter().position(|h| {
            h.file == q.file && h.start_line <= q.signature_line && q.signature_line <= h.end_line
        });
        l5 += usize::from(pos.is_some_and(|p| p < 5));
        l10 += usize::from(pos.is_some());
    }
    let n = queries.len().max(1) as f64;
    Ok(EvalReport {
        files: stats.files,
        chunks: stats.chunks,
        candidates: candidates.len(),
        queries: queries.len(),
        baseline_recall_at_5: b5 as f64 / n,
        baseline_recall_at_10: b10 as f64 / n,
        rag_recall_at_5: r5 as f64 / n,
        rag_recall_at_10: r10 as f64 / n,
        ladder_recall_at_5: l5 as f64 / n,
        ladder_recall_at_10: l10 as f64 / n,
        compression_ratio: stats.avg_compression_ratio,
        text_bytes: stats.text_bytes,
        memory_bytes: stats.memory_bytes,
        rag_index_bytes: stats.index_bytes,
        baseline_index_bytes: baseline.approx_bytes(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_are_removed_from_queries() {
        let doc = "Parses the `Config` file and returns parse_config values using HttpClient::new for every request made";
        let q = strip_identifiers(doc, "parse_config").expect("enough words");
        for banned in ["Config", "parse_config", "HttpClient", "new", "parse"] {
            assert!(
                !q.split_whitespace().any(|w| w == banned),
                "{banned} in {q}"
            );
        }
        assert!(q.contains("returns"));
        assert!(strip_identifiers("too short here", "x").is_none());
    }

    #[test]
    fn finds_documented_functions_and_skips_code_fences() {
        let src = "/// Does a thing\n/// over two lines\n/// ```\n/// let x = 1;\n/// ```\n#[inline]\npub fn thing() {}\n\nfn undocumented() {}\n";
        let f = documented_functions(src);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].0, 7);
        assert_eq!(f[0].1, "thing");
        assert_eq!(f[0].2, "Does a thing over two lines");
    }

    #[test]
    fn sampling_is_even_and_deterministic() {
        let c: Vec<EvalQuery> = (0..250)
            .map(|i| EvalQuery {
                query: String::new(),
                file: "f.rs".into(),
                signature_line: i,
                name: String::new(),
            })
            .collect();
        let s = sample(&c, 100);
        assert_eq!(s.len(), 100);
        assert_eq!(s, sample(&c, 100));
        assert_eq!(s[0].signature_line, 0);
        assert_eq!(s[99].signature_line, 247);
        assert_eq!(sample(&c[..10], 100).len(), 10);
    }
}
