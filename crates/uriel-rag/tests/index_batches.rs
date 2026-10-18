use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use uriel_core::{Graph, MemoryBudget};
use uriel_rag::{IndexOptions, Rag, RagConfig};

const FILES: usize = 240;
const LINES: usize = 400;
const MAX_CHUNKS_PER_COMMIT: usize = 1024;

fn write_tree(root: &std::path::Path) {
    for f in 0..FILES {
        let body: String = (0..LINES)
            .map(|l| {
                format!(
                    "alpha{f} beta{l} gamma{} delta{}\n",
                    f * LINES + l,
                    (f + l) % 97
                )
            })
            .collect();
        std::fs::write(root.join(format!("part_{f}.txt")), body).expect("write");
    }
}

#[test]
fn indexing_a_large_tree_commits_in_bounded_batches_and_a_rerun_commits_nothing() {
    let docs = tempfile::tempdir().expect("docs");
    write_tree(docs.path());
    let db = tempfile::tempdir().expect("db");
    let graph = Graph::open(db.path().join("g.uriel"), MemoryBudget::mib(64)).expect("graph");
    let rag = Rag::open(graph.clone(), RagConfig::default()).expect("rag");
    let commits = Arc::new(AtomicUsize::new(0));
    let seen = commits.clone();
    graph.on_commit(Box::new(move |_| {
        seen.fetch_add(1, Ordering::SeqCst);
    }));
    let report = rag
        .index_dir(docs.path(), &IndexOptions::default())
        .expect("index");
    assert_eq!(report.files_indexed, FILES);
    assert!(
        report.chunks_added > 2 * MAX_CHUNKS_PER_COMMIT,
        "{}",
        report.chunks_added
    );
    let first = commits.load(Ordering::SeqCst);
    assert!(
        first * MAX_CHUNKS_PER_COMMIT >= report.chunks_added,
        "{first} commits carried {} chunks",
        report.chunks_added
    );
    assert!(
        first >= 3,
        "{first} commits for {} chunks",
        report.chunks_added
    );
    let again = rag
        .index_dir(docs.path(), &IndexOptions::default())
        .expect("reindex");
    assert_eq!(again.files_unchanged, FILES);
    assert_eq!(again.files_indexed, 0);
    assert_eq!(commits.load(Ordering::SeqCst), first);
    assert_eq!(rag.stats().expect("stats").chunks, report.chunks_added);
}
