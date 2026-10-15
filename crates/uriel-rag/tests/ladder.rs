use std::fs;
use std::path::Path;

use uriel_core::{Graph, NodeId, Props, Value};
use uriel_rag::{IndexOptions, Level, Rag, RagConfig, SearchOptions};

fn tokens(s: &str) -> usize {
    s.chars().count().div_ceil(4)
}

fn corpus(dir: &Path, n: usize) {
    for i in 0..n {
        let mut body = vec![
            format!("/// Compute the running checksum of the input buffer for stream {i}. It resets per block."),
            format!("pub fn checksum_stream_{i}(buffer: &[u8], block: usize) -> u32 {{"),
            "    let mut acc: u32 = 1;".to_string(),
        ];
        for j in 0..24 {
            body.push(format!(
                "    acc = acc.wrapping_mul(31).wrapping_add(buffer[{j} % buffer.len()] as u32 + block as u32 + {i});"
            ));
        }
        body.push("    acc".to_string());
        body.push("}".to_string());
        fs::write(dir.join(format!("s{i}.rs")), body.join("\n")).expect("write");
    }
}

fn indexed(n: usize) -> (tempfile::TempDir, Graph, Rag) {
    let dir = tempfile::tempdir().expect("tempdir");
    corpus(dir.path(), n);
    let graph = Graph::open_in_memory().expect("graph");
    let rag = Rag::open(graph.clone(), RagConfig::default()).expect("rag");
    rag.index_dir(dir.path(), &IndexOptions::default())
        .expect("index");
    (dir, graph, rag)
}

fn opts(k: usize, budget: usize) -> SearchOptions {
    SearchOptions {
        k,
        budget_tokens: budget,
        ..SearchOptions::default()
    }
}

const QUERY: &str = "running checksum of the input buffer for stream";

#[test]
fn the_four_levels_shrink_and_l0_is_the_original() {
    let (dir, _g, rag) = indexed(3);
    let hits = rag.search(QUERY, &opts(1, 2000)).expect("search");
    let id = hits[0].chunk_id;
    let original = fs::read_to_string(dir.path().join(&hits[0].file)).expect("read");
    let l0 = rag.expand_level(id, Level::L0).expect("l0");
    let l1 = rag.expand_level(id, Level::L1).expect("l1");
    let l2 = rag.expand_level(id, Level::L2).expect("l2");
    let l3 = rag.expand_level(id, Level::L3).expect("l3");
    assert_eq!(l0, original, "L0 is the exact original");
    assert_eq!(rag.expand(id).expect("expand"), l0);
    assert_eq!(l2, hits[0].memory, "L2 is the searched memory");
    assert!(l1.len() > l2.len(), "L1 {} vs L2 {}", l1.len(), l2.len());
    assert!(l1.len() < l0.len() / 2, "L1 {} vs L0 {}", l1.len(), l0.len());
    assert!(l2.len() < l1.len() && l3.len() < l2.len());
    assert!(!l3.contains('\n'));
    assert!(tokens(&l3) <= 30, "gist of {} tokens: {l3}", tokens(&l3));
    assert!(l3.starts_with("pub fn checksum_stream_"), "{l3}");
    assert!(l3.contains("Compute the running checksum"), "{l3}");
}

#[test]
fn pack_ladder_assigns_levels_by_rank_and_keeps_citations() {
    let (_d, _g, rag) = indexed(12);
    let o = opts(12, 100_000);
    let pack = rag.pack_ladder(QUERY, &o).expect("pack");
    let plain = rag.search(QUERY, &o).expect("search");
    assert_eq!(pack.hits.len(), plain.len());
    assert_eq!(pack.omitted, 0);
    let levels: Vec<Level> = pack.hits.iter().map(|h| h.level).collect();
    assert_eq!(levels[0], Level::L1);
    assert!(levels[1..4].iter().all(|l| *l == Level::L2), "{levels:?}");
    assert!(levels[4..].iter().all(|l| *l == Level::L3), "{levels:?}");
    for (h, p) in plain.iter().zip(&pack.hits) {
        assert_eq!(h.chunk_id, p.chunk_id, "same order as search");
        let header = format!("// {}:{}-{} #{}", p.file, p.start_line, p.end_line, p.chunk_id);
        assert!(pack.text.contains(&header), "missing {header}");
    }
}

#[test]
fn pack_ladder_never_exceeds_budget_and_degrades_before_dropping() {
    let (_d, _g, rag) = indexed(12);
    let plain = rag.search(QUERY, &opts(12, 0)).expect("search");
    let full = rag.pack_ladder(QUERY, &opts(12, 100_000)).expect("pack");
    let mut saw_degraded = false;
    let mut saw_dropped = false;
    for budget in (0..=full.tokens + 10).step_by(7) {
        let pack = rag.pack_ladder(QUERY, &opts(12, budget)).expect("pack");
        assert!(pack.tokens <= budget, "{} > {budget}", pack.tokens);
        assert_eq!(pack.tokens, tokens(&pack.text));
        assert_eq!(pack.hits.len() + pack.omitted, plain.len());
        for (h, p) in plain.iter().zip(&pack.hits) {
            assert_eq!(h.chunk_id, p.chunk_id, "kept hits are a prefix of search");
            let header = format!("// {}:{}-{} #{}", p.file, p.start_line, p.end_line, p.chunk_id);
            assert!(pack.text.contains(&header), "missing {header}");
        }
        if pack.omitted > 0 {
            saw_dropped = true;
            assert!(
                pack.hits.iter().all(|h| h.level == Level::L3),
                "a hit was dropped while another was above L3 (budget {budget})"
            );
        }
        if pack.hits.len() == plain.len() && pack.hits.iter().any(|h| h.level != full.hits[0].level)
        {
            saw_degraded = true;
        }
    }
    assert!(saw_degraded && saw_dropped, "the sweep must cover both regimes");
    let zero = rag.pack_ladder(QUERY, &opts(12, 0)).expect("pack");
    assert!(zero.hits.is_empty() && zero.text.is_empty());
}

