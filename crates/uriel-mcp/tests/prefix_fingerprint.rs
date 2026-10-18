use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde_json::{json, Value};
use uriel_mcp::build_server;
use uriel_mcp::config::{parse_args, Cli};
use uriel_mcp::prefix::{fingerprint_of, OUTPUT_CONTRACT_VERSION};
use uriel_mcp::server::Server;

fn write_corpus(dir: &Path, extra: bool) {
    for i in 0..12usize {
        fs::write(
            dir.join(format!("stage_{i}.rs")),
            format!("/// Stage {i} folds ledger frames into digest windows.\npub fn stage_{i}(input: &[u8]) -> usize {{\n    input.len() + {i}\n}}\n"),
        )
        .expect("write");
    }
    if extra {
        fs::write(
            dir.join("extra.rs"),
            "/// Routes cursor packets through the socket buffer.\npub fn extra() -> usize {\n    7\n}\n",
        )
        .expect("write");
    }
}

fn call(server: &Server, name: &str, args: Value) -> String {
    let line = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":args}})
        .to_string();
    let resp: Value = serde_json::from_str(&server.handle_line(&line).expect("response")).expect("json");
    resp["result"]["content"][0]["text"].as_str().expect("text").to_string()
}

fn fresh(corpus: &Path, flags: &[&str]) -> Server {
    let mut args = vec!["--in-memory"];
    args.extend_from_slice(flags);
    let Cli::Run(cfg) = parse_args(args).expect("args") else {
        panic!("not a run config")
    };
    let server = build_server(&cfg).expect("server");
    call(&server, "rag_index", json!({"path": corpus.to_string_lossy()}));
    server
}

fn corpus(extra: bool) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("corpus");
    write_corpus(dir.path(), extra);
    dir
}

#[test]
fn the_scaffold_fingerprint_is_identical_across_twenty_fresh_servers() {
    let dir = corpus(false);
    let seen: BTreeSet<String> = (0..20)
        .map(|_| fresh(dir.path(), &[]).scaffold_fingerprint().expect("fingerprint"))
        .collect();
    assert_eq!(seen.len(), 1, "{seen:?}");
    let only = seen.into_iter().next().expect("one");
    assert_eq!(only.len(), 64);
    assert!(only.bytes().all(|b| b.is_ascii_hexdigit()));
}

#[test]
fn the_scaffold_fingerprint_is_identical_across_concurrent_threads() {
    let dir = corpus(false);
    let path = dir.path().to_path_buf();
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let path = path.clone();
            std::thread::spawn(move || fresh(&path, &[]).scaffold_fingerprint().expect("fingerprint"))
        })
        .collect();
    let seen: BTreeSet<String> = handles.into_iter().map(|h| h.join().expect("join")).collect();
    assert_eq!(seen.len(), 1, "{seen:?}");
}

#[test]
fn the_scaffold_fingerprint_changes_when_a_tool_schema_changes() {
    let base = fingerprint_of("{\"tools\":[{\"name\":\"view\"}]}", OUTPUT_CONTRACT_VERSION, "d");
    let edited = fingerprint_of("{\"tools\":[{\"name\":\"view2\"}]}", OUTPUT_CONTRACT_VERSION, "d");
    assert_ne!(base, edited);
}

#[test]
fn the_scaffold_fingerprint_changes_when_the_format_version_changes() {
    let base = fingerprint_of("t", OUTPUT_CONTRACT_VERSION, "d");
    assert_ne!(base, fingerprint_of("t", OUTPUT_CONTRACT_VERSION + 1, "d"));
}

#[test]
fn the_scaffold_fingerprint_changes_when_the_soul_header_changes() {
    assert_ne!(
        fingerprint_of("t", OUTPUT_CONTRACT_VERSION, "d"),
        fingerprint_of("t", OUTPUT_CONTRACT_VERSION, "e")
    );
}

#[test]
fn the_scaffold_fingerprint_does_not_split_fields_ambiguously() {
    assert_ne!(
        fingerprint_of("ab", OUTPUT_CONTRACT_VERSION, "c"),
        fingerprint_of("a", OUTPUT_CONTRACT_VERSION, "bc")
    );
}

#[test]
fn the_scaffold_fingerprint_changes_when_the_index_changes() {
    let (a, b) = (corpus(false), corpus(true));
    assert_ne!(
        fresh(a.path(), &[]).scaffold_fingerprint().expect("a"),
        fresh(b.path(), &[]).scaffold_fingerprint().expect("b")
    );
}

#[test]
fn the_scaffold_fingerprint_does_not_change_when_only_the_query_changes() {
    let dir = corpus(false);
    let server = fresh(dir.path(), &[]);
    let before = server.scaffold_fingerprint().expect("before");
    for question in ["ledger frames digest windows", "socket buffer routes", "nothing matches zzzq"] {
        call(&server, "view", json!({"question": question, "budget_tokens": 400}));
        assert_eq!(server.scaffold_fingerprint().expect("after"), before, "{question}");
    }
}

#[test]
fn the_scaffold_fingerprint_differs_between_read_only_and_writable_tool_sets() {
    let dir = corpus(false);
    let writable = fresh(dir.path(), &[]).scaffold_fingerprint().expect("w");
    let read_only = fresh(dir.path(), &["--read-only"]).scaffold_fingerprint().expect("r");
    assert_ne!(writable, read_only);
}
