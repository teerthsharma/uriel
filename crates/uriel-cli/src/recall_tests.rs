use std::io::Cursor;
use std::path::Path;

use serde_json::{json, Value};
use uriel_core::{Graph, MemoryBudget};
use uriel_rag::{Rag, RagConfig};

use crate::capture::{capture_into, scan, MAX_WRITES, SUMMARY_PREFIX};

use super::*;

fn user(text: &str) -> String {
    json!({"type": "user", "message": {"role": "user", "content": text}}).to_string()
}

fn assistant(text: &str) -> String {
    json!({"type": "assistant", "message": {"role": "assistant", "content": [{"type": "text", "text": text}]}}).to_string()
}

fn summary(text: &str) -> String {
    json!({"type": "user", "isCompactSummary": true, "message": {"role": "user", "content": format!("{SUMMARY_PREFIX}. {text}")}}).to_string()
}

fn jsonl(lines: &[String]) -> String {
    let mut s = lines.join("\n");
    s.push('\n');
    s
}

fn mem_rag() -> Rag {
    let graph = Graph::open_in_memory(MemoryBudget::mib(16)).expect("graph");
    Rag::open(graph, RagConfig::default()).expect("rag")
}

fn filler(n: usize) -> Vec<String> {
    (0..n)
        .map(|i| {
            let text = format!("turn {i}: scheduler ledger orbit tensor harbor quartz batch {i} meadow cipher lantern granite");
            if i % 2 == 0 { user(&text) } else { assistant(&text) }
        })
        .collect()
}

fn captured(lines: &[String]) -> Rag {
    let rag = mem_rag();
    let sc = scan(Cursor::new(jsonl(lines).into_bytes()), "s1");
    capture_into(&rag, "s1", &sc, MAX_WRITES).expect("capture");
    rag
}

fn context_of(json: &str) -> String {
    let v: Value = serde_json::from_str(json).expect("json");
    v["hookSpecificOutput"]["additionalContext"].as_str().expect("context").to_string()
}

#[test]
fn tail_query_uses_the_last_user_prompt_and_the_project_name() {
    let s = jsonl(&[user("fix the quokka parser"), assistant("done"), user("now wire the zeppelin cache")]);
    let q = tail_query(Cursor::new(s.into_bytes()), Some(Path::new("/work/myproj")));
    assert!(q.contains("zeppelin cache"));
    assert!(q.contains("myproj"));
}

#[test]
fn tail_query_includes_the_summary_head() {
    let s = jsonl(&[summary("Primary request: migrate the pangolin store"), user("continue")]);
    let q = tail_query(Cursor::new(s.into_bytes()), None);
    assert!(q.contains("pangolin"));
    assert!(q.contains("continue"));
}

#[test]
fn tail_query_is_bounded_and_empty_input_gives_the_cwd_or_nothing() {
    let s = jsonl(&[user(&"w ".repeat(5000)), summary(&"s ".repeat(5000))]);
    let q = tail_query(Cursor::new(s.into_bytes()), Some(Path::new("proj")));
    assert!(q.chars().count() <= 3 * QUERY_PART_CHARS + 8);
    assert_eq!(tail_query(Cursor::new(Vec::new()), None), "");
    assert_eq!(tail_query(Cursor::new(Vec::new()), Some(Path::new("/a/b"))), "b");
    assert_eq!(tail_query(Cursor::new(b"\xff\xfe garbage\n{{{".to_vec()), None), "");
}

#[test]
fn an_empty_database_recalls_the_empty_object() {
    assert_eq!(recall_json(&mem_rag(), "s1", "anything at all", DEFAULT_BUDGET).expect("recall"), "{}");
}

#[test]
fn a_blank_query_recalls_the_empty_object() {
    let rag = captured(&filler(20));
    assert_eq!(recall_json(&rag, "s1", "   ", DEFAULT_BUDGET).expect("recall"), "{}");
}

#[test]
fn recall_has_the_session_start_hook_shape_and_file_citations() {
    let rag = captured(&filler(40));
    let out = recall_json(&rag, "s1", "scheduler ledger orbit", DEFAULT_BUDGET).expect("recall");
    let v: Value = serde_json::from_str(&out).expect("json");
    let keys: Vec<&String> = v.as_object().expect("object").keys().collect();
    assert_eq!(keys, ["hookSpecificOutput"]);
    assert_eq!(v["hookSpecificOutput"]["hookEventName"], "SessionStart");
    let ctx = context_of(&out);
    assert!(ctx.starts_with("uriel recall"));
    assert!(ctx.contains("// ctx/s1/"), "{ctx}");
    assert!(ctx.contains(".md:"));
}

#[test]
fn recall_stays_under_the_token_budget() {
    let rag = captured(&filler(60));
    for budget in [60usize, 200, 1200] {
        let out = recall_json(&rag, "s1", "scheduler ledger orbit", budget).expect("recall");
        if out != "{}" {
            assert!(context_of(&out).chars().count().div_ceil(4) <= budget, "budget {budget}");
        }
    }
}

#[test]
fn recall_output_is_hard_capped_whatever_the_budget() {
    let rag = captured(&filler(400));
    let out = recall_json(&rag, "s1", "scheduler ledger orbit", usize::MAX).expect("recall");
    assert!(out.len() <= MAX_OUTPUT_BYTES + 128, "{}", out.len());
    assert!(context_of(&out).chars().count().div_ceil(4) <= MAX_BUDGET);
}

#[test]
fn the_compaction_summary_comes_before_other_hits() {
    let mut lines = filler(40);
    lines.push(summary("Primary request: tune the kestrel scheduler ledger orbit"));
    let rag = captured(&lines);
    let out = recall_json(&rag, "s1", "scheduler ledger orbit", 4000).expect("recall");
    let ctx = context_of(&out);
    let summary_at = ctx.find("ctx/s1/summary-1.md").expect("summary cited");
    let other_at = ctx.find("// ctx/s1/0.md").or_else(|| ctx.find("// ctx/s1/1.md")).expect("fraction cited");
    assert!(summary_at < other_at, "{ctx}");
}

#[test]
fn recall_ignores_sources_outside_ctx() {
    let rag = captured(&filler(20));
    rag.index_text("src/lib.rs", "fn scheduler_ledger_orbit() { let tensor = 1; }").expect("index");
    let out = recall_json(&rag, "s1", "scheduler ledger orbit tensor", 4000).expect("recall");
    assert!(!context_of(&out).contains("src/lib.rs"));
}
