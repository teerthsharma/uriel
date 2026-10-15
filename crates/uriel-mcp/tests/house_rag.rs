mod common;

use common::*;
use serde_json::{json, Value};

fn hits(v: &Value) -> &Vec<Value> {
    v["hits"].as_array().expect("hits")
}

fn indexed(p: &Proj) -> Session {
    let mut s = p.session(&[], &[]);
    s.ok("rag_index", json!({}));
    s
}

fn tokens_of(text: &str) -> usize {
    text.chars().count().div_ceil(4)
}

#[test]
fn rag_index_reports_files_and_chunks() {
    let p = Proj::new();
    let mut s = p.session(&[], &[]);
    let r = s.ok("rag_index", json!({}));
    assert!(r["files_indexed"].as_u64().unwrap_or(0) >= 6, "{r}");
    assert!(r["chunks_added"].as_u64().unwrap_or(0) >= 6, "{r}");
    assert_eq!(r["files_unchanged"], 0);
}

#[test]
fn rag_reindex_skips_unchanged_files() {
    let p = Proj::new();
    let mut s = p.session(&[], &[]);
    let first = s.ok("rag_index", json!({}));
    let again = s.ok("rag_index", json!({}));
    assert_eq!(again["files_indexed"], 0, "{again}");
    assert_eq!(again["chunks_added"], 0);
    assert_eq!(again["files_unchanged"], first["files_indexed"]);
}

#[test]
fn rag_index_accepts_a_relative_path() {
    let p = Proj::new();
    let mut s = p.session(&[], &[]);
    let r = s.ok("rag_index", json!({"path": "docs"}));
    assert_eq!(r["files_indexed"], 2, "{r}");
    let found = s.ok("rag_search", json!({"query": "scheduler priority"}));
    assert!(!hits(&found).is_empty(), "{found}");
    assert!(
        hits(&found)
            .iter()
            .all(|h| h["file"].as_str().unwrap_or("").ends_with(".md")),
        "{found}"
    );
}

#[test]
fn rag_index_of_a_missing_directory_is_a_tool_error() {
    let p = Proj::new();
    let mut s = p.session(&[], &[]);
    let e = s.err("rag_index", json!({"path": "no/such/dir"}));
    assert!(e.contains("not a directory"), "{e}");
}

#[test]
fn rag_search_cites_file_and_line() {
    let p = Proj::new();
    let mut s = indexed(&p);
    let r = s.ok(
        "rag_search",
        json!({"query": "exponential backoff retry payment gateway"}),
    );
    let top = &hits(&r)[0];
    assert_eq!(top["file"], "src/backoff.rs", "{r}");
    assert!(top["start_line"].as_u64().unwrap_or(0) >= 1);
    assert!(top["end_line"].as_u64().unwrap_or(0) >= top["start_line"].as_u64().unwrap_or(0));
    assert!(top["chunk_id"].is_u64());
    let text = r["text"].as_str().expect("text");
    let header = format!(
        "// src/backoff.rs:{}-{} #{}",
        top["start_line"], top["end_line"], top["chunk_id"]
    );
    assert!(text.contains(&header), "missing `{header}` in {text}");
}

#[test]
fn rag_search_ranks_the_matching_file_first() {
    let p = Proj::new();
    let mut s = indexed(&p);
    let r = s.ok(
        "rag_search",
        json!({"query": "invoice pdf billing address tax"}),
    );
    assert_eq!(hits(&r)[0]["file"], "src/invoice.rs", "{r}");
}

#[test]
fn rag_search_respects_budget_tokens() {
    let p = Proj::new();
    let mut s = indexed(&p);
    let wide = s.ok(
        "rag_search",
        json!({"query": "scheduler job priority backoff invoice settings", "budget_tokens": 4000}),
    );
    assert!(hits(&wide).len() >= 3, "{wide}");
    for budget in [60usize, 30] {
        let r = s.ok(
            "rag_search",
            json!({"query": "scheduler job priority backoff invoice settings", "budget_tokens": budget}),
        );
        let text = r["text"].as_str().expect("text");
        assert!(
            tokens_of(text) <= budget,
            "{} > {budget}: {text}",
            tokens_of(text)
        );
        assert_eq!(r["budget_tokens"], budget);
        assert!(r["tokens"].as_u64().unwrap_or(u64::MAX) <= budget as u64);
        assert!(hits(&r).len() < hits(&wide).len(), "{r}");
        assert!(r["omitted"].as_u64().unwrap_or(0) > 0, "{r}");
    }
}

#[test]
fn rag_search_k_limits_direct_hits() {
    let p = Proj::new();
    let mut s = indexed(&p);
    let r = s.ok(
        "rag_search",
        json!({"query": "scheduler priority job", "k": 1, "expand_hops": 0}),
    );
    assert_eq!(hits(&r).len(), 1, "{r}");
    assert_eq!(hits(&r)[0]["via"], "direct");
}

#[test]
fn rag_search_rejects_bad_arguments() {
    let p = Proj::new();
    let mut s = indexed(&p);
    for bad in [
        json!({}),
        json!({"query": "   "}),
        json!({"query": "x", "k": 0}),
        json!({"query": "x", "k": "many"}),
        json!({"query": "x", "expand_hops": 3}),
        json!({"query": "x", "budget_tokens": 0}),
    ] {
        s.err("rag_search", bad);
    }
}

