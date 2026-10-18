use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::{json, Value};
use uriel_core::{Graph, MemoryBudget};
use uriel_rag::{Rag, RagConfig, SearchOptions};

const NEEDLE: &str = "The vermilion gasket resonates at forty four hundred hertz under load.";
const SUMMARY_PREFIX: &str = "This session is being continued from a previous conversation";

fn user(text: &str) -> String {
    json!({"type": "user", "timestamp": "2026-03-04T05:06:07.000Z", "message": {"role": "user", "content": text}}).to_string()
}

fn assistant(text: &str) -> String {
    json!({"type": "assistant", "message": {"role": "assistant", "content": [{"type": "text", "text": text}]}}).to_string()
}

fn filler(range: std::ops::Range<usize>) -> Vec<String> {
    range
        .map(|i| {
            let text = format!(
                "turn {i}: scheduler ledger orbit tensor harbor quartz batch {i} meadow cipher lantern granite pollen"
            );
            if i % 2 == 0 {
                user(&text)
            } else {
                assistant(&text)
            }
        })
        .collect()
}

fn conversation(n: usize) -> Vec<String> {
    let mut lines = filler(0..n);
    lines[3] = assistant(NEEDLE);
    lines
}

fn write_transcript(path: &Path, lines: &[String]) {
    let mut s = lines.join("\n");
    s.push('\n');
    std::fs::write(path, s).expect("write transcript");
}

fn payload(event: &str, session: &str, transcript: &Path, extra: Value) -> Vec<u8> {
    let mut v = json!({
        "session_id": session,
        "transcript_path": transcript.display().to_string(),
        "hook_event_name": event,
        "cwd": "C:/work/projx",
    });
    if let (Some(a), Some(b)) = (v.as_object_mut(), extra.as_object()) {
        for (k, x) in b {
            a.insert(k.clone(), x.clone());
        }
    }
    v.to_string().into_bytes()
}

fn run(args: &[&str], stdin: &[u8]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_uriel"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    child.stdin.take().expect("stdin").write_all(stdin).expect("feed");
    child.wait_with_output().expect("wait")
}

fn db_arg(dir: &Path) -> (PathBuf, String) {
    let db = dir.join("graph.uriel");
    let arg = db.display().to_string();
    (db, arg)
}

fn open_rag(db: &Path) -> Rag {
    let graph = Graph::open(db, MemoryBudget::mib(50)).expect("open graph");
    Rag::open(graph, RagConfig::default()).expect("rag")
}

fn stderr_lines(o: &Output) -> usize {
    String::from_utf8_lossy(&o.stderr)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .count()
}

fn needle_files(db: &Path) -> Vec<String> {
    let rag = open_rag(db);
    rag.search("vermilion gasket resonates", &SearchOptions::default())
        .expect("search")
        .into_iter()
        .map(|h| h.file)
        .collect()
}

#[test]
fn capture_round_trips_a_turn_three_sentence_through_the_binary() {
    let dir = tempfile::tempdir().expect("tmp");
    let t = dir.path().join("t.jsonl");
    write_transcript(&t, &conversation(30));
    let (db, arg) = db_arg(dir.path());
    let out = run(
        &["capture", "--db", &arg],
        &payload("PreCompact", "sess-1", &t, json!({"trigger": "auto"})),
    );
    assert!(out.status.success());
    assert!(out.stdout.is_empty());
    assert_eq!(stderr_lines(&out), 0, "{}", String::from_utf8_lossy(&out.stderr));
    let files = needle_files(&db);
    assert!(files.first().is_some_and(|f| f.starts_with("ctx/sess-1/")), "{files:?}");
}

#[test]
fn capturing_twice_leaves_the_database_content_unchanged() {
    let dir = tempfile::tempdir().expect("tmp");
    let t = dir.path().join("t.jsonl");
    write_transcript(&t, &conversation(60));
    let (db, arg) = db_arg(dir.path());
    let p = payload("PreCompact", "sess-1", &t, json!({}));
    assert!(run(&["capture", "--db", &arg], &p).status.success());
    let snapshot = |db: &Path| {
        let rag = open_rag(db);
        let hits = rag
            .search("scheduler ledger orbit", &SearchOptions::default())
            .expect("search");
        let s = rag.stats().expect("stats");
        (hits, s.chunks, s.files, s.text_bytes)
    };
    let before = snapshot(&db);
    assert!(run(&["capture", "--db", &arg], &p).status.success());
    assert_eq!(snapshot(&db), before);
}

