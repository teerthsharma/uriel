mod common;

use std::collections::BTreeSet;
use std::process::{Command, Stdio};

use common::{Proj, Session};
use serde_json::{json, Value};
use uriel_mcp::build_server;
use uriel_mcp::config::{parse_args_env, Cli};
use uriel_mcp::prefix::fingerprint_of;

const HOUSE: [&str; 11] = [
    "house_need",
    "house_chart",
    "house_merge",
    "house_fact",
    "house_next",
    "house_report",
    "house_test_run",
    "house_leap",
    "house_status",
    "house_select_tests",
    "house_config",
];

const RESEARCH: [&str; 3] = [
    "research_orbit_audit",
    "research_pooling_audit",
    "research_record",
];

fn start(args: &[&str], envs: &[(&str, &str)]) -> Session {
    let mut all: Vec<String> = vec!["--in-memory".into()];
    all.extend(args.iter().map(|s| s.to_string()));
    Session::start(&all, envs)
}

fn names(args: &[&str], envs: &[(&str, &str)]) -> BTreeSet<String> {
    start(args, envs).names().into_iter().collect()
}

fn only_other_mode(set: &BTreeSet<String>) -> Vec<String> {
    set.iter()
        .filter(|n| n.starts_with("house_") || n.starts_with("research_"))
        .cloned()
        .collect()
}

fn instructions(args: &[&str]) -> String {
    let mut s = start(args, &[]);
    let r = s.rpc("initialize", json!({"protocolVersion": "2025-06-18"}));
    r["result"]["instructions"]
        .as_str()
        .expect("instructions")
        .to_string()
}

#[test]
fn the_default_mode_is_normal_and_lists_nothing_from_house() {
    let got = names(&[], &[]);
    assert!(only_other_mode(&got).is_empty(), "{got:?}");
    for shown in ["rag_search", "rag_index", "seal_recall", "read_neo4j_cypher", "view", "soul"] {
        assert!(got.contains(shown), "{shown} missing");
    }
}

#[test]
fn explicit_normal_is_byte_identical_to_the_default() {
    let mut a = start(&[], &[]);
    let mut b = start(&["--mode", "normal"], &[]);
    let mut c = start(&["--mode=normal"], &[]);
    let la = a.rpc("tools/list", json!({}))["result"].to_string();
    assert_eq!(la, b.rpc("tools/list", json!({}))["result"].to_string());
    assert_eq!(la, c.rpc("tools/list", json!({}))["result"].to_string());
}

#[test]
fn house_mode_adds_exactly_the_house_and_research_tools() {
    let normal = names(&[], &[]);
    let house = names(&["--mode", "house"], &[]);
    let extra: BTreeSet<String> = house.difference(&normal).cloned().collect();
    let want: BTreeSet<String> = HOUSE.iter().chain(&RESEARCH).map(|s| s.to_string()).collect();
    assert_eq!(extra, want);
    assert!(normal.is_subset(&house));
}

#[test]
fn the_environment_selects_the_mode_when_no_flag_is_given() {
    let got = names(&[], &[("URIEL_MODE", "house")]);
    assert!(got.contains("research_orbit_audit") && got.contains("house_need"));
    assert!(only_other_mode(&names(&[], &[("URIEL_MODE", "normal")])).is_empty());
    assert!(only_other_mode(&names(&[], &[("URIEL_MODE", "")])).is_empty());
}

#[test]
fn the_flag_beats_the_environment_in_both_directions() {
    let a = names(&["--mode", "normal"], &[("URIEL_MODE", "house")]);
    assert!(only_other_mode(&a).is_empty());
    let b = names(&["--mode", "house"], &[("URIEL_MODE", "normal")]);
    assert!(b.contains("house_need") && b.contains("research_record"));
}

fn exits_with_usage_error(args: &[&str], envs: &[(&str, &str)], needle: &str) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_uriel-mcp"));
    cmd.args(args)
        .env_remove("URIEL_MODE")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let out = cmd.output().expect("run");
    assert_eq!(out.status.code(), Some(2), "{args:?} {envs:?}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains(needle), "{err}");
}