#[test]
fn rag_expand_returns_the_full_chunk_text() {
    let p = Proj::new();
    let mut s = indexed(&p);
    let r = s.ok(
        "rag_search",
        json!({"query": "exponential backoff retry payment gateway"}),
    );
    let id = hits(&r)[0]["chunk_id"].clone();
    let full = s.text("rag_expand", json!({"chunk_id": id}));
    assert!(full.contains("pub fn backoff_delay"), "{full}");
    assert!(full.contains("raw.min(ceiling_ms)"), "{full}");
}

#[test]
fn rag_expand_unknown_chunk_is_a_tool_error() {
    let p = Proj::new();
    let mut s = indexed(&p);
    s.err("rag_expand", json!({"chunk_id": 999_999}));
    s.err("rag_expand", json!({}));
    s.err("rag_expand", json!({"chunk_id": "abc"}));
}

#[test]
fn rag_feedback_changes_ranking() {
    let p = Proj::new();
    let mut s = indexed(&p);
    let q = json!({"query": "scheduler next job priority age", "expand_hops": 0});
    let before = s.ok("rag_search", q.clone());
    let docs: Vec<&Value> = hits(&before)
        .iter()
        .filter(|h| h["file"].as_str().unwrap_or("").starts_with("docs/"))
        .collect();
    assert_eq!(docs.len(), 2, "{before}");
    let (first, second) = (docs[0]["chunk_id"].clone(), docs[1]["chunk_id"].clone());
    for _ in 0..3 {
        s.ok("rag_feedback", json!({"used": [second], "unused": [first]}));
    }
    let after = s.ok("rag_search", q);
    let order: Vec<&Value> = hits(&after)
        .iter()
        .filter(|h| h["file"].as_str().unwrap_or("").starts_with("docs/"))
        .map(|h| &h["chunk_id"])
        .collect();
    assert_eq!(
        order,
        [&second, &first],
        "feedback must flip the order: {after}"
    );
}

#[test]
fn rag_feedback_needs_ids() {
    let p = Proj::new();
    let mut s = indexed(&p);
    s.err("rag_feedback", json!({}));
    s.err("rag_feedback", json!({"used": [], "unused": []}));
    s.err("rag_feedback", json!({"used": ["x"]}));
}

const FINDING: &str =
    "Decision: cap payment gateway retries at five because the rate limiter bursts after ten seconds.";

#[test]
fn remember_then_recall_round_trip() {
    let p = Proj::new();
    let mut s = p.session(&[], &[]);
    let id = s.ok(
        "seal_remember",
        json!({"text": FINDING, "tags": ["payments", "retry"]}),
    );
    assert_eq!(id["id"], "finding:1");
    let r = s.ok(
        "seal_recall",
        json!({"query": "why cap payment gateway retries"}),
    );
    let top = &hits(&r)[0];
    assert_eq!(top["remembered"], true, "{r}");
    assert!(top["file"].as_str().unwrap_or("").starts_with("memory/"));
    assert!(r["text"].as_str().unwrap_or("").contains("five"), "{r}");
}

#[test]
fn recall_prefers_remembered_findings_over_indexed_code() {
    let p = Proj::new();
    let mut s = indexed(&p);
    s.ok("seal_remember", json!({"text": FINDING}));
    let r = s.ok(
        "seal_recall",
        json!({"query": "payment gateway retry exponential backoff delay"}),
    );
    let flags: Vec<bool> = hits(&r).iter().map(|h| h["remembered"] == true).collect();
    assert!(flags[0], "remembered first: {r}");
    assert!(flags.iter().any(|f| !f), "general hits follow: {r}");
    let first_general = flags.iter().position(|f| !f).expect("general");
    assert!(flags[first_general..].iter().all(|f| !f), "{flags:?}");
}

#[test]
fn remembered_findings_are_visible_to_rag_search_too() {
    let p = Proj::new();
    let mut s = p.session(&[], &[]);
    s.ok("seal_remember", json!({"text": FINDING}));
    let r = s.ok(
        "rag_search",
        json!({"query": "rate limiter bursts retries"}),
    );
    assert_eq!(hits(&r)[0]["file"], "memory/finding-1.md", "{r}");
}

#[test]
fn remembered_findings_survive_a_restart() {
    let p = Proj::new();
    let mut s = p.session(&["--db", &p.db()], &[]);
    s.ok(
        "seal_remember",
        json!({"text": FINDING, "tags": ["payments"]}),
    );
    s.ok(
        "seal_remember",
        json!({"text": "The invoice renderer must stay deterministic."}),
    );
    assert!(s.finish(), "clean exit");
    let mut s = p.session(&["--db", &p.db()], &[]);
    let r = s.ok(
        "seal_recall",
        json!({"query": "why cap payment gateway retries"}),
    );
    assert_eq!(hits(&r)[0]["remembered"], true, "{r}");
    assert!(r["text"].as_str().unwrap_or("").contains("five"));
    let next = s.ok(
        "seal_remember",
        json!({"text": "A third finding about caching."}),
    );
    assert_eq!(next["id"], "finding:3");
}

