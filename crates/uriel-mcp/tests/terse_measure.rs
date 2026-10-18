use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::{json, Value};
use uriel_terse::estimate_tokens;

mod common;

use common::Session;

const FINDINGS: [&str; 2] = [
    "We should basically make sure that the retrieval index really does always rebuild the chunk table after a crash, in order to avoid serving stale citations, because it is just a very important thing to remember about the recovery path.",
    "I think that it would probably be a good idea to actually keep the scoring weights in one place, so that we do not have to change them in a lot of different files whenever we want to try out a different ranking.",
];

fn calls(json_format: bool) -> Vec<(&'static str, &'static str, Value)> {
    let f = |mut v: Value| {
        if json_format {
            v["format"] = json!("json");
        }
        v
    };
    vec![
        ("index", "rag_index", json!({"path": "crates"})),
        ("schema", "get_neo4j_schema", json!({})),
        (
            "cypher",
            "read_neo4j_cypher",
            f(json!({"query": "MATCH (c:Chunk) RETURN c.file AS file, c.start AS start, c.`end` AS stop ORDER BY file, start LIMIT 60"})),
        ),
        (
            "cypher",
            "read_neo4j_cypher",
            f(json!({"query": "MATCH (n) RETURN labels(n) AS labels, count(n) AS n ORDER BY n DESC"})),
        ),
        (
            "cypher",
            "read_neo4j_cypher",
            f(json!({"query": "MATCH (a)-[r]->(b) RETURN type(r) AS rel, count(r) AS n ORDER BY n DESC"})),
        ),
        (
            "cypher",
            "read_neo4j_cypher",
            f(json!({"query": "MATCH (c:Chunk) RETURN c.file AS file, count(c) AS chunks ORDER BY chunks DESC, file LIMIT 40"})),
        ),
        (
            "cypher",
            "read_neo4j_cypher",
            f(json!({"query": "MATCH (c:Chunk) WHERE c.file CONTAINS 'uriel-terse' RETURN c.file AS file, c.start AS start ORDER BY file, start LIMIT 50"})),
        ),
        (
            "cypher",
            "read_neo4j_cypher",
            f(json!({"query": "MATCH (c:Chunk) RETURN c.file AS file, c.`end` - c.start AS span ORDER BY span DESC, file LIMIT 30"})),
        ),
        (
            "cypher_write",
            "write_neo4j_cypher",
            json!({"query": "CREATE (:Probe {name: 'p1', kind: 'x'}), (:Probe {name: 'p2', kind: 'x'})"}),
        ),
        (
            "graph",
            "create_entities",
            json!({"entities": [
                {"name": "uriel-terse", "type": "Crate", "observations": ["lossless data encoding", "lossy prose compaction"]},
                {"name": "uriel-mcp", "type": "Crate", "observations": ["stdio server", "tool results"]},
                {"name": "uriel-rag", "type": "Crate", "observations": ["hybrid retrieval", "resolution ladder"]},
            ]}),
        ),
        (
            "graph",
            "create_relations",
            json!({"relations": [
                {"source": "uriel-mcp", "target": "uriel-terse", "relationType": "DEPENDS_ON"},
                {"source": "uriel-mcp", "target": "uriel-rag", "relationType": "DEPENDS_ON"},
            ]}),
        ),
        ("graph", "read_graph", json!({})),
        ("graph", "search_memories", json!({"query": "retrieval"})),
        ("graph", "find_memories_by_name", json!({"names": ["uriel-terse", "uriel-mcp", "uriel-rag"]})),
        ("house", "house_status", json!({})),
        ("house", "house_config", json!({})),
        (
            "research",
            "research_orbit_audit",
            json!({"injective": true, "relation": [
                {"entity": "a", "answer": "x"}, {"entity": "b", "answer": "y"},
                {"entity": "c", "answer": "x"}, {"entity": "d", "answer": "z"},
                {"entity": "e", "answer": "w"}, {"entity": "f", "answer": "y"},
            ]}),
        ),
        (
            "research",
            "research_pooling_audit",
            json!({"recoveries": [{"entity": "a", "recovery": 0.9}, {"entity": "b", "recovery": 0.4}, {"entity": "e", "recovery": 0.7}],
                "relation": [
                    {"entity": "a", "answer": "x"}, {"entity": "b", "answer": "x"},
                    {"entity": "c", "answer": "y"}, {"entity": "e", "answer": "y"},
                ]}),
        ),
        ("remember", "seal_remember", json!({"text": FINDINGS[0], "tags": ["recovery"]})),
        ("remember", "seal_remember", json!({"text": FINDINGS[1], "tags": ["ranking"]})),
        (
            "recall",
            "seal_recall",
            f(json!({"query": "rebuild chunk table after crash stale citations"})),
        ),
        ("recall", "seal_recall", f(json!({"query": "scoring weights ranking one place"}))),
        ("recall", "seal_recall", f(json!({"query": "resolution ladder", "k": 6}))),
        ("view", "view", f(json!({"question": "how are tool results encoded", "budget_tokens": 2500}))),
        ("view", "view", f(json!({"question": "token estimate chars per token", "budget_tokens": 2500}))),
        ("view", "view", f(json!({"question": "protected spans in prose compaction", "budget_tokens": 2500}))),
        ("rag_search", "rag_search", f(json!({"query": "bm25 tokenizer"}))),
        ("rag_search", "rag_search", f(json!({"query": "session dedup delivered chunks", "k": 10}))),
        ("rag_search", "rag_search", f(json!({"query": "memory budget page cache"}))),
        ("soul", "soul", f(json!({"budget_tokens": 1200}))),
    ]
}

