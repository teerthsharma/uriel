mod common;

use std::fs;
use std::path::Path;

use common::Session;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uriel_mcp::compact::decode_pack;

const WORDS: [&str; 12] = [
    "routes", "frames", "ledger", "window", "cursor", "digest", "packet", "socket", "buffer",
    "kernel", "schema", "stream",
];

fn write_corpus(dir: &Path) {
    for i in 0..30usize {
        let n = 2 + (i * 7) % 9;
        let doc = (0..n)
            .map(|k| WORDS[(i * 5 + k * 3) % WORDS.len()])
            .collect::<Vec<_>>()
            .join(" ");
        let next = (i + 1) % 30;
        let sub = if i % 2 == 0 { "core" } else { "edge" };
        fs::create_dir_all(dir.join(sub)).expect("mkdir");
        fs::write(
            dir.join(sub).join(format!("stage_{i}.rs")),
            format!(
                "/// Handle {doc} for stage {i}.\npub fn handle_stage_{i}(input: &[u8]) -> usize {{\n    handle_stage_{next}(input) + input.len()\n}}\n"
            ),
        )
        .expect("write");
    }
    fs::write(
        dir.join("README.md"),
        "# Pipeline\n\nA staged byte pipeline where every stage hands its input to the next.\n",
    )
    .expect("readme");
}