#[test]
fn remember_links_its_source_chunks() {
    let p = Proj::new();
    let mut s = indexed(&p);
    let found = s.ok(
        "rag_search",
        json!({"query": "exponential backoff retry payment gateway", "expand_hops": 0}),
    );
    let chunk = hits(&found)[0]["chunk_id"].clone();
    s.ok(
        "seal_remember",
        json!({"text": FINDING, "sources": [chunk]}),
    );
    let rows = s.ok(
        "read_neo4j_cypher",
        json!({"query": "MATCH (d:Decision)-[:CITES]->(c:Chunk) RETURN d.key AS key, c.file AS file"}),
    );
    assert_eq!(rows[0]["key"], "finding:1", "{rows}");
    assert_eq!(rows[0]["file"], "src/backoff.rs");
}

#[test]
fn remember_with_an_unknown_source_stores_nothing() {
    let p = Proj::new();
    let mut s = p.session(&[], &[]);
    let e = s.err(
        "seal_remember",
        json!({"text": FINDING, "sources": [424242]}),
    );
    assert!(e.contains("not a chunk"), "{e}");
    let rows = s.ok(
        "read_neo4j_cypher",
        json!({"query": "MATCH (d:Decision) RETURN count(d) AS n"}),
    );
    assert_eq!(rows[0]["n"], 0, "{rows}");
}

#[test]
fn remember_and_recall_validate_their_arguments() {
    let p = Proj::new();
    let mut s = p.session(&[], &[]);
    s.err("seal_remember", json!({}));
    s.err("seal_remember", json!({"text": "  "}));
    s.err("seal_remember", json!({"text": "x", "tags": "not-a-list"}));
    s.err("seal_recall", json!({}));
    s.err("seal_recall", json!({"query": "x", "k": 0}));
}

#[test]
fn rag_and_house_share_the_one_database() {
    let p = Proj::new();
    let mut s = p.session(&["--db", &p.db()], &[]);
    s.ok("rag_index", json!({}));
    need(&mut s);
    s.ok("seal_remember", json!({"text": FINDING}));
    let mut count = |label: &str| {
        let q = format!("MATCH (n:{label}) RETURN count(n) AS n");
        s.ok("read_neo4j_cypher", json!({"query": q}))[0]["n"]
            .as_u64()
            .expect("count")
    };
    assert!(count("Chunk") > 0);
    assert_eq!(count("Goal"), 1);
    assert_eq!(count("Decision"), 1);
}

#[test]
fn house_full_loop_through_mcp() {
    let p = Proj::new();
    let mut s = p.session(&[], &[("SEAL_MCP_FAKE_PROBE", "16000,16000")]);
    let n = s.ok(
        "house_need",
        json!({"project": "proj", "goal": "build x", "acceptance": ["it works"]}),
    );
    assert_eq!(n["goal"], "G0");
    for (fellow, goals) in [
        ("Foreman", tiny()),
        ("Chase", vec![guard("C1", "G1")]),
        ("Cameron", vec![alternative("A1", "G2", 30_000)]),
    ] {
        let r = s.ok(
            "house_chart",
            json!({"fellow": fellow, "chart": chart(goals)}),
        );
        assert_eq!(r["accepted"], true, "{r}");
        assert_eq!(r["fellow"], fellow);
    }
    let m = s.ok("house_merge", json!({}));
    assert_eq!(m["alt_groups"], 1, "{m}");
    assert_eq!(m["retired"][0][0], "A1", "{m}");

    let b = s.ok("house_next", json!({}));
    let ids: Vec<&str> = b["dispatches"]
        .as_array()
        .expect("dispatches")
        .iter()
        .map(|d| d["goal"].as_str().expect("goal"))
        .collect();
    assert_eq!(ids, ["G1", "G2"], "the guard waits for its leaf: {b}");
    assert_eq!(b["dispatches"][0]["model"], "sonnet");
    assert_eq!(b["dispatches"][0]["role"], "resident");
    assert_eq!(b["complete"], false);

    assert_eq!(prove(&mut s, "G1")["outcome"], "Accepted");
    assert_eq!(prove(&mut s, "G2")["outcome"], "Accepted");
    let b = s.ok("house_next", json!({}));
    assert_eq!(b["dispatches"][0]["goal"], "C1", "{b}");
    let last = prove(&mut s, "C1");
    assert_eq!(last["resolved"][0], "G0", "{last}");

    let st = s.ok("house_status", json!({}));
    assert_eq!(st["complete"], true, "{st}");
    assert_eq!(st["goals"]["reached"], 3);
    assert_eq!(s.ok("house_next", json!({}))["complete"], true);
}

#[test]
fn house_events_reach_the_events_file() {
    let p = Proj::new();
    let mut s = p.session(&[], &[]);
    need(&mut s);
    for (fellow, goals) in [("Foreman", tiny()), ("Chase", vec![guard("C1", "G1")])] {
        s.ok(
            "house_chart",
            json!({"fellow": fellow, "chart": chart(goals)}),
        );
    }
    s.ok("house_merge", json!({}));
    s.ok("house_next", json!({}));
    prove(&mut s, "G1");
    let events = p.event_lines();
    let of = |t: &str| events.iter().filter(|e| e["t"] == t).collect::<Vec<_>>();
    let agents: Vec<&str> = of("chart")
        .iter()
        .map(|e| e["agent"].as_str().expect("agent"))
        .collect();
    assert_eq!(agents, ["Foreman", "Chase"]);
    assert_eq!(of("chart")[0]["goals"][0]["id"], "G1");
    assert!(of("dispatch").iter().all(|e| e["model"] == "sonnet"));
    assert!(of("test")
        .iter()
        .any(|e| e["name"] == "t_G1" && e["status"] == "red"));
    let reached = of("goal")
        .into_iter()
        .find(|e| e["status"] == "reached")
        .expect("reached");
    assert_eq!(
        (reached["id"].clone(), reached["test"].clone()),
        (json!("G1"), json!("t_G1"))
    );
}