#[test]
fn a_bad_mode_is_a_usage_error_never_a_silent_default() {
    exits_with_usage_error(&["--in-memory", "--mode", "warp"], &[], "--mode");
    exits_with_usage_error(&["--in-memory", "--mode"], &[], "--mode");
    exits_with_usage_error(&["--in-memory"], &[("URIEL_MODE", "warp")], "URIEL_MODE");
}

fn refusal(s: &mut Session, name: &str) -> Value {
    let text = s.err(name, json!({}));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{name}: not typed JSON ({e}): {text}"))
}

#[test]
fn normal_mode_refuses_every_house_and_research_tool_with_a_typed_error() {
    for flags in [&[][..], &["--read-only"][..], &["--mode", "normal"][..]] {
        let mut s = start(flags, &[]);
        for name in HOUSE.iter().chain(&RESEARCH) {
            let v = refusal(&mut s, name);
            assert_eq!(v["error"], "wrong_mode", "{name}: {v}");
            assert_eq!(v["tool"], *name, "{v}");
            assert_eq!(v["mode"], "normal", "{v}");
            assert_eq!(v["requires"], "house", "{v}");
            assert!(v["hint"].as_str().expect("hint").contains("--mode house"), "{v}");
        }
    }
}

#[test]
fn a_refused_call_leaves_no_trace() {
    let p = Proj::new();
    let mut s = Session::start(
        &["--root".to_string(), p.root(), "--in-memory".to_string()],
        &[],
    );
    s.err(
        "house_need",
        json!({"project": "x", "goal": "y", "acceptance": ["z"]}),
    );
    s.err("research_record", json!({"tool": "research_orbit_audit", "arguments": {}, "goal": "G0"}));
    assert!(!p.events().exists());
    let rows = s.ok("read_neo4j_cypher", json!({"query": "MATCH (n) RETURN count(n) AS c"}));
    assert_eq!(rows, json!([{"c": 0}]));
}

#[test]
fn unknown_tools_are_still_invalid_params_in_both_modes() {
    for flags in [&[][..], &["--mode", "house"][..]] {
        let mut s = start(flags, &[]);
        let r = s.rpc("tools/call", json!({"name": "research_nope", "arguments": {}}));
        assert_eq!(r["error"]["code"], -32602, "{r}");
    }
}

#[test]
fn house_mode_serves_house_and_research_calls() {
    let p = Proj::new();
    let mut s = Session::start(
        &[
            "--root".to_string(),
            p.root(),
            "--in-memory".to_string(),
            "--mode".to_string(),
            "house".to_string(),
        ],
        &[],
    );
    let cfg = s.ok("house_config", json!({}));
    assert!(cfg["models"].is_object(), "{cfg}");
    let r = s.ok(
        "research_orbit_audit",
        json!({"relation": [{"entity": "a", "answer": "x"}], "injective": true}),
    );
    assert_eq!(r["violated"], false, "{r}");
}

#[test]
fn normal_instructions_name_the_mode_and_nothing_from_house() {
    for (flags, read_only) in [(&[][..], false), (&["--read-only"][..], true)] {
        let text = instructions(flags);
        assert!(text.contains("normal mode"), "{text}");
        let lower = text.to_lowercase();
        for banned in ["house", "research_", "caustic", "fellow", "inspector", "differential"] {
            assert!(!lower.contains(banned), "{banned} in {text}");
        }
        assert_eq!(text.contains("read-only"), read_only, "{text}");
        assert!(text.contains("rag_search"), "{text}");
    }
}

#[test]
fn house_instructions_name_the_mode_and_list_every_house_and_research_tool() {
    for flags in [&["--mode", "house"][..], &["--mode", "house", "--read-only"][..]] {
        let text = instructions(flags);
        assert!(text.contains("house mode"), "{text}");
        assert!(!text.contains("normal mode"), "{text}");
        let read_only = flags.contains(&"--read-only");
        for name in HOUSE.iter().chain(&RESEARCH) {
            let writes = matches!(
                *name,
                "house_need"
                    | "house_chart"
                    | "house_merge"
                    | "house_fact"
                    | "house_next"
                    | "house_report"
                    | "house_test_run"
                    | "house_leap"
                    | "research_record"
            );
            if read_only && writes {
                continue;
            }
            assert!(text.contains(name), "{name} missing from {text}");
        }
    }
}