fn render(x: &Value) -> String {
    let head = format!(
        "// {}:{}-{} #{}",
        x["file"].as_str().expect("file"),
        x["start_line"],
        x["end_line"],
        x["chunk_id"]
    );
    if x["level"] == 3 {
        format!("{head} {}
", x["text"].as_str().expect("text"))
    } else {
        format!("{head}
{}
", x["text"].as_str().expect("text"))
    }
}

fn cited(compact: &str) -> Vec<(u64, String, u32, u32)> {
    decode_pack(compact)
        .expect("a compact pack")
        .hits
        .into_iter()
        .map(|h| (h.chunk_id, h.file, h.start, h.end))
        .collect()
}

fn cited_json(sections: &Value) -> Vec<(u64, String, u32, u32)> {
    sections
        .as_array()
        .expect("sections")
        .iter()
        .map(|x| {
            (
                x["chunk_id"].as_u64().expect("id"),
                x["file"].as_str().expect("file").to_string(),
                x["start_line"].as_u64().expect("start") as u32,
                x["end_line"].as_u64().expect("end") as u32,
            )
        })
        .collect()
}

fn proxy(text: &str) -> usize {
    text.chars().count().div_ceil(4)
}

fn started(extra: &[&str], corpus: &Path) -> Session {
    let mut args: Vec<String> = extra.iter().map(|s| s.to_string()).collect();
    if !args.iter().any(|a| a == "--db") {
        args.push("--in-memory".into());
    }
    let mut s = Session::start(&args, &[]);
    s.ok("rag_index", json!({"path": corpus.to_string_lossy()}));
    s
}

fn corpus() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("corpus");
    write_corpus(dir.path());
    dir
}

#[test]
fn the_tool_list_carries_soul_as_a_read_only_tool_with_an_object_schema() {
    for flags in [&[][..], &["--read-only"][..]] {
        let mut s = Session::start(
            &flags
                .iter()
                .map(|f| f.to_string())
                .chain(["--in-memory".to_string()])
                .collect::<Vec<_>>(),
            &[],
        );
        let list = s.rpc("tools/list", json!({}));
        let tool = list["result"]["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .find(|t| t["name"] == "soul")
            .expect("soul listed");
        assert_eq!(tool["annotations"]["readOnlyHint"], true);
        let schema = &tool["inputSchema"];
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(schema["required"], json!([]));
        for key in ["budget_tokens", "focus", "format", "sections"] {
            assert!(schema["properties"][key].is_object(), "{key}");
        }
        assert!(tool["description"].as_str().unwrap_or("").len() > 80);
    }
}

#[test]
fn the_default_call_returns_a_guide_under_the_default_budget_with_an_address_per_section() {
    let dir = corpus();
    let mut s = started(&[], dir.path());
    let compact = s.text("soul", json!({}));
    assert!(!compact.is_empty());
    assert!(proxy(&compact) <= 1200, "{}", proxy(&compact));
    let v = s.ok("soul", json!({}));
    let rebuilt: String = v["sections"].as_array().expect("sections").iter().map(render).collect();
    assert_eq!(v["text"], json!(rebuilt));
    assert_eq!(cited(&compact), cited_json(&v["sections"]));
    assert_eq!(v["budget_tokens"], 1200);
    assert!(v["tokens"].as_u64().expect("tokens") <= 1200);
    let sections = v["sections"].as_array().expect("sections");
    assert!(sections.len() >= 5);
    for sec in sections {
        let (file, start, end, id) = (
            sec["file"].as_str().expect("file"),
            sec["start_line"].as_u64().expect("start"),
            sec["end_line"].as_u64().expect("end"),
            sec["chunk_id"].as_u64().expect("id"),
        );
        assert!(
            cited(&compact).contains(&(id, file.to_string(), start as u32, end as u32)),
            "{file}:{start}"
        );
        let expanded = s.text("rag_expand", json!({"chunk_id": id, "level": 0}));
        let original = fs::read_to_string(dir.path().join(file)).expect("original");
        let lines: Vec<&str> = original.lines().collect();
        let want = lines[(start as usize - 1)..(end as usize)].join("\n");
        assert!(expanded.contains(want.trim_end()), "{file}: {expanded}");
    }
}

#[test]
fn every_explicit_budget_is_respected_and_a_bad_budget_is_an_error() {
    let dir = corpus();
    let mut s = started(&[], dir.path());
    let mut sizes = Vec::new();
    for budget in [1, 20, 60, 150, 400, 900, 3000] {
        let text = s.text("soul", json!({"budget_tokens": budget}));
        assert!(proxy(&text) <= budget, "budget {budget}: {}", proxy(&text));
        sizes.push(text.len());
    }
    assert!(sizes.windows(2).all(|w| w[0] <= w[1]), "{sizes:?}");
    assert!(sizes[6] > sizes[2]);
    for bad in [json!({"budget_tokens": 0}), json!({"budget_tokens": "x"}), json!({"budget_tokens": -3})] {
        s.err("soul", bad);
    }
    s.err("soul", json!({"format": "yaml"}));
    s.err("soul", json!({"focus": 7}));
}

#[test]
fn the_same_arguments_return_the_same_bytes_in_one_session_and_in_a_fresh_process() {
    let dir = corpus();
    let mut a = started(&[], dir.path());
    let first = a.text("soul", json!({"budget_tokens": 500}));
    assert_eq!(first, a.text("soul", json!({"budget_tokens": 500})));
    let mut b = started(&[], dir.path());
    assert_eq!(first, b.text("soul", json!({"budget_tokens": 500})));
}

#[test]
fn focus_restricts_the_guide_to_files_under_the_path_prefix() {
    let dir = corpus();
    let mut s = started(&[], dir.path());
    let all = s.ok("soul", json!({"budget_tokens": 3000}));
    let core = s.ok("soul", json!({"budget_tokens": 3000, "focus": "core/"}));
    let files = |v: &Value| -> Vec<String> {
        v["sections"]
            .as_array()
            .expect("sections")
            .iter()
            .map(|x| x["file"].as_str().expect("file").to_string())
            .collect()
    };
    assert!(files(&all).iter().any(|f| f.starts_with("edge/")));
    let core_files = files(&core);
    assert!(!core_files.is_empty());
    assert!(core_files.iter().all(|f| f.starts_with("core/")), "{core_files:?}");
    let none = s.text("soul", json!({"focus": "nowhere/"}));
    assert_eq!(none, "#0");
}

#[test]
fn sections_returns_only_the_requested_guide_sections_in_guide_order() {
    let dir = corpus();
    let mut s = started(&[], dir.path());
    let full = s.ok("soul", json!({"budget_tokens": 600}));
    let all = full["sections"].as_array().expect("sections");
    assert!(all.len() >= 4);
    let pick = [all[2]["chunk_id"].as_u64().expect("id"), all[0]["chunk_id"].as_u64().expect("id")];
    let compact = s.text("soul", json!({"budget_tokens": 600, "sections": pick}));
    assert_eq!(cited(&compact), cited_json(&json!([all[0], all[2]])));
    let bodies: Vec<String> = decode_pack(&compact).expect("pack").hits.into_iter().filter_map(|h| h.body).collect();
    assert_eq!(bodies, [&all[0], &all[2]].iter().map(|x| x["text"].as_str().expect("text").to_string()).collect::<Vec<_>>());
    let one = s.ok("soul", json!({"budget_tokens": 600, "sections": [pick[0]]}));
    let listed = one["sections"].as_array().expect("sections");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["chunk_id"], json!(pick[0]));
    assert!(listed[0]["text"].is_string());
    s.err("soul", json!({"budget_tokens": 600, "sections": [u64::MAX >> 12]}));
    s.err("soul", json!({"sections": "all"}));
    s.err("soul", json!({"sections": [-1]}));
}

fn sha256(path: &Path) -> String {
    Sha256::digest(fs::read(path).expect("db"))
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[test]
fn a_restarted_server_reads_the_cached_guide_and_a_read_only_server_writes_nothing() {
    let dir = corpus();
    let home = tempfile::tempdir().expect("home");
    let db = home.path().join("g.uriel").to_string_lossy().into_owned();
    let (first_text, first_cached) = {
        let mut s = started(&["--db", &db], dir.path());
        let v = s.ok("soul", json!({"budget_tokens": 500}));
        assert!(s.finish());
        (v["text"].as_str().expect("text").to_string(), v["cached"].clone())
    };
    assert_eq!(first_cached, json!(false));
    {
        let mut s = Session::start(&["--db".to_string(), db.clone()], &[]);
        let v = s.ok("soul", json!({"budget_tokens": 500}));
        assert_eq!(v["cached"], json!(true));
        assert_eq!(v["text"].as_str().expect("text"), first_text);
        assert!(s.finish());
    }
    let fresh = home.path().join("fresh.uriel").to_string_lossy().into_owned();
    {
        let s = started(&["--db", &fresh], dir.path());
        assert!(s.finish());
    }
    let before = sha256(Path::new(&fresh));
    {
        let mut s = Session::start(&["--db".to_string(), fresh.clone(), "--read-only".to_string()], &[]);
        let a = s.ok("soul", json!({"budget_tokens": 500}));
        let b = s.ok("soul", json!({"budget_tokens": 500}));
        assert_eq!(a["cached"], json!(false));
        assert_eq!(b["cached"], json!(false));
        assert_eq!(a["text"].as_str().expect("text"), first_text);
        assert!(s.finish());
    }
    assert_eq!(sha256(Path::new(&fresh)), before);
}