#[test]
fn custom_events_path_is_honoured() {
    let p = Proj::new();
    let custom = p.dir.path().join("board").join("ev.jsonl");
    let mut s = p.session(&["--events", &custom.to_string_lossy()], &[]);
    need(&mut s);
    s.ok(
        "house_chart",
        json!({"fellow": "Foreman", "chart": chart(tiny())}),
    );
    assert!(custom.is_file(), "events written to the --events path");
    assert!(!p.events().exists(), "default path untouched");
}

#[test]
fn events_parse_with_gates_py_compute_gates() {
    let skills = std::env::var_os("USERPROFILE")
        .map(|h| std::path::PathBuf::from(h).join(".claude/skills/dispatching-house-mode"));
    let Some(dir) = skills.filter(|d| d.join("gates.py").is_file()) else {
        eprintln!("skipped: gates.py not found");
        return;
    };
    let py = ["python", "py"].into_iter().find(|exe| {
        std::process::Command::new(exe)
            .args(["-c", "import json"])
            .output()
            .is_ok_and(|o| o.status.success())
    });
    let Some(py) = py else {
        eprintln!("skipped: python is not available");
        return;
    };
    let p = Proj::new();
    let mut s = p.session(&[], &[]);
    leap_ready(&mut s);
    s.ok("house_next", json!({}));
    prove(&mut s, "G1.1");
    s.ok(
        "house_leap",
        json!({"goal": "G2", "cause": "missing innovation"}),
    );
    let out = std::process::Command::new(py)
        .args([
            "-c",
            "import sys, json; sys.path.insert(0, sys.argv[1]); import gates; \
             ev = [json.loads(l) for l in open(sys.argv[2], encoding='utf-8') if l.strip()]; \
             print(json.dumps(gates.compute_gates(ev)))",
        ])
        .arg(&dir)
        .arg(p.events())
        .output()
        .expect("run python");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let g: Value = serde_json::from_slice(&out.stdout).expect("gates json");
    assert_eq!(g["active"], true);
    assert_eq!(g["leaps"][0]["id"], "L1", "{g}");
    assert_eq!(
        g["leaps"][0]["bound"], false,
        "an unbound leap is an Open gate row"
    );
    assert_eq!(g["leap_unbound"], json!(["L1"]));
    assert_eq!(
        g["goal_unproven"],
        json!([]),
        "reached goals are RED-first: {g}"
    );
    assert!(g["charts"]["Foreman"]["goals"]
        .as_array()
        .is_some_and(|a| !a.is_empty()));
    let dead: Vec<&Value> = g["theories"]
        .as_array()
        .expect("theories")
        .iter()
        .filter(|t| t["status"] == "dead")
        .collect();
    assert!(dead.iter().any(|t| t["id"] == "G2"), "{g}");
}

#[test]
fn house_report_rejects_reached_without_a_red_first_test() {
    let p = Proj::new();
    let mut s = p.session(&[], &[]);
    need(&mut s);
    chart_and_merge(&mut s, tiny());
    s.ok("house_next", json!({}));
    s.ok("house_test_run", json!({"test": "t_G1", "status": "green"}));
    let e = s.err(
        "house_report",
        json!({"goal": "G1", "status": "reached", "evidence": {"test": "t_G1", "status": "green"}}),
    );
    let v: Value = serde_json::from_str(&e).expect("outcome json");
    assert_eq!(v["outcome"], "Unproven", "{e}");
    assert!(v["reason"].as_str().unwrap_or("").contains("RED"), "{e}");
    let st = s.ok("house_status", json!({}));
    let g1 = st["goal_list"]
        .as_array()
        .expect("goals")
        .iter()
        .find(|g| g["id"] == "G1")
        .expect("G1")
        .clone();
    assert_eq!(g1["status"], "active", "the goal stays active");
}

#[test]
fn house_report_reached_needs_evidence() {
    let p = Proj::new();
    let mut s = p.session(&[], &[]);
    need(&mut s);
    chart_and_merge(&mut s, tiny());
    s.ok("house_next", json!({}));
    let e = s.err("house_report", json!({"goal": "G1", "status": "reached"}));
    assert!(e.contains("evidence"), "{e}");
}

#[test]
fn house_report_rejects_a_goal_that_was_never_dispatched() {
    let p = Proj::new();
    let mut s = p.session(&[], &[]);
    need(&mut s);
    chart_and_merge(&mut s, tiny());
    prove_runs_only(&mut s, "G1");
    let e = s.err(
        "house_report",
        json!({"goal": "G1", "status": "reached", "evidence": {"test": "t_G1", "status": "green"}}),
    );
    assert!(e.contains("never dispatched"), "{e}");
}

fn prove_runs_only(s: &mut Session, goal: &str) {
    let t = test_name(goal);
    s.ok("house_test_run", json!({"test": t, "status": "red"}));
    s.ok("house_test_run", json!({"test": t, "status": "green"}));
}

