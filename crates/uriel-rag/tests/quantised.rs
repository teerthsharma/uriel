use std::fs;
use std::path::{Path, PathBuf};

use uriel_rag::eval::{generate_queries, parse_queries, queries_json, run_frozen, EvalReport};

const F32_RECALL_AT_5: f64 = 0.880;
const F32_RECALL_AT_10: f64 = 0.890;
const RERANKED_RECALL_AT_5: f64 = 0.95;
const COMPRESSION_TARGET: f64 = 16.0;

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

fn frozen_report() -> EvalReport {
    let tests = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let corpus = tests.join("frozen_corpus");
    let tmp = tempfile::tempdir().expect("tmp");
    let corpus_copy = tmp.path().join("corpus");
    let mut files = Vec::new();
    all_files(&corpus, &mut files);
    for f in &files {
        let rel = f.strip_prefix(&corpus).expect("inside");
        let dest = corpus_copy.join(rel);
        fs::create_dir_all(dest.parent().expect("parent")).expect("mkdir");
        let bytes = fs::read(f).expect("read");
        let text = String::from_utf8(bytes).expect("utf8").replace("\r\n", "\n");
        fs::write(&dest, text).expect("write");
    }
    let fresh = generate_queries(&corpus_copy, 100).expect("queries");
    let recorded = fs::read_to_string(tests.join("eval_queries.json")).expect("recorded");
    assert_eq!(
        fresh.queries,
        parse_queries(&recorded).expect("parse").queries,
        "the regenerated queries must be the checked-in ones"
    );
    let queries = tmp.path().join("queries.json");
    fs::write(&queries, queries_json(&fresh)).expect("write queries");
    run_frozen(&corpus_copy, &queries).expect("eval")
}

#[test]
fn frozen_corpus_recall_is_not_below_the_f32_baseline() {
    let r = frozen_report();
    println!(
        "FROZEN recall@5={:.3} recall@10={:.3} queries={} chunks={}",
        r.rag_recall_at_5, r.rag_recall_at_10, r.queries, r.chunks
    );
    assert_eq!(r.queries, 100);
    assert_eq!(r.chunks, 3534);
    assert!(
        r.rag_recall_at_5 >= F32_RECALL_AT_5 - 1e-9,
        "recall@5 {} fell below the f32 baseline {F32_RECALL_AT_5}",
        r.rag_recall_at_5
    );
    assert!(
        r.rag_recall_at_10 >= F32_RECALL_AT_10 - 1e-9,
        "recall@10 {} fell below the f32 baseline {F32_RECALL_AT_10}",
        r.rag_recall_at_10
    );
}

#[test]
fn frozen_recall_after_rerank_reaches_the_target() {
    let r = frozen_report();
    println!("{r}");
    assert!(
        r.rag_recall_at_5 >= RERANKED_RECALL_AT_5 - 1e-9,
        "recall@5 {} below {RERANKED_RECALL_AT_5}",
        r.rag_recall_at_5
    );
}

#[test]
fn frozen_compression_ratio_reaches_the_target() {
    let r = frozen_report();
    assert!(
        r.compression_ratio >= COMPRESSION_TARGET,
        "compression {:.2}x below {COMPRESSION_TARGET}x",
        r.compression_ratio
    );
}