#[test]
fn a_grown_transcript_adds_the_new_tail_and_keeps_the_old_head() {
    let dir = tempfile::tempdir().expect("tmp");
    let t = dir.path().join("t.jsonl");
    let (db, arg) = db_arg(dir.path());
    let p = payload("PreCompact", "sess-1", &t, json!({}));
    write_transcript(&t, &conversation(30));
    assert!(run(&["capture", "--db", &arg], &p).status.success());
    let mut grown = conversation(30);
    grown.extend(filler(30..50));
    grown.push(assistant("The late zeppelin handshake arrives at dusk."));
    write_transcript(&t, &grown);
    assert!(run(&["capture", "--db", &arg], &p).status.success());
    let rag = open_rag(&db);
    let late = rag
        .search("zeppelin handshake dusk", &SearchOptions::default())
        .expect("search");
    assert!(late.first().is_some_and(|h| h.file.starts_with("ctx/sess-1/")));
    drop(rag);
    assert!(needle_files(&db).first().is_some_and(|f| f.starts_with("ctx/sess-1/")));
}

#[test]
fn transcript_flag_works_when_stdin_is_not_json() {
    let dir = tempfile::tempdir().expect("tmp");
    let t = dir.path().join("t.jsonl");
    write_transcript(&t, &conversation(30));
    let (db, arg) = db_arg(dir.path());
    let ts = t.display().to_string();
    let out = run(
        &["capture", "--db", &arg, "--transcript", &ts, "--session", "manual-1"],
        b"this is not json",
    );
    assert!(out.status.success());
    assert!(needle_files(&db).first().is_some_and(|f| f.starts_with("ctx/manual-1/")));
}

#[test]
fn nothing_to_capture_exits_zero_with_one_diagnostic_and_creates_no_database() {
    let dir = tempfile::tempdir().expect("tmp");
    let (db, arg) = db_arg(dir.path());
    for stdin in [&b""[..], b"garbage", b"{}"] {
        let out = run(&["capture", "--db", &arg], stdin);
        assert!(out.status.success());
        assert!(out.stdout.is_empty());
        assert_eq!(stderr_lines(&out), 1, "{}", String::from_utf8_lossy(&out.stderr));
    }
    assert!(!db.exists());
}

#[test]
fn a_missing_transcript_exits_zero_with_one_diagnostic() {
    let dir = tempfile::tempdir().expect("tmp");
    let (db, arg) = db_arg(dir.path());
    let gone = dir.path().join("gone.jsonl");
    let out = run(&["capture", "--db", &arg], &payload("SessionEnd", "s", &gone, json!({})));
    assert!(out.status.success());
    assert_eq!(stderr_lines(&out), 1);
    assert!(!db.exists());
}

#[test]
fn a_locked_database_is_left_untouched_and_exits_zero() {
    let dir = tempfile::tempdir().expect("tmp");
    let t = dir.path().join("t.jsonl");
    write_transcript(&t, &conversation(30));
    let (db, arg) = db_arg(dir.path());
    let rag = open_rag(&db);
    let before = rag.stats().expect("stats").chunks;
    let out = run(
        &["capture", "--db", &arg],
        &payload("PreCompact", "sess-1", &t, json!({})),
    );
    assert!(out.status.success());
    assert!(out.stdout.is_empty());
    assert_eq!(stderr_lines(&out), 1, "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(rag.stats().expect("stats").chunks, before);
    drop(rag);
    assert!(needle_files(&db).is_empty());
}

#[test]
fn a_locked_database_recalls_the_empty_object_and_exits_zero() {
    let dir = tempfile::tempdir().expect("tmp");
    let t = dir.path().join("t.jsonl");
    write_transcript(&t, &conversation(10));
    let (db, arg) = db_arg(dir.path());
    let _held = open_rag(&db);
    let out = run(
        &["recall-context", "--db", &arg],
        &payload("SessionStart", "sess-1", &t, json!({"source": "compact"})),
    );
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "{}");
    assert_eq!(stderr_lines(&out), 1);
}