#[test]
fn house_report_blocked_is_recorded() {
    let p = Proj::new();
    let mut s = p.session(&[], &[]);
    need(&mut s);
    chart_and_merge(&mut s, tiny());
    s.ok("house_next", json!({}));
    let r = s.ok("house_report", json!({"goal": "G1", "status": "blocked"}));
    assert_eq!(r["outcome"], "Recorded");
    assert_eq!(r["status"], "blocked");
}

#[test]
fn house_report_unknown_goal_and_status_are_tool_errors() {
    let p = Proj::new();
    let mut s = p.session(&[], &[]);
    need(&mut s);
    chart_and_merge(&mut s, tiny());
    s.err("house_report", json!({"goal": "G77", "status": "blocked"}));
    s.err(
        "house_report",
        json!({"goal": "G1", "status": "victorious"}),
    );
    s.err("house_report", json!({"status": "blocked"}));
}

#[test]
fn house_chart_violations_come_back_as_a_tool_error() {
    let p = Proj::new();
    let mut s = p.session(&[], &[]);
    need(&mut s);
    let padded = vec![branch("G1", "G0"), leaf("G1.1", "G1"), leaf("G2", "G0")];
    let e = s.err(
        "house_chart",
        json!({"fellow": "Foreman", "chart": chart(padded)}),
    );
    let v: Value = serde_json::from_str(&e).expect("violations json");
    assert_eq!(v["rejected"], true);
    let vs = v["violations"].as_array().expect("violations");
    assert!(
        vs.iter()
            .any(|x| x["rule"] == 3 && x["fellow"] == "Foreman"),
        "{e}"
    );
    assert!(
        p.event_lines().iter().all(|e| e["t"] != "chart"),
        "no chart event for a rejected chart"
    );
}

#[test]
fn house_chart_rejects_bad_input() {
    let p = Proj::new();
    let mut s = p.session(&[], &[]);
    s.err(
        "house_chart",
        json!({"fellow": "Foreman", "chart": chart(tiny())}),
    );
    need(&mut s);
    s.err(
        "house_chart",
        json!({"fellow": "House", "chart": chart(tiny())}),
    );
    s.err(
        "house_chart",
        json!({"fellow": "Foreman", "chart": "not json"}),
    );
    s.err(
        "house_chart",
        json!({"fellow": "Foreman", "chart": {"need": "G0"}}),
    );
    s.err("house_chart", json!({"fellow": "Foreman"}));
}

#[test]
fn house_chart_accepts_a_json_string_and_any_case() {
    let p = Proj::new();
    let mut s = p.session(&[], &[]);
    need(&mut s);
    let as_string = chart(tiny()).to_string();
    let r = s.ok(
        "house_chart",
        json!({"fellow": "foreman", "chart": as_string}),
    );
    assert_eq!(r["fellow"], "Foreman");
}

#[test]
fn house_chart_signed_by_another_fellow_is_rejected() {
    let p = Proj::new();
    let mut s = p.session(&[], &[]);
    need(&mut s);
    let mut c = chart(tiny());
    c["fellow"] = json!("Chase");
    let e = s.err("house_chart", json!({"fellow": "Foreman", "chart": c}));
    assert!(e.contains("signed Chase"), "{e}");
}

#[test]
fn house_need_twice_and_next_before_merge_are_errors() {
    let p = Proj::new();
    let mut s = p.session(&[], &[]);
    s.err("house_next", json!({}));
    s.err("house_merge", json!({}));
    s.err("house_need", json!({"project": "p"}));
    need(&mut s);
    s.err("house_next", json!({}));
    let e = s.err(
        "house_need",
        json!({"project": "p", "goal": "g", "acceptance": []}),
    );
    assert!(e.contains("already exists"), "{e}");
}

#[test]
fn house_fact_assigns_an_id_and_defaults_to_wilson() {
    let p = Proj::new();
    let mut s = p.session(&[], &[]);
    need(&mut s);
    let r = s.ok(
        "house_fact",
        json!({"fact": {"source": "src/a.rs:1", "text": "a exists"}}),
    );
    assert_eq!(r["id"], "F1");
    let r = s.ok(
        "house_fact",
        json!({"fact": {"source": "src/b.rs:1", "text": "b exists"}}),
    );
    assert_eq!(r["id"], "F2");
    let st = s.ok("house_status", json!({}));
    assert_eq!(st["facts"][0]["by"], "Wilson");
    s.err(
        "house_fact",
        json!({"fact": {"id": "F1", "source": "s", "text": "dup"}}),
    );
    s.err("house_fact", json!({"fact": {"text": "no source"}}));
    let fact_events: Vec<Value> = p
        .event_lines()
        .into_iter()
        .filter(|e| e["t"] == "fact")
        .collect();
    assert_eq!(fact_events.len(), 2);
}

#[test]
fn house_test_run_and_select_tests() {
    let p = Proj::new();
    let mut s = p.session(&[], &[]);
    need(&mut s);
    chart_and_merge(&mut s, tiny());
    s.ok(
        "house_test_run",
        json!({"test": "t_G1", "status": "red", "secs": 2.5, "peak_ram_mb": 300, "covered_files": ["src/a.rs"]}),
    );
    let picks = s.ok("house_select_tests", json!({"files": ["src/a.rs"]}));
    assert!(
        picks
            .as_array()
            .expect("picks")
            .iter()
            .any(|t| t["test"] == "t_G1"),
        "{picks}"
    );
    s.err("house_select_tests", json!({"files": []}));
    s.err("house_test_run", json!({"test": "nope", "status": "red"}));
    s.err(
        "house_test_run",
        json!({"test": "t_G1", "status": "purple"}),
    );
    s.err(
        "house_test_run",
        json!({"test": "t_G1", "status": "red", "secs": -1}),
    );
}

