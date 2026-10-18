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
fn tail_query_is_bounded_and_empty_input_gives_the_cwd_or_nothing() {
    let s = jsonl(&[user(&"w ".repeat(5000)), summary(&"s ".repeat(5000))]);
    let q = tail_query(Cursor::new(s.into_bytes()), Some(Path::new("proj")));
    assert!(q.chars().count() <= QUERY_SUMMARY_CHARS + 8);
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

fn ctx_files(ctx: &str) -> Vec<String> {
    ctx.lines()
        .filter_map(|l| l.strip_prefix("// "))
        .filter_map(|l| l.split(':').next())
        .map(str::to_owned)
        .collect()
}

fn source_of(rag: &Rag, query: &str) -> String {
    rag.search(query, &SearchOptions::default()).expect("search")[0].file.clone()
}

fn with_needle(at: usize, text: &str) -> Vec<String> {
    let mut lines = filler(240);
    lines[at] = assistant(text);
    lines
}

#[test]
fn tail_query_is_the_summary_alone_when_a_summary_exists() {
    let s = jsonl(&[
        user("fix the quokka parser"),
        summary("Primary request: migrate the pangolin store"),
        user("continue"),
    ]);
    let q = tail_query(Cursor::new(s.into_bytes()), Some(Path::new("/work/myproj")));
    assert!(q.contains("pangolin"), "{q}");
    assert!(q.contains("myproj"), "{q}");
    assert!(!q.contains("quokka") && !q.contains("continue"), "{q}");
}

#[test]
fn tail_query_without_a_summary_is_the_last_prompts() {
    let prompts: Vec<String> = ["alpha", "bravo", "charlie", "delta", "echo"]
        .iter()
        .map(|w| user(&format!("please look at the {w} module")))
        .collect();
    let q = tail_query(Cursor::new(jsonl(&prompts).into_bytes()), None);
    assert_eq!(QUERY_PROMPTS, 3);
    for kept in ["charlie", "delta", "echo"] {
        assert!(q.contains(kept), "{q}");
    }
    for dropped in ["alpha", "bravo"] {
        assert!(!q.contains(dropped), "{q}");
    }
    assert!(q.find("charlie") < q.find("echo"));
}

#[test]
fn tail_query_parts_are_bounded_separately() {
    let long = jsonl(&[user(&"w ".repeat(5000)), user(&"v ".repeat(5000)), user(&"u ".repeat(5000))]);
    let q = tail_query(Cursor::new(long.into_bytes()), None);
    assert!(q.chars().count() <= QUERY_PROMPTS * (QUERY_PART_CHARS + 1), "{}", q.chars().count());
    let s = jsonl(&[summary(&"s ".repeat(9000))]);
    let q = tail_query(Cursor::new(s.into_bytes()), Some(Path::new("proj")));
    assert!(q.chars().count() <= QUERY_SUMMARY_CHARS + 1 + "proj".len(), "{}", q.chars().count());
    assert!(q.chars().count() > QUERY_PART_CHARS);
}

#[test]
fn the_best_topical_fraction_follows_the_summary_directly() {
    let mut lines = with_needle(143, "The zeppelin handshake needs the amber gasket before the dusk shift.");
    lines.push(summary("Primary request: tune the kestrel scheduler"));
    let rag = captured(&lines);
    let best = source_of(&rag, "zeppelin handshake amber gasket");
    let out = recall_json(&rag, "s1", "zeppelin handshake amber gasket", 4000).expect("recall");
    let files = ctx_files(&context_of(&out));
    assert_eq!(files[0], "ctx/s1/summary-1.md", "{files:?}");
    assert_eq!(files[1], best, "{files:?}");
}

#[test]
fn only_this_sessions_summaries_are_pinned_newest_first() {
    let mut mine = filler(30);
    mine.push(summary("first plan about the vermilion harbor"));
    mine.extend(filler(10));
    mine.push(summary("second plan about the cobalt harbor"));
    let rag = mem_rag();
    capture_into(&rag, "s1", &scan(Cursor::new(jsonl(&mine).into_bytes()), "s1"), MAX_WRITES).expect("s1");
    let other = jsonl(&[user("hello there friend"), summary("foreign plan about walrus pottery")]);
    capture_into(&rag, "s2", &scan(Cursor::new(other.into_bytes()), "s2"), MAX_WRITES).expect("s2");
    let out = recall_json(&rag, "s1", "scheduler ledger orbit", 4000).expect("recall");
    let files = ctx_files(&context_of(&out));
    assert_eq!(files[0], "ctx/s1/summary-2.md", "{files:?}");
    assert_eq!(files[1], "ctx/s1/summary-1.md", "{files:?}");
}

#[test]
fn recall_text_finds_the_needle_in_the_first_citation() {
    let rag = captured(&with_needle(143, "The zeppelin handshake needs the amber gasket before the dusk shift."));
    let text = recall_text(&rag, "zeppelin handshake amber gasket", 1200, 8).expect("text");
    let first = text.lines().next().expect("line");
    let best = source_of(&rag, "zeppelin handshake amber gasket");
    assert!(first.starts_with(&format!("// {best}:")), "{first}");
    assert!(first.contains(" #"), "{first}");
}

#[test]
fn recall_text_is_bounded_deterministic_and_k_limited() {
    let rag = captured(&filler(120));
    for budget in [10usize, 60, 300, 1200] {
        let a = recall_text(&rag, "scheduler ledger orbit", budget, 40).expect("a");
        assert!(a.chars().count().div_ceil(4) <= budget, "budget {budget}: {}", a.chars().count());
        assert_eq!(a, recall_text(&rag, "scheduler ledger orbit", budget, 40).expect("b"));
    }
    let one = recall_text(&rag, "scheduler ledger orbit", 4000, 1).expect("one");
    assert_eq!(one.lines().filter(|l| l.starts_with("// ")).count(), 1, "{one}");
    let many = recall_text(&rag, "scheduler ledger orbit", 4000, 10).expect("many");
    assert!(many.lines().filter(|l| l.starts_with("// ")).count() > 1);
}

#[test]
fn recall_text_is_plain_empty_for_blank_or_unmatched_queries() {
    let rag = captured(&filler(20));
    assert_eq!(recall_text(&rag, "   ", 1200, 8).expect("blank"), "");
    assert_eq!(recall_text(&mem_rag(), "anything", 1200, 8).expect("empty db"), "");
    let out = recall_text(&rag, "scheduler ledger orbit", usize::MAX, usize::MAX).expect("huge");
    assert!(out.len() <= MAX_OUTPUT_BYTES);
    assert!(!out.starts_with('{'));
}

#[test]
fn tail_query_ignores_tag_wrapped_prompts() {
    let s = jsonl(&[
        user("check the gasket tolerance"),
        user("<task-notification>agent finished noise noise</task-notification>"),
    ]);
    let q = tail_query(Cursor::new(s.into_bytes()), None);
    assert!(q.contains("gasket") && !q.contains("noise"), "{q}");
}

#[test]
fn expand_text_returns_the_whole_stored_chunk_and_rejects_unknown_ids() {
    let rag = captured(&with_needle(143, "The zeppelin handshake needs the amber gasket before the dusk shift."));
    let hit = rag.search("zeppelin handshake amber gasket", &SearchOptions::default()).expect("search").remove(0);
    let text = expand_text(&rag, hit.chunk_id).expect("expand");
    assert_eq!(text, rag.expand(hit.chunk_id).expect("expand"));
    assert!(text.contains("zeppelin handshake needs the amber gasket"));
    assert!(expand_text(&rag, u64::MAX).is_err());
}

#[test]
fn the_hook_header_names_the_command_that_zooms_a_hit() {
    let rag = captured(&filler(60));
    let out = recall_json_at(&rag, "s1", "scheduler ledger orbit", 1200, Some(Path::new("C:/x/ctx.uriel"))).expect("recall");
    let ctx = context_of(&out);
    let header = ctx.lines().next().expect("header");
    assert!(header.contains("uriel recall --db C:/x/ctx.uriel --expand <chunk_id>"), "{header}");
    assert!(ctx.chars().count().div_ceil(4) <= 1200);
}