#[test]
fn bad_arguments_to_a_hook_command_still_exit_zero() {
    let out = run(&["capture", "--bogus"], b"");
    assert!(out.status.success());
    assert!(stderr_lines(&out) >= 1);
    let out = run(&["recall-context", "--budget", "many"], b"");
    assert!(out.status.success());
    assert!(!run(&["index", "--bogus"], b"").status.success());
}

#[test]
fn help_documents_the_hook_commands() {
    let out = run(&["--help"], b"");
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(text.contains("capture"));
    assert!(text.contains("recall-context"));
    assert!(text.contains("PreCompact"));
    assert!(text.contains("SessionStart"));
}

#[test]
fn recall_on_an_empty_or_missing_database_prints_the_empty_object() {
    let dir = tempfile::tempdir().expect("tmp");
    let t = dir.path().join("t.jsonl");
    write_transcript(&t, &conversation(10));
    let (_, arg) = db_arg(dir.path());
    for stdin in [
        payload("SessionStart", "s", &t, json!({})),
        b"".to_vec(),
        b"junk".to_vec(),
    ] {
        let out = run(&["recall-context", "--db", &arg], &stdin);
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "{}");
    }
}

#[test]
fn recall_after_capture_prints_the_session_start_hook_json() {
    let dir = tempfile::tempdir().expect("tmp");
    let t = dir.path().join("t.jsonl");
    let mut lines = conversation(30);
    lines.push(user("what was the vermilion gasket frequency again"));
    write_transcript(&t, &lines);
    let (_, arg) = db_arg(dir.path());
    let p = payload("SessionStart", "sess-1", &t, json!({"source": "compact"}));
    assert!(run(&["capture", "--db", &arg], &p).status.success());
    let out = run(&["recall-context", "--db", &arg], &p);
    assert!(out.status.success());
    let v: Value = serde_json::from_slice(&out.stdout).expect("stdout is one json document");
    assert_eq!(v["hookSpecificOutput"]["hookEventName"], "SessionStart");
    let ctx = v["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .expect("context");
    assert!(ctx.contains("// ctx/sess-1/"), "{ctx}");
    assert!(ctx.chars().count().div_ceil(4) <= 1200);
}

#[test]
fn recall_honours_a_small_budget() {
    let dir = tempfile::tempdir().expect("tmp");
    let t = dir.path().join("t.jsonl");
    write_transcript(&t, &conversation(60));
    let (_, arg) = db_arg(dir.path());
    let p = payload("SessionStart", "sess-1", &t, json!({}));
    assert!(run(&["capture", "--db", &arg], &p).status.success());
    let out = run(&["recall-context", "--db", &arg, "--budget", "90"], &p);
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if text != "{}" {
        let v: Value = serde_json::from_str(&text).expect("json");
        let ctx = v["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .expect("context");
        assert!(ctx.chars().count().div_ceil(4) <= 90);
    }
}

#[test]
fn recall_with_capture_stores_the_compaction_summary_and_shows_it() {
    let dir = tempfile::tempdir().expect("tmp");
    let t = dir.path().join("t.jsonl");
    let mut lines = filler(0..6);
    lines.push(
        json!({"type": "user", "isCompactSummary": true, "message": {"role": "user",
            "content": format!("{SUMMARY_PREFIX}. Primary request: migrate the pangolin store to the new ledger")}})
        .to_string(),
    );
    lines.push(user("continue with the pangolin migration"));
    write_transcript(&t, &lines);
    let (db, arg) = db_arg(dir.path());
    let p = payload("SessionStart", "sess-1", &t, json!({"source": "compact"}));
    let out = run(&["recall-context", "--db", &arg, "--capture"], &p);
    assert!(out.status.success());
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    let ctx = v["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .expect("context");
    assert!(ctx.contains("ctx/sess-1/summary-1.md"), "{ctx}");
    let rag = open_rag(&db);
    let hits = rag
        .search("pangolin store ledger", &SearchOptions::default())
        .expect("search");
    assert!(hits.iter().any(|h| h.file == "ctx/sess-1/summary-1.md"));
}