fn mixed() -> Vec<Value> {
    vec![leaf("G1", "G0"), leaf("G2", "G0"), nurse("G3", "G0")]
}

#[test]
fn house_next_defers_heavy_leaves_when_ram_is_low() {
    let p = Proj::new();
    let mut s = p.session(&[], &[("SEAL_MCP_FAKE_PROBE", "1000,16000")]);
    need(&mut s);
    chart_and_merge(&mut s, mixed());
    let b = s.ok("house_next", json!({}));
    let ids: Vec<&str> = b["dispatches"]
        .as_array()
        .expect("d")
        .iter()
        .map(|d| d["goal"].as_str().expect("g"))
        .collect();
    assert_eq!(ids, ["G3"], "only the light leaf runs: {b}");
    assert_eq!(b["dispatches"][0]["model"], "haiku");
    assert_eq!(b["governor"]["heavy_slots"], 0);
    assert_eq!(b["governor"]["free_mb"], 1000);
    let deferred: Vec<(&str, &str)> = b["governor"]["deferrals"]
        .as_array()
        .expect("deferrals")
        .iter()
        .map(|d| {
            (
                d["goal"].as_str().expect("g"),
                d["reason"].as_str().expect("r"),
            )
        })
        .collect();
    assert!(
        deferred.contains(&("G1", "ram")) && deferred.contains(&("G2", "ram")),
        "{deferred:?}"
    );
}

#[test]
fn house_next_runs_heavy_leaves_when_ram_is_plentiful() {
    let p = Proj::new();
    let mut s = p.session(&[], &[("SEAL_MCP_FAKE_PROBE", "16000,16000")]);
    need(&mut s);
    chart_and_merge(&mut s, mixed());
    let b = s.ok("house_next", json!({}));
    assert_eq!(b["dispatches"].as_array().expect("d").len(), 3, "{b}");
    assert_eq!(b["governor"]["reserve_mb"], 2400);
}

#[test]
fn house_dispatch_models_follow_the_config_file() {
    let p = Proj::new();
    let cfg = p.dir.path().join("cfg.json");
    std::fs::write(
        &cfg,
        r#"{"models":{"residents":"sonnet-9","nurses":"haiku-9"}}"#,
    )
    .expect("cfg");
    let mut s = p.session(&["--config", &cfg.to_string_lossy()], &[]);
    need(&mut s);
    chart_and_merge(&mut s, mixed());
    let b = s.ok("house_next", json!({}));
    let models: Vec<&str> = b["dispatches"]
        .as_array()
        .expect("d")
        .iter()
        .map(|d| d["model"].as_str().expect("m"))
        .collect();
    assert_eq!(models, ["sonnet-9", "sonnet-9", "haiku-9"]);
}

#[test]
fn house_leap_returns_the_dispatch_with_a_fallback() {
    let p = Proj::new();
    let mut s = p.session(&[], &[]);
    leap_ready(&mut s);
    let l = s.ok(
        "house_leap",
        json!({"goal": "G2", "cause": "missing innovation"}),
    );
    assert_eq!(l["goal"], "G2");
    assert_eq!(l["model"], "opus");
    assert_eq!(l["fallback_model"], "opus");
    assert_eq!(l["effort"], "max");
    assert_eq!(l["timeout_secs"], 300);
    assert_eq!(l["nurses"], 0);
    let input = l["input"].as_str().expect("input");
    assert!(
        input.contains("missing innovation") && input.contains("G2 cannot hold"),
        "{input}"
    );
}

#[test]
fn house_leap_model_comes_from_the_config_file() {
    let p = Proj::new();
    let cfg = p.dir.path().join("cfg.json");
    std::fs::write(
        &cfg,
        r#"{"models":{"dr_house":"fable","dr_house_fallback":"opus-4"}}"#,
    )
    .expect("cfg");
    let mut s = p.session(&["--config", &cfg.to_string_lossy()], &[]);
    leap_ready(&mut s);
    let l = s.ok("house_leap", json!({"goal": "G2", "cause": "c"}));
    assert_eq!(
        (l["model"].clone(), l["fallback_model"].clone()),
        (json!("fable"), json!("opus-4"))
    );
    let ev = p
        .event_lines()
        .into_iter()
        .find(|e| e["t"] == "leap")
        .expect("leap event");
    assert_eq!(ev["model"], "fable");
}

#[test]
fn house_leap_model_env_overrides_the_config_file() {
    let p = Proj::new();
    let cfg = p.dir.path().join("cfg.json");
    std::fs::write(
        &cfg,
        r#"{"models":{"dr_house":"fable","dr_house_fallback":"opus-4"}}"#,
    )
    .expect("cfg");
    let mut s = p.session(
        &["--config", &cfg.to_string_lossy()],
        &[("SEAL_DR_HOUSE_MODEL", "fable-5.5")],
    );
    leap_ready(&mut s);
    let l = s.ok("house_leap", json!({"goal": "G2", "cause": "c"}));
    assert_eq!(l["model"], "fable-5.5");
    assert_eq!(
        l["fallback_model"], "opus-4",
        "the fallback is not touched by the env var"
    );
}

