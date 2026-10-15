use std::fs;
use std::path::{Path, PathBuf};

use uriel_rag::eval::{
    corpus_digest, generate_queries, parse_queries, queries_json, run_frozen, QuerySet,
};

fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/frozen_corpus")
}

fn queries_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/eval_queries.json")
}

fn all_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = fs::read_dir(dir)
        .expect("read_dir")
        .map(|e| e.expect("entry").path())
        .collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            all_files(&p, out);
        } else {
            out.push(p);
        }
    }
}

fn load() -> QuerySet {
    let text = fs::read_to_string(queries_path()).expect("tests/eval_queries.json is checked in");
    parse_queries(&text).expect("valid query file")
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

#[test]
fn frozen_corpus_holds_sources_only() {
    let mut files = Vec::new();
    all_files(&corpus_dir(), &mut files);
    assert!(files.len() >= 150, "{} files", files.len());
    for f in &files {
        let rel = f.strip_prefix(corpus_dir()).expect("inside").to_string_lossy().replace('\\', "/");
        assert!(rel.ends_with(".frozen"), "{rel}");
        for banned in ["results.json", "queries.json", "calibration.json"] {
            assert!(!rel.ends_with(&format!("{banned}.frozen")), "{rel}");
        }
        assert!(!rel.split('/').any(|c| c == "target" || c == ".git"), "{rel}");
    }
    assert!(corpus_dir().join("seal-core/src/graph.rs.frozen").is_file());
    assert!(corpus_dir().join("seal-rag/src/lib.rs.frozen").is_file());
}

#[test]
fn query_file_is_valid_against_the_corpus() {
    let set = load();
    assert_eq!(set.queries.len(), 100);
    for q in &set.queries {
        assert!(q.query.split_whitespace().count() >= 6, "{}", q.query);
        let path = corpus_dir().join(format!("{}.frozen", q.file));
        let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", q.file));
        let line = text
            .lines()
            .nth(q.signature_line as usize - 1)
            .unwrap_or_else(|| panic!("{}:{} past the end", q.file, q.signature_line));
        assert!(line.contains(&format!("fn {}", q.name)), "{}:{} {line}", q.file, q.signature_line);
    }
    let mut seen: Vec<_> = set.queries.iter().map(|q| (&q.file, q.signature_line)).collect();
    seen.sort();
    seen.dedup();
    assert_eq!(seen.len(), 100);
}

#[test]
fn corpus_digest_matches_the_recorded_one() {
    let set = load();
    let (digest, files) = corpus_digest(&corpus_dir()).expect("digest");
    assert_eq!(digest, set.corpus_sha256);
    assert_eq!(files, set.files);
    assert_eq!(digest.len(), 64);
}

#[test]
fn regenerating_reproduces_the_checked_in_query_file() {
    let fresh = generate_queries(&corpus_dir(), 100).expect("generate");
    assert_eq!(fresh, load());
    let on_disk = fs::read_to_string(queries_path()).expect("read");
    assert_eq!(queries_json(&fresh), on_disk);
    assert_eq!(generate_queries(&corpus_dir(), 100).expect("again"), fresh);
}

#[test]
fn a_drifted_corpus_is_refused() {
    let tmp = tempfile::tempdir().expect("tmp");
    let mut files = Vec::new();
    all_files(&corpus_dir(), &mut files);
    for f in &files {
        let rel = f.strip_prefix(corpus_dir()).expect("inside");
        let dest = tmp.path().join(rel);
        fs::create_dir_all(dest.parent().expect("parent")).expect("mkdir");
        fs::copy(f, &dest).expect("copy");
    }
    let (clean, _) = corpus_digest(tmp.path()).expect("digest");
    assert_eq!(clean, load().corpus_sha256);
    let victim = tmp.path().join("seal-core/src/graph.rs.frozen");
    let mut bytes = fs::read(&victim).expect("read");
    bytes.push(b'\n');
    fs::write(&victim, bytes).expect("write");
    let err = run_frozen(tmp.path(), &queries_path()).expect_err("drift");
    assert!(err.to_string().contains("drifted"), "{err}");
}

#[test]
fn query_loader_survives_5000_adversarial_inputs() {
    let valid = fs::read_to_string(queries_path()).expect("read");
    let mut r = Rng(0xBAD5_EED5_0000_0001);
    let pieces = [
        "{", "}", "[", "]", ",", ":", "\"", "null", "-1", "4294967296", "18446744073709551616", "\"queries\"",
        "\"file\"", "\\u0000", "\u{e9}", "\u{1F600}", "[[]]", "{}", "1e999", "\"corpus_sha256\"",
    ];
    let (mut ok, mut err) = (0, 0);
    for i in 0..5000 {
        let text = if i % 4 == 0 {
            (0..r.below(30)).map(|_| pieces[r.below(pieces.len())]).collect::<String>()
        } else {
            let mut chars: Vec<char> = valid.chars().collect();
            for _ in 0..1 + r.below(6) {
                let at = r.below(chars.len());
                match r.below(3) {
                    0 => {
                        chars.remove(at);
                    }
                    1 => {
                        let p: Vec<char> = pieces[r.below(pieces.len())].chars().collect();
                        chars.splice(at..at, p);
                    }
                    _ => chars[at] = pieces[r.below(pieces.len())].chars().next().unwrap_or(' '),
                }
            }
            chars.into_iter().collect()
        };
        match parse_queries(&text) {
            Ok(_) => ok += 1,
            Err(_) => err += 1,
        }
    }
    assert!(err > 1000, "ok {ok} err {err}");
    for bad in ["", "null", "[]", "{}", "{\"queries\":[]}", "{\"corpus_sha256\":1,\"files\":1,\"queries\":[]}"] {
        assert!(parse_queries(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn query_file_roundtrips_through_the_loader() {
    let set = load();
    let text = queries_json(&set);
    assert_eq!(parse_queries(&text).expect("parse"), set);
}