struct Arm {
    label: &'static str,
    terse: &'static str,
    json: bool,
}

const ARMS: [Arm; 6] = [
    Arm { label: "off-default", terse: "off", json: false },
    Arm { label: "off-json", terse: "off", json: true },
    Arm { label: "data-default", terse: "data", json: false },
    Arm { label: "data-json", terse: "data", json: true },
    Arm { label: "prose-default", terse: "prose", json: false },
    Arm { label: "prose-json", terse: "prose", json: true },
];

fn session(root: &str, terse: &str) -> Session {
    Session::start(
        &[
            "--root".to_string(),
            root.to_string(),
            "--in-memory".to_string(),
            "--mode".to_string(),
            "house".to_string(),
            "--memory-mib".to_string(),
            "256".to_string(),
            "--terse".to_string(),
            terse.to_string(),
        ],
        &prefix_env(),
    )
}

fn prefix_env() -> Vec<(&'static str, &'static str)> {
    match std::env::var("TERSE_MEASURE_PREFIX_OFF") {
        Ok(_) => vec![("URIEL_STABLE_PREFIX", "off")],
        Err(_) => Vec::new(),
    }
}

#[test]
#[ignore]
fn measure_the_three_terse_levels_on_the_repository_index() {
    let root = std::env::var("TERSE_MEASURE_ROOT").expect("TERSE_MEASURE_ROOT");
    let dump = std::env::var("TERSE_MEASURE_OUT").ok().map(PathBuf::from);
    let mut rows: Vec<(&str, &str, Vec<usize>)> = Vec::new();
    let mut texts: Vec<Vec<String>> = Vec::new();
    for arm in &ARMS {
        let mut s = session(&root, arm.terse);
        let mut column = Vec::new();
        for (_, name, args) in calls(arm.json) {
            column.push(s.text(name, args));
        }
        if arm.terse != "off" {
            let stats: Value = serde_json::from_str(&s.text("terse_stats", json!({}))).expect("stats");
            println!("stats {}: {stats}", arm.label);
        }
        texts.push(column);
    }
    for (i, (class, name, _)) in calls(true).into_iter().enumerate() {
        rows.push((class, name, texts.iter().map(|c| estimate_tokens(&c[i])).collect()));
    }
    println!("| # | class | tool | {} |", ARMS.iter().map(|a| a.label).collect::<Vec<_>>().join(" | "));
    for (i, (class, name, t)) in rows.iter().enumerate() {
        println!("| {} | {class} | {name} | {} |", i + 1, t.iter().map(|n| n.to_string()).collect::<Vec<_>>().join(" | "));
    }
    let mut by_class: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (class, _, t) in &rows {
        let e = by_class.entry(class).or_insert_with(|| vec![0; ARMS.len()]);
        for (a, b) in e.iter_mut().zip(t) {
            *a += b;
        }
    }
    println!("| class | {} |", ARMS.iter().map(|a| a.label).collect::<Vec<_>>().join(" | "));
    for (class, t) in &by_class {
        println!("| {class} | {} |", t.iter().map(|n| n.to_string()).collect::<Vec<_>>().join(" | "));
    }
    let total: Vec<usize> = (0..ARMS.len()).map(|a| rows.iter().map(|r| r.2[a]).sum()).collect();
    println!("| TOTAL | {} |", total.iter().map(|n| n.to_string()).collect::<Vec<_>>().join(" | "));
    let ratio = |a: usize, b: usize| 1.0 - total[a] as f64 / total[b] as f64;
    println!("saved vs off-default: data {:.4} prose {:.4}", ratio(2, 0), ratio(4, 0));
    println!("saved vs off-json: data-json {:.4} prose-json {:.4}", ratio(3, 1), ratio(5, 1));
    for (class, t) in &by_class {
        assert!(t[2] <= t[0], "{class}: data-default {} > off-default {}", t[2], t[0]);
        assert!(t[4] <= t[0], "{class}: prose-default {} > off-default {}", t[4], t[0]);
        assert!(t[3] <= t[1], "{class}: data-json {} > off-json {}", t[3], t[1]);
    }
    assert!(total[2] <= total[0] && total[4] <= total[0]);
    if let Some(dir) = dump {
        std::fs::create_dir_all(&dir).expect("dir");
        for (i, (class, name, _)) in calls(true).into_iter().enumerate() {
            for (a, arm) in ARMS.iter().enumerate() {
                std::fs::write(dir.join(format!("{:02}-{class}-{name}-{}.txt", i + 1, arm.label)), &texts[a][i]).expect("write");
            }
        }
    }
}