#[test]
fn house_leap_model_env_applies_without_a_config_file() {
    let p = Proj::new();
    let mut s = p.session(&[], &[("SEAL_DR_HOUSE_MODEL", "fable")]);
    leap_ready(&mut s);
    let l = s.ok("house_leap", json!({"goal": "G2", "cause": "c"}));
    assert_eq!(l["model"], "fable");
    assert_eq!(l["fallback_model"], "opus");
}

#[test]
fn a_second_house_leap_for_the_same_goal_errors() {
    let p = Proj::new();
    let mut s = p.session(&[], &[]);
    leap_ready(&mut s);
    s.ok("house_leap", json!({"goal": "G2", "cause": "c"}));
    let e = s.err("house_leap", json!({"goal": "G2", "cause": "c"}));
    assert!(e.contains("already used its leap"), "{e}");
    assert_eq!(
        p.event_lines().iter().filter(|e| e["t"] == "leap").count(),
        1
    );
}

#[test]
fn house_leap_for_a_live_goal_is_not_a_dispatch() {
    let p = Proj::new();
    let mut s = p.session(&[], &[]);
    leap_ready(&mut s);
    let r = s.ok("house_leap", json!({"goal": "G1", "cause": "c"}));
    assert_eq!(r["leap"], false, "{r}");
    assert!(r.get("model").is_none());
    s.err("house_leap", json!({"goal": "G99", "cause": "c"}));
    s.err("house_leap", json!({"goal": "G1"}));
}