fn old_format_graph() -> (Graph, u64) {
    let graph = Graph::open_in_memory().expect("graph");
    let text = "/// Render a table of rows as aligned plain text columns.\npub fn render_table(rows: &[Vec<String>]) -> String {\n    let mut out = String::new();\n    for row in rows {\n        out.push_str(&row.join(\"  \"));\n        out.push('\\n');\n    }\n    out\n}";
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
    cp.insert(
        "mem".into(),
        Value::from("/// Render a table of rows as aligned plain text columns.\npub fn render_table(rows: &[Vec<String>]) -> String {"),
    );
    cp.insert("text".into(), Value::from(text));
    cp.insert("hash".into(), Value::from("c"));
    cp.insert("utility".into(), Value::Float(0.0));
    let chunk = tx.create_node(&["Chunk"], cp).expect("chunk");
    tx.create_rel(chunk, file, "IN_FILE", Props::new())
        .expect("rel");
    tx.commit().expect("commit");
    (graph, chunk.0)
}

fn has_prop(graph: &Graph, id: u64, key: &str) -> bool {
    graph
        .begin_read()
        .expect("read")
        .get_node(NodeId(id))
        .expect("get")
        .expect("node")
        .props
        .contains_key(key)
}

#[test]
fn an_old_format_chunk_still_works_and_is_upgraded_in_place() {
    let (graph, id) = old_format_graph();
    assert!(!has_prop(&graph, id, "g3") && !has_prop(&graph, id, "l1"));
    let rag = Rag::open(graph.clone(), RagConfig::default()).expect("open old db");
    assert!(has_prop(&graph, id, "g3"), "gist written back on open");
    assert!(!has_prop(&graph, id, "l1"));
    let hits = rag
        .search("aligned plain text columns", &SearchOptions::default())
        .expect("search");
    assert_eq!(hits[0].chunk_id, id);
    let pack = rag
        .pack_ladder("aligned plain text columns", &SearchOptions::default())
        .expect("pack");
    assert_eq!(pack.hits[0].chunk_id, id);
    assert!(pack.text.contains(&format!("// old.rs:1-9 #{id}")));
    assert_eq!(pack.hits[0].level, Level::L1);
    assert!(has_prop(&graph, id, "l1"), "L1 written back on first use");
    let l3 = rag.expand_level(id, Level::L3).expect("l3");
    assert!(
        l3.starts_with("pub fn render_table(rows: &[Vec<String>]) -> String -- Render a table"),
        "{l3}"
    );
    assert!(rag.expand_level(id, Level::L0).expect("l0").ends_with("out\n}"));
    let again = Rag::open(graph, RagConfig::default()).expect("reopen");
    assert_eq!(again.expand_level(id, Level::L3).expect("l3"), l3);
    assert_eq!(
        again
            .expand_level(id, Level::L1)
            .expect("l1"),
        rag.expand_level(id, Level::L1).expect("l1")
    );
}

#[test]
fn unknown_chunk_is_not_found_at_every_level() {
    let (_d, _g, rag) = indexed(1);
    for l in [Level::L0, Level::L1, Level::L2, Level::L3] {
        assert!(rag.expand_level(9_999_999, l).is_err());
    }
}

#[test]
fn fingerprint_tracks_the_stored_text() {
    let (_d, graph, rag) = indexed(2);
    let id = rag.search(QUERY, &opts(1, 2000)).expect("search")[0].chunk_id;
    let a = rag.chunk_fingerprint(id).expect("fp").expect("some");
    assert_eq!(a.len(), 64);
    assert_eq!(rag.chunk_fingerprint(id).expect("fp"), Some(a.clone()));
    let mut tx = graph.begin_write().expect("tx");
    let mut props = Props::new();
    props.insert("text".into(), Value::from("changed text"));
    tx.set_node_props(NodeId(id), props).expect("set");
    tx.commit().expect("commit");
    let b = rag.chunk_fingerprint(id).expect("fp").expect("some");
    assert_ne!(a, b);
    assert_eq!(rag.chunk_fingerprint(9_999_999).expect("fp"), None);
}

#[test]
fn pack_ladder_never_exceeds_budget_over_200_random_budgets() {
    let (_d, _g, rag) = indexed(14);
    let mut state: u64 = 0x2545_F491_4F6C_DD1D;
    let mut next = move || {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        state.wrapping_mul(0x2545_F491_4F6C_DD1D)
    };
    let mut dropped = 0;
    let mut degraded = 0;
    for _ in 0..200 {
        let budget = (next() % 700) as usize;
        let k = 1 + (next() % 14) as usize;
        let pack = rag.pack_ladder(QUERY, &opts(k, budget)).expect("pack");
        assert!(pack.tokens <= budget, "{} > {budget} (k={k})", pack.tokens);
        assert_eq!(pack.tokens, tokens(&pack.text));
        for h in &pack.hits {
            let header = format!("// {}:{}-{} #{}", h.file, h.start_line, h.end_line, h.chunk_id);
            assert!(pack.text.contains(&header), "missing {header}");
        }
        if pack.omitted > 0 {
            dropped += 1;
            assert!(pack.hits.iter().all(|h| h.level == Level::L3), "budget {budget}");
        }
        if pack.hits.iter().any(|h| h.level == Level::L3) && pack.omitted == 0 {
            degraded += 1;
        }
    }
    assert!(dropped > 10 && degraded > 10, "dropped {dropped}, degraded {degraded}");
}
