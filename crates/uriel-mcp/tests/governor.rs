use serde_json::{json, Value};
use uriel_mcp::build_server;
use uriel_mcp::config::{parse_args_envs, Cli};
use uriel_mcp::governor::Level;
use uriel_mcp::server::Server;

fn server(flags: &[&str]) -> Server {
    let Cli::Run(cfg) = parse_args_envs(flags.iter().copied(), None, None).expect("args") else {
        panic!("not a run config")
    };
    build_server(&cfg).expect("server")
}

fn call(s: &Server, name: &str, args: Value) -> (Value, bool) {
    let line = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":args}}).to_string();
    let r: Value = serde_json::from_str(&s.handle_line(&line).expect("response")).expect("json");
    let text = r["result"]["content"][0]["text"].as_str().expect("text").to_string();
    let is_error = r["result"]["isError"].as_bool().expect("isError");
    (serde_json::from_str(&text).unwrap_or(Value::String(text)), is_error)
}

fn memory(s: &Server) -> Value {
    call(s, "usage", json!({})).0["memory"].clone()
}

fn graph_server(dir: &tempfile::TempDir) -> Server {
    let db = dir.path().join("graph.uriel");
    server(&["--db", db.to_str().unwrap(), "--memory-mib", "16", "--root", dir.path().to_str().unwrap()])
}

fn write_and_read(s: &Server) {
    let (_, e) = call(s, "write_neo4j_cypher", json!({"query": "CREATE (:T {n: 1})"}));
    assert!(!e);
    let (out, e) = call(s, "read_neo4j_cypher", json!({"query": "MATCH (n:T) RETURN n.n AS n"}));
    assert!(!e, "{out}");
}

#[test]
fn red_releases_the_database_and_the_next_call_reopens_it() {
    let dir = tempfile::tempdir().unwrap();
    let s = graph_server(&dir);
    write_and_read(&s);
    let limit = s.governor().limit_bytes();
    s.governor().force_rss(Some(limit * 2));
    assert_eq!(s.governor_tick(), Level::Red);
    assert_eq!(s.governor().shrinks(), 1);
    assert!(!s.release(), "the governor already released the handle");
    s.governor().force_rss(Some(limit / 10));
    let (out, e) = call(&s, "read_neo4j_cypher", json!({"query": "MATCH (n:T) RETURN n.n AS n"}));
    assert!(!e, "{out}");
    assert!(out.to_string().contains('1'), "{out}");
    assert_eq!(s.governor().reopens(), 1);
    assert_eq!(s.governor_tick(), Level::Green);
    let m = memory(&s);
    assert_eq!(m["level"], "green");
    assert_eq!(m["shrinks"], 1);
    assert_eq!(m["reopens"], 1);
    assert_eq!(m["budget_mib"], 16.0);
}

#[test]
fn amber_sheds_caches_but_keeps_the_database_open() {
    let dir = tempfile::tempdir().unwrap();
    let s = graph_server(&dir);
    write_and_read(&s);
    let limit = s.governor().limit_bytes();
    s.governor().force_rss(Some(limit * 85 / 100));
    assert_eq!(s.governor_tick(), Level::Amber);
    assert_eq!(s.governor().shrinks(), 1);
    assert!(s.release(), "amber must not close the database");
}

#[test]
fn a_reading_between_seventy_and_eighty_percent_does_not_flap_amber_back_to_green() {
    let dir = tempfile::tempdir().unwrap();
    let s = graph_server(&dir);
    let limit = s.governor().limit_bytes();
    s.governor().force_rss(Some(limit * 85 / 100));
    assert_eq!(s.governor_tick(), Level::Amber);
    s.governor().force_rss(Some(limit * 75 / 100));
    assert_eq!(s.governor_tick(), Level::Amber);
    s.governor().force_rss(Some(limit * 60 / 100));
    assert_eq!(s.governor_tick(), Level::Green);
}

#[test]
fn shedding_is_rate_limited_while_the_level_holds() {
    let dir = tempfile::tempdir().unwrap();
    let s = graph_server(&dir);
    let limit = s.governor().limit_bytes();
    s.governor().force_rss(Some(limit * 85 / 100));
    for _ in 0..5 {
        s.governor_tick();
    }
    assert_eq!(s.governor().shrinks(), 1);
    s.governor().force_rss(Some(limit * 2));
    s.governor_tick();
    assert_eq!(s.governor().shrinks(), 2);
}

#[test]
fn an_in_memory_graph_is_never_released_even_at_red() {
    let s = server(&["--in-memory", "--memory-mib", "16"]);
    let (_, e) = call(&s, "write_neo4j_cypher", json!({"query": "CREATE (:T {n: 7})"}));
    assert!(!e);
    s.governor().force_rss(Some(s.governor().limit_bytes() * 3));
    assert_eq!(s.governor_tick(), Level::Red);
    let (out, e) = call(&s, "read_neo4j_cypher", json!({"query": "MATCH (n:T) RETURN n.n AS n"}));
    assert!(!e && out.to_string().contains('7'), "{out}");
    assert_eq!(s.governor().reopens(), 0);
}

#[test]
fn usage_reports_the_platform_fields_on_every_os() {
    let s = server(&["--in-memory"]);
    let m = memory(&s);
    for key in ["rss_mib", "budget_mib", "level", "shrinks", "reopens", "free_ram_mib", "os", "rss_source"] {
        assert!(!m[key].is_null(), "{key} missing in {m}");
    }
    assert!(m["rss_mib"].as_f64().unwrap() > 0.0);
    assert!(m["os"].as_str().unwrap().contains(std::env::consts::OS));
}

#[test]
fn an_explicit_budget_is_used_as_given_and_auto_never_exceeds_fifty() {
    let s = server(&["--in-memory", "--memory-mib", "100"]);
    assert_eq!(memory(&s)["budget_mib"], 100.0);
    let auto = server(&["--in-memory"]);
    let mib = memory(&auto)["budget_mib"].as_f64().unwrap();
    assert!((16.0..=50.0).contains(&mib), "{mib}");
    let named = server(&["--in-memory", "--memory-mib", "auto"]);
    assert_eq!(memory(&named)["budget_mib"].as_f64().unwrap(), mib);
}

#[test]
fn home_resolution_prefers_the_native_variable_and_skips_empty_ones() {
    use std::ffi::OsString;
    use uriel_mcp::config::pick_home;
    let some = |s: &str| Some(OsString::from(s));
    assert_eq!(pick_home(true, some(r"C:\Users\a b"), some("/home/x")).unwrap().to_str(), Some(r"C:\Users\a b"));
    assert_eq!(pick_home(false, some(r"C:\Users\a"), some("/home/x y")).unwrap().to_str(), Some("/home/x y"));
    assert_eq!(pick_home(false, some(r"C:\Users\a"), some("")).unwrap().to_str(), Some(r"C:\Users\a"));
    assert_eq!(pick_home(true, some(""), None), None);
}

#[test]
fn the_default_db_path_ends_in_the_dot_seal_folder_on_every_os() {
    let p = uriel_mcp::config::default_db_path();
    assert!(p.ends_with(std::path::Path::new(".seal").join("graph.seal")), "{}", p.display());
}