#[test]
fn house_config_reports_models_and_governor_settings() {
    let p = Proj::new();
    let cfg = p.dir.path().join("cfg.json");
    std::fs::write(&cfg, r#"{"models":{"dr_house":"fable"},"heavy_cap":3}"#).expect("cfg");
    let mut s = p.session(
        &["--config", &cfg.to_string_lossy()],
        &[("SEAL_DR_HOUSE_MODEL", "fable-5.5")],
    );
    let c = s.ok("house_config", json!({}));
    assert_eq!(c["models"]["dr_house"], "fable-5.5");
    assert_eq!(c["models"]["residents"], "sonnet");
    assert_eq!(c["models"]["nurses"], "haiku");
    assert_eq!(c["governor"]["heavy_cap"], 3);
    assert_eq!(c["governor"]["reserve_pct"], 15);
    assert_eq!(c["governor"]["max_agents"], 20);
    assert!(c["events"]
        .as_str()
        .unwrap_or("")
        .ends_with("house-events.jsonl"));
    assert!(c["config_file"]
        .as_str()
        .unwrap_or("")
        .ends_with("cfg.json"));
    assert_eq!(c["dr_house_env"], "SEAL_DR_HOUSE_MODEL");
}

#[test]
fn default_config_file_is_read_from_the_home_directory() {
    let p = Proj::new();
    let home = tempfile::tempdir().expect("home");
    std::fs::create_dir_all(home.path().join(".seal")).expect("mkdir");
    std::fs::write(
        home.path().join(".seal/config.json"),
        r#"{"models":{"dr_house":"fable"}}"#,
    )
    .expect("cfg");
    let mut s = p.session(
        &[],
        &[
            ("USERPROFILE", &home.path().to_string_lossy()),
            ("HOME", &home.path().to_string_lossy()),
        ],
    );
    let c = s.ok("house_config", json!({}));
    assert_eq!(c["models"]["dr_house"], "fable");
    assert!(c["config_file"].is_string());
}

#[test]
fn house_config_without_any_file_uses_the_defaults() {
    let p = Proj::new();
    let mut s = p.session(&[], &[]);
    let c = s.ok("house_config", json!({}));
    assert_eq!(c["models"]["dr_house"], "opus");
    assert_eq!(c["models"]["dr_house_fallback"], "opus");
    assert_eq!(c["config_file"], Value::Null);
}

#[test]
fn an_explicit_config_that_is_missing_or_malformed_stops_the_server() {
    let p = Proj::new();
    let missing = p
        .dir
        .path()
        .join("absent.json")
        .to_string_lossy()
        .into_owned();
    let bad = p.dir.path().join("bad.json");
    std::fs::write(&bad, "{ nope").expect("bad");
    for cfg in [missing, bad.to_string_lossy().into_owned()] {
        let args = vec![
            "--root".to_string(),
            p.root(),
            "--in-memory".to_string(),
            "--config".to_string(),
            cfg,
        ];
        let s = Session::start(&args, &[]);
        assert!(!s.finish(), "server must exit with a failure code");
    }
}

#[test]
fn a_root_that_is_not_a_directory_stops_the_server() {
    let args = vec![
        "--root".to_string(),
        "Z:/definitely/not/here".to_string(),
        "--in-memory".to_string(),
    ];
    let s = Session::start(&args, &[]);
    assert!(!s.finish());
}

#[test]
fn house_state_survives_a_restart() {
    let p = Proj::new();
    let mut s = p.session(&["--db", &p.db()], &[]);
    need(&mut s);
    chart_and_merge(&mut s, tiny());
    s.ok("house_next", json!({}));
    prove(&mut s, "G1");
    assert!(s.finish());
    let mut s = p.session(&["--db", &p.db()], &[]);
    let st = s.ok("house_status", json!({}));
    assert_eq!(st["merged"], true, "{st}");
    assert_eq!(st["goals"]["reached"], 1);
    assert_eq!(st["goals"]["active"], 1);
    assert_eq!(prove(&mut s, "G2")["resolved"][0], "G0");
}

#[test]
fn a_leap_is_used_once_across_restarts() {
    let p = Proj::new();
    let mut s = p.session(&["--db", &p.db()], &[]);
    leap_ready(&mut s);
    s.ok("house_leap", json!({"goal": "G2", "cause": "c"}));
    assert!(s.finish());
    let mut s = p.session(&["--db", &p.db()], &[]);
    s.err("house_leap", json!({"goal": "G2", "cause": "c"}));
}

const HIDDEN_WHEN_READ_ONLY: &[&str] = &[
    "rag_index",
    "rag_feedback",
    "seal_remember",
    "house_need",
    "house_chart",
    "house_merge",
    "house_fact",
    "house_next",
    "house_report",
    "house_test_run",
    "house_leap",
];

const SHOWN_WHEN_READ_ONLY: &[&str] = &[
    "rag_search",
    "rag_expand",
    "seal_recall",
    "house_status",
    "house_select_tests",
    "house_config",
];

#[test]
fn read_only_hides_the_mutating_mode_tools() {
    let p = Proj::new();
    let mut s = p.session(&["--read-only"], &[]);
    let names = s.names();
    for hidden in HIDDEN_WHEN_READ_ONLY {
        assert!(!names.contains(&hidden.to_string()), "{hidden} visible");
    }
    for shown in SHOWN_WHEN_READ_ONLY {
        assert!(names.contains(&shown.to_string()), "{shown} missing");
    }
}

#[test]
fn read_only_denies_mutating_mode_calls_with_a_tool_error() {
    let p = Proj::new();
    let mut s = p.session(&["--read-only"], &[]);
    for (tool, args) in [
        (
            "house_need",
            json!({"project": "p", "goal": "g", "acceptance": []}),
        ),
        ("rag_index", json!({})),
        ("seal_remember", json!({"text": "x"})),
        ("house_leap", json!({"goal": "G1", "cause": "c"})),
    ] {
        let e = s.err(tool, args);
        assert!(e.contains("--read-only"), "{tool}: {e}");
    }
    assert!(!p.events().exists(), "nothing was written to the board");
}

#[test]
fn read_only_still_serves_the_read_tools() {
    let p = Proj::new();
    let mut s = p.session(&["--read-only"], &[]);
    s.ok("house_config", json!({}));
    s.ok("rag_search", json!({"query": "anything"}));
    s.ok("seal_recall", json!({"query": "anything"}));
    s.err("rag_expand", json!({"chunk_id": 1}));
}

#[test]
fn every_mode_tool_has_a_proper_input_schema() {
    let p = Proj::new();
    let mut s = p.session(&[], &[]);
    let r = s.rpc("tools/list", json!({}));
    let tools = r["result"]["tools"].as_array().expect("tools");
    let want: &[(&str, &[&str])] = &[
        ("rag_index", &[]),
        ("rag_search", &["query"]),
        ("rag_expand", &["chunk_id"]),
        ("rag_feedback", &[]),
        ("seal_remember", &["text"]),
        ("seal_recall", &["query"]),
        ("house_need", &["project", "goal", "acceptance"]),
        ("house_chart", &["fellow", "chart"]),
        ("house_merge", &[]),
        ("house_fact", &["fact"]),
        ("house_next", &[]),
        ("house_report", &["goal", "status"]),
        ("house_test_run", &["test", "status"]),
        ("house_leap", &["goal", "cause"]),
        ("house_status", &[]),
        ("house_select_tests", &["files"]),
        ("house_config", &[]),
    ];
    for (name, required) in want {
        let t = tools
            .iter()
            .find(|t| t["name"] == *name)
            .unwrap_or_else(|| panic!("{name} missing"));
        let schema = &t["inputSchema"];
        assert_eq!(schema["type"], "object", "{name}");
        assert!(schema["properties"].is_object(), "{name}");
        assert!(t["description"].as_str().unwrap_or("").len() > 20, "{name}");
        let got: Vec<&str> = schema["required"]
            .as_array()
            .expect("required")
            .iter()
            .map(|v| v.as_str().expect("s"))
            .collect();
        assert_eq!(&got, required, "{name}");
        for r in *required {
            assert!(schema["properties"][r].is_object(), "{name}.{r}");
        }
        let mutating = HIDDEN_WHEN_READ_ONLY.contains(name);
        assert_eq!(t["annotations"]["readOnlyHint"], !mutating, "{name}");
    }
}

#[test]
fn bad_argument_shapes_are_tool_errors_not_rpc_errors() {
    let p = Proj::new();
    let mut s = p.session(&[], &[]);
    for name in ["rag_search", "house_leap", "house_chart", "seal_remember"] {
        let r = s.rpc("tools/call", json!({"name": name, "arguments": [1, 2]}));
        assert_eq!(r["result"]["isError"], true, "{name}: {r}");
        assert!(r.get("error").is_none(), "{name}: {r}");
    }
}
