use std::path::Path;

use sha2::{Digest, Sha256};
use uriel_core::{Graph, MemoryBudget, NodeId, Props, Value};
use uriel_rag::{Level, Rag, RagConfig, SearchOptions};

const TEXT: &str = "/// Render a table of rows as aligned plain text columns.\npub fn render_table(rows: &[Vec<String>]) -> String {\n    let mut out = String::new();\n    for row in rows {\n        out.push_str(&row.join(\"  \"));\n        out.push('\n');\n    }\n    out\n}";

fn sha256(path: &Path) -> String {
    let bytes = std::fs::read(path).expect("read db");
    Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect()
}

fn old_format_db(path: &Path) -> u64 {
    let graph = Graph::open(path, MemoryBudget::mib(32)).expect("graph");
    let mut tx = graph.begin_write().expect("tx");
    let mut fp = Props::new();
    fp.insert("path".into(), Value::from("old.rs"));
    fp.insert("hash".into(), Value::from("h"));
    fp.insert("kind".into(), Value::from("dir"));
    let file = tx.create_node(&["File"], fp).expect("file");
    let mut cp = Props::new();
    cp.insert("file".into(), Value::from("old.rs"));
    cp.insert("start".into(), Value::Int(1));
    cp.insert("end".into(), Value::Int(9));
    cp.insert("mem".into(), Value::from(&TEXT[..110]));
    cp.insert("text".into(), Value::from(TEXT));
    cp.insert("hash".into(), Value::from("c"));
    cp.insert("utility".into(), Value::Float(0.0));
    let chunk = tx.create_node(&["Chunk"], cp).expect("chunk");
    tx.create_rel(chunk, file, "IN_FILE", Props::new()).expect("rel");
    tx.commit().expect("commit");
    chunk.0
}

fn has_prop(path: &Path, id: u64, key: &str) -> bool {
    let g = Graph::open_read_only(path, MemoryBudget::mib(32)).expect("ro");
    g.begin_read()
        .expect("read")
        .get_node(NodeId(id))
        .expect("get")
        .expect("node")
        .props
        .contains_key(key)
}

fn exercise(rag: &Rag, id: u64) -> (Vec<u64>, String, String, String) {
    let hits = rag
        .search("aligned plain text columns", &SearchOptions::default())
        .expect("search");
    let pack = rag
        .pack_ladder("aligned plain text columns", &SearchOptions::default())
        .expect("pack");
    assert_eq!(pack.hits[0].chunk_id, id);
    (
        hits.iter().map(|h| h.chunk_id).collect(),
        pack.text,
        rag.expand_level(id, Level::L1).expect("l1"),
        rag.expand_level(id, Level::L3).expect("l3"),
    )
}

#[test]
fn a_read_only_rag_session_writes_nothing_and_answers_like_a_writable_one() {
    let dir = tempfile::tempdir().expect("tmp");
    let db = dir.path().join("old.uriel");
    let id = old_format_db(&db);
    let writable_copy = dir.path().join("writable.uriel");
    std::fs::copy(&db, &writable_copy).expect("copy");
    let before = sha256(&db);

    let readonly = {
        let graph = Graph::open_read_only(&db, MemoryBudget::mib(32)).expect("ro graph");
        let rag = Rag::open(graph, RagConfig::default()).expect("ro rag");
        assert!(rag.feedback(&[id], &[]).is_err(), "feedback must be refused");
        assert!(
            rag.index_dir(dir.path(), &uriel_rag::IndexOptions::default())
                .is_err(),
            "indexing must be refused"
        );
        exercise(&rag, id)
    };
    assert_eq!(sha256(&db), before, "the read-only session changed the file");
    assert!(!has_prop(&db, id, "g3") && !has_prop(&db, id, "l1"));

    let writable = {
        let graph = Graph::open(&writable_copy, MemoryBudget::mib(32)).expect("graph");
        let rag = Rag::open(graph, RagConfig::default()).expect("rag");
        exercise(&rag, id)
    };
    assert_ne!(sha256(&writable_copy), before, "the writable session writes, so the check can fail");
    assert!(has_prop(&writable_copy, id, "g3") && has_prop(&writable_copy, id, "l1"));
    assert_eq!(readonly, writable);
}