#[test]
fn every_tool_an_instruction_text_names_is_listed_in_that_mode() {
    for flags in [&[][..], &["--read-only"][..], &["--mode", "house"][..], &["--mode", "house", "--read-only"][..]] {
        let text = instructions(flags);
        let listed = names(flags, &[]);
        for word in text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')) {
            let looks_like_tool = word.starts_with("house_")
                || word.starts_with("research_")
                || word.starts_with("rag_")
                || word.starts_with("seal_");
            if looks_like_tool {
                assert!(listed.contains(word), "{word} named but not listed for {flags:?}");
            }
        }
    }
}

#[test]
fn read_only_house_keeps_the_audits_and_drops_every_writer() {
    let got = names(&["--mode", "house", "--read-only"], &[]);
    for kept in ["research_orbit_audit", "research_pooling_audit", "house_status", "house_select_tests", "house_config"] {
        assert!(got.contains(kept), "{kept} missing");
    }
    for dropped in ["research_record", "house_need", "house_chart", "house_merge", "house_fact", "house_next", "house_report", "house_test_run", "house_leap"] {
        assert!(!got.contains(dropped), "{dropped} visible");
    }
}

#[test]
fn read_only_house_refuses_research_record_with_the_read_only_error() {
    let mut s = start(&["--mode", "house", "--read-only"], &[]);
    let msg = s.err(
        "research_record",
        json!({"tool": "research_orbit_audit", "arguments": {}, "goal": "G0"}),
    );
    assert!(msg.contains("read-only"), "{msg}");
}

fn run_config(args: &[&str]) -> uriel_mcp::config::Config {
    match parse_args_env(args.iter().copied(), None).expect("args") {
        Cli::Run(c) => c,
        other => panic!("{other:?}"),
    }
}

fn fingerprint(mode: &str) -> String {
    let dir = tempfile::tempdir().expect("corpus");
    for i in 0..6 {
        std::fs::write(
            dir.path().join(format!("m{i}.rs")),
            format!("/// Module {i} folds frames into digest windows.\npub fn m{i}() -> usize {{ {i} }}\n"),
        )
        .expect("write");
    }
    let cfg = run_config(&["--in-memory", "--mode", mode]);
    let server = build_server(&cfg).expect("server");
    let line = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"rag_index","arguments":{"path": dir.path().to_string_lossy()}}}).to_string();
    server.handle_line(&line).expect("indexed");
    server.scaffold_fingerprint().expect("fingerprint")
}

#[test]
fn the_scaffold_fingerprint_differs_by_mode_and_is_stable_within_one() {
    let normal = fingerprint("normal");
    let house = fingerprint("house");
    assert_ne!(normal, house);
    assert_eq!(normal, fingerprint("normal"));
    assert_eq!(house, fingerprint("house"));
}

#[test]
fn the_fingerprint_function_takes_the_mode_as_its_own_field() {
    use uriel_mcp::prefix::OUTPUT_CONTRACT_VERSION as V;
    assert_ne!(
        fingerprint_of("t", V, "d", "normal"),
        fingerprint_of("t", V, "d", "house")
    );
    assert_eq!(
        fingerprint_of("t", V, "d", "normal"),
        fingerprint_of("t", V, "d", "normal")
    );
    assert_ne!(
        fingerprint_of("ab", V, "c", "house"),
        fingerprint_of("a", V, "bc", "house")
    );
    assert_ne!(
        fingerprint_of("t", V, "d", "house"),
        fingerprint_of("t", V, "dhouse", "")
    );
}

#[test]
fn the_json_preamble_states_the_mode() {
    for (flags, mode) in [(&[][..], "normal"), (&["--mode", "house"][..], "house")] {
        let p = Proj::new();
        let mut args = vec!["--root".to_string(), p.root(), "--in-memory".to_string()];
        args.extend(flags.iter().map(|s| s.to_string()));
        let mut s = Session::start(&args, &[]);
        s.ok("rag_index", json!({}));
        let soul = s.ok("soul", json!({"budget_tokens": 800}));
        let preamble = soul["preamble"].as_str().expect("preamble");
        assert!(preamble.contains(&format!("mode {mode}")), "{preamble}");
    }
}
