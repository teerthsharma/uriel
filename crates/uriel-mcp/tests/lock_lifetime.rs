mod common;

use std::path::Path;
use std::time::{Duration, Instant};

use common::{chart_and_merge, need, prove, tiny, Proj, Session};
use serde_json::json;
use tempfile::TempDir;
use uriel_core::{Graph, MemoryBudget};

fn hold(db: &Path) -> Graph {
    Graph::open(db, MemoryBudget::mib(16)).expect("the test holds the database")
}

fn server(dir: &TempDir, extra: &[&str]) -> Session {
    let mut args = vec![
        "--db".to_string(),
        dir.path().join("graph.uriel").display().to_string(),
        "--root".to_string(),
        dir.path().display().to_string(),
    ];
    args.extend(extra.iter().map(|s| s.to_string()));
    Session::start(&args, &[])
}

fn wait_released(db: &Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match Graph::open(db, MemoryBudget::mib(16)) {
            Ok(g) => return drop(g),
            Err(e) if Instant::now() > deadline => panic!("the server never released the lock: {e}"),
            Err(_) => std::thread::sleep(Duration::from_millis(100)),
        }
    }
}

fn one(s: &mut Session) -> (String, bool) {
    s.tool("read_neo4j_cypher", json!({"query": "RETURN 1 AS one", "format": "json"}))
}

#[test]
fn a_held_database_still_answers_initialize_and_tools_list() {
    let dir = tempfile::tempdir().expect("dir");
    let held = hold(&dir.path().join("graph.uriel"));
    let mut s = server(&dir, &[]);
    let init = s.rpc(
        "initialize",
        json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "0"}}),
    );
    assert_eq!(init["result"]["serverInfo"]["name"], json!("uriel-mcp"), "{init}");
    assert!(s.names().iter().any(|n| n == "read_neo4j_cypher"));
    drop(held);
    assert!(s.finish());
}

#[test]
fn a_tool_call_on_a_held_database_is_an_error_result_naming_the_lock_and_the_server_lives() {
    let dir = tempfile::tempdir().expect("dir");
    let held = hold(&dir.path().join("graph.uriel"));
    let mut s = server(&dir, &[]);
    let (text, is_error) = one(&mut s);
    assert!(is_error, "{text}");
    assert!(text.contains("locked by another uriel process"), "{text}");
    drop(held);
    let (text, is_error) = one(&mut s);
    assert!(!is_error, "{text}");
    assert_eq!(serde_json::from_str::<serde_json::Value>(&text).expect("rows"), json!([{"one": 1}]));
    assert!(s.finish());
}

#[test]
fn an_idle_server_releases_the_lock_and_the_next_call_reopens() {
    let dir = tempfile::tempdir().expect("dir");
    let db = dir.path().join("graph.uriel");
    let mut s = server(&dir, &["--idle-release-secs", "1"]);
    let (text, is_error) = one(&mut s);
    assert!(!is_error, "{text}");
    assert!(Graph::open(&db, MemoryBudget::mib(16)).is_err());
    wait_released(&db);
    let (text, is_error) = one(&mut s);
    assert!(!is_error, "{text}");
    assert_eq!(serde_json::from_str::<serde_json::Value>(&text).expect("rows"), json!([{"one": 1}]));
    assert!(s.finish());
}

#[test]
fn idle_release_zero_keeps_the_lock() {
    let dir = tempfile::tempdir().expect("dir");
    let db = dir.path().join("graph.uriel");
    let mut s = server(&dir, &["--idle-release-secs", "0"]);
    let (text, is_error) = one(&mut s);
    assert!(!is_error, "{text}");
    std::thread::sleep(Duration::from_millis(1500));
    assert!(Graph::open(&db, MemoryBudget::mib(16)).is_err());
    assert!(s.finish());
}

#[test]
fn house_dispatches_and_commitments_survive_a_release() {
    let p = Proj::new();
    let db = p.db();
    let db = Path::new(&db);
    let mut s = p.session(
        &["--db", &p.db(), "--idle-release-secs", "1"],
        &[("SEAL_MCP_FAKE_PROBE", "16000,16000")],
    );
    need(&mut s);
    chart_and_merge(&mut s, tiny());
    let b = s.ok("house_next", json!({}));
    assert_eq!(b["dispatches"].as_array().expect("dispatches").len(), 2, "{b}");
    wait_released(db);
    let st = s.ok("house_status", json!({}));
    assert_eq!(st["goals"]["active"], 2, "{st}");
    let b = s.ok("house_next", json!({}));
    assert!(b["dispatches"].as_array().expect("dispatches").is_empty(), "{b}");
    assert_eq!(prove(&mut s, "G1")["outcome"], "Accepted");
    wait_released(db);
    let last = prove(&mut s, "G2");
    assert_eq!(last["resolved"][0], "G0", "{last}");
    assert_eq!(s.ok("house_status", json!({}))["complete"], true);
    assert!(s.finish());
}
