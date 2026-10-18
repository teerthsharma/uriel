use std::io::Cursor;

use serde_json::{json, Value};
use uriel_core::{Graph, MemoryBudget};
use uriel_rag::{Rag, RagConfig, SearchOptions};

use super::*;

const CAP: usize = FRACTION_TOKENS * CHARS_PER_TOKEN;
const OVERLAP: usize = OVERLAP_TOKENS * CHARS_PER_TOKEN;

fn line(role: &str, content: Value, ts: Option<&str>) -> String {
    let mut v = json!({"type": role, "message": {"role": role, "content": content}, "sessionId": "s1"});
    if let Some(t) = ts {
        v["timestamp"] = json!(t);
    }
    v.to_string()
}

fn user(text: &str) -> String {
    line("user", json!(text), None)
}

fn assistant(text: &str) -> String {
    line("assistant", json!([{"type": "text", "text": text}]), None)
}

fn jsonl(lines: &[String]) -> String {
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

fn words(i: usize) -> String {
    const W: [&str; 12] = [
        "scheduler", "ledger", "orbit", "tensor", "harbor", "quartz", "meadow", "cipher", "lantern",
        "granite", "pollen", "vector",
    ];
    (0..40)
        .map(|j| W[(i * 7 + j * 5 + j * j) % 12])
        .collect::<Vec<_>>()
        .join(" ")
}

fn conversation(turns: usize) -> Vec<String> {
    (0..turns)
        .map(|i| {
            let body = format!("turn {i}: {}", words(i));
            if i % 2 == 0 {
                user(&body)
            } else {
                assistant(&body)
            }
        })
        .collect()
}

fn scan_str(s: &str) -> Scan {
    scan(Cursor::new(s.as_bytes()), "s1")
}

fn body(f: &Fraction) -> &str {
    f.text.split_once('\n').map_or("", |x| x.1)
}

fn mem_rag() -> Rag {
    let graph = Graph::open_in_memory(MemoryBudget::mib(16)).expect("graph");
    Rag::open(graph, RagConfig::default()).expect("rag")
}

fn files_for(rag: &Rag, query: &str) -> Vec<String> {
    rag.search(query, &SearchOptions::default())
        .expect("search")
        .into_iter()
        .map(|h| h.file)
        .collect()
}

#[test]
fn payload_parses_precompact_fields() {
    let p = parse_payload(
        br#"{"session_id":"abc","transcript_path":"/t/abc.jsonl","cwd":"/w","hook_event_name":"PreCompact","trigger":"auto"}"#,
    )
    .expect("payload");
    assert_eq!(p.session_id.as_deref(), Some("abc"));
    assert_eq!(p.transcript_path, Some(PathBuf::from("/t/abc.jsonl")));
    assert_eq!(p.cwd, Some(PathBuf::from("/w")));
    assert_eq!(p.event.as_deref(), Some("PreCompact"));
    assert_eq!(p.trigger.as_deref(), Some("auto"));
    assert_eq!(p.source, None);
}

#[test]
fn payload_parses_sessionstart_source_and_ignores_unknown_fields() {
    let p = parse_payload(br#"{"hook_event_name":"SessionStart","source":"compact","extra":{"a":[1,2]},"session_id":"z"}"#)
        .expect("payload");
    assert_eq!(p.source.as_deref(), Some("compact"));
    assert_eq!(p.session_id.as_deref(), Some("z"));
}

#[test]
fn payload_rejects_empty_garbage_and_non_objects() {
    for bad in [&b""[..], b"   \n", b"not json", b"[1,2]", b"null", b"\xff\xfe\x00", b"{\"a\":"] {
        assert_eq!(parse_payload(bad), None, "{bad:?}");
    }
}

#[test]
fn payload_wrong_typed_fields_are_dropped_not_fatal() {
    let p = parse_payload(br#"{"session_id":5,"transcript_path":[],"source":null,"trigger":"auto"}"#)
        .expect("payload");
    assert_eq!(p.session_id, None);
    assert_eq!(p.transcript_path, None);
    assert_eq!(p.source, None);
    assert_eq!(p.trigger.as_deref(), Some("auto"));
}

#[test]
fn entry_extracts_user_string_and_assistant_text_blocks() {
    let u = entry_of(user("hello there").as_bytes()).expect("user");
    assert_eq!(u.text, "[user] hello there");
    assert_eq!(u.prompt.as_deref(), Some("hello there"));
    assert!(!u.summary);
    let a = entry_of(assistant("general kenobi").as_bytes()).expect("assistant");
    assert_eq!(a.text, "[assistant] general kenobi");
    assert_eq!(a.prompt, None);
}

#[test]
fn entry_skips_thinking_and_unknown_blocks() {
    let l = line(
        "assistant",
        json!([
            {"type": "thinking", "thinking": "secret chain", "signature": "x"},
            {"type": "redacted_thinking", "data": "zzz"},
            {"type": "image", "source": {"data": "AAAA"}},
            {"type": "text", "text": "visible"}
        ]),
        None,
    );
    let e = entry_of(l.as_bytes()).expect("entry");
    assert_eq!(e.text, "[assistant] visible");
    let only_thinking = line("assistant", json!([{"type": "thinking", "thinking": "hmm"}]), None);
    assert_eq!(entry_of(only_thinking.as_bytes()), None);
}

#[test]
fn entry_renders_tool_use_name_and_capped_input() {
    let short = line(
        "assistant",
        json!([{"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "ls"}}]),
        None,
    );
    assert_eq!(entry_of(short.as_bytes()).expect("entry").text, "[tool_use] Bash {\"command\":\"ls\"}");
    let long = line(
        "assistant",
        json!([{"type": "tool_use", "id": "t1", "name": "Write", "input": {"content": "y".repeat(5000)}}]),
        None,
    );
    let text = entry_of(long.as_bytes()).expect("entry").text;
    assert!(text.starts_with("[tool_use] Write "));
    assert!(text.chars().count() <= "[tool_use] Write ".len() + TOOL_INPUT_CAP_CHARS + 3);
    assert!(text.matches('y').count() <= TOOL_INPUT_CAP_CHARS);
}

#[test]
fn entry_truncates_tool_result_in_string_and_array_forms() {
    let as_string = line(
        "user",
        json!([{"type": "tool_result", "tool_use_id": "t1", "content": "x".repeat(5000)}]),
        None,
    );
    let e = entry_of(as_string.as_bytes()).expect("entry");
    assert!(e.text.starts_with("[tool_result] "));
    assert_eq!(e.text.matches('x').count(), TOOL_RESULT_CAP_CHARS);
    assert_eq!(e.prompt, None);
    let as_array = line(
        "user",
        json!([{"type": "tool_result", "tool_use_id": "t1", "content": [{"type": "text", "text": "w".repeat(5000)}, {"type": "text", "text": "tail"}]}]),
        None,
    );
    let e = entry_of(as_array.as_bytes()).expect("entry");
    assert_eq!(e.text.matches('w').count(), TOOL_RESULT_CAP_CHARS);
}

#[test]
fn entry_truncation_never_splits_a_char() {
    let l = line(
        "user",
        json!([{"type": "tool_result", "tool_use_id": "t", "content": "\u{1F600}".repeat(2000)}]),
        None,
    );
    let e = entry_of(l.as_bytes()).expect("entry");
    assert_eq!(e.text.matches('\u{1F600}').count(), TOOL_RESULT_CAP_CHARS);
    assert!(!e.text.contains('\u{FFFD}'));
}

#[test]
fn entry_ignores_unknown_record_types_and_meta() {
    for l in [
        r#"{"type":"system","subtype":"compact_boundary","content":"x"}"#,
        r#"{"type":"queue-operation","content":"x"}"#,
        r#"{"type":"file-history-snapshot","snapshot":{}}"#,
        r#"{"type":"last-prompt","lastPrompt":"abc"}"#,
        r#"{"type":"user","isMeta":true,"message":{"role":"user","content":"skill body"}}"#,
        r#"{}"#,
    ] {
        assert_eq!(entry_of(l.as_bytes()), None, "{l}");
    }
}

#[test]
fn entry_reads_the_record_timestamp() {
    let l = line("user", json!("hi"), Some("2026-03-04T05:06:07.000Z"));
    assert_eq!(entry_of(l.as_bytes()).expect("entry").ts, "2026-03-04T05:06:07.000Z");
    assert_eq!(entry_of(user("hi").as_bytes()).expect("entry").ts, "-");
}

#[test]
fn entry_flags_compact_summary_by_field_and_by_prefix() {
    let mut v: Value = serde_json::from_str(&user(&format!("{SUMMARY_PREFIX}. Summary: zebra"))).expect("json");
    v["isCompactSummary"] = json!(true);
    let by_field = entry_of(v.to_string().as_bytes()).expect("entry");
    assert!(by_field.summary);
    assert_eq!(by_field.prompt, None);
    let by_prefix = entry_of(user(&format!("{SUMMARY_PREFIX}. Summary: zebra")).as_bytes()).expect("entry");
    assert!(by_prefix.summary);
    assert!(!entry_of(user("an ordinary prompt").as_bytes()).expect("entry").summary);
}

#[test]
fn session_key_keeps_safe_chars_and_replaces_the_rest() {
    assert_eq!(session_key("abc-123_x.y"), "abc-123_x.y");
    assert_eq!(session_key("../../etc/passwd"), ".._.._etc_passwd");
    assert_eq!(session_key(""), "unknown");
    assert_eq!(session_key("a b\u{1F600}c"), "a_b_c");
    assert_eq!(session_key(&"z".repeat(500)).len(), 64);
}

#[test]
fn source_ids_follow_the_ctx_layout() {
    assert_eq!(fraction_source("s1", 7), "ctx/s1/7.md");
    assert_eq!(summary_source("s1", 2), "ctx/s1/summary-2.md");
}

#[test]
fn empty_and_blank_transcripts_yield_nothing() {
    for s in ["", "\n", "\n\n  \n", "\r\n"] {
        let sc = scan_str(s);
        assert!(sc.fractions.is_empty(), "{s:?}");
        assert!(sc.summaries.is_empty());
    }
    let rag = mem_rag();
    let report = capture_into(&rag, "s1", &scan_str(""), MAX_WRITES).expect("capture");
    assert_eq!(report, Report::default());
}

#[test]
fn malformed_and_oddly_shaped_lines_are_tolerated() {
    let lines = vec![
        user("first survivor"),
        "not json at all".to_string(),
        "{\"type\":\"user\",\"message\"".to_string(),
        "[]".to_string(),
        "null".to_string(),
        "123".to_string(),
        "\"just a string\"".to_string(),
        r#"{"type":"user","message":5}"#.to_string(),
        r#"{"type":"assistant","message":{"role":"assistant","content":[1,null,{"type":"text"},{"type":"text","text":5},{"type":"tool_use"}]}}"#.to_string(),
        r#"{"type":"user","message":{"content":{"a":1}}}"#.to_string(),
        assistant("second survivor"),
    ];
    let sc = scan_str(&jsonl(&lines));
    assert_eq!(sc.fractions.len(), 1);
    let text = &sc.fractions[0].text;
    assert!(text.contains("first survivor") && text.contains("second survivor"));
    assert!(sc.bad_lines >= 2);
}

#[test]
fn crlf_line_endings_parse() {
    let s = format!("{}\r\n{}\r\n", user("alpha crlf"), assistant("beta crlf"));
    let sc = scan_str(&s);
    assert_eq!(sc.fractions.len(), 1);
    assert!(sc.fractions[0].text.contains("alpha crlf") && sc.fractions[0].text.contains("beta crlf"));
}

#[test]
fn oversized_line_is_skipped_and_neighbours_survive() {
    let giant = user(&"g".repeat(MAX_LINE_BYTES + 10));
    let s = jsonl(&[user("before the giant"), giant, assistant("after the giant")]);
    let sc = scan_str(&s);
    assert_eq!(sc.oversize_lines, 1);
    let all: String = sc.fractions.iter().map(|f| f.text.as_str()).collect();
    assert!(all.contains("before the giant") && all.contains("after the giant"));
    assert!(!all.contains("gggggggggg"));
}

#[test]
fn oversized_line_at_eof_without_newline_is_skipped() {
    let mut s = jsonl(&[user("only survivor")]);
    s.push_str(&user(&"g".repeat(MAX_LINE_BYTES + 10)));
    let sc = scan_str(&s);
    assert_eq!(sc.oversize_lines, 1);
    assert_eq!(sc.fractions.len(), 1);
}

#[test]
fn adversarial_bytes_never_panic() {
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let mut data = Vec::new();
    for _ in 0..200_000 {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        data.push((state >> 56) as u8);
    }
    data.extend(std::iter::repeat_n(b'[', 100_000));
    data.push(b'\n');
    data.extend(b"{\"type\":\"user\",\"message\":{\"content\":\"ok \xff\xfe\"}}\n");
    data.extend(std::iter::repeat_n(0u8, 5000));
    let sc = scan(Cursor::new(data), "s1");
    assert!(sc.bad_lines > 0);
}

#[test]
fn fractions_are_bounded_contiguous_and_overlap() {
    let sc = scan_str(&jsonl(&conversation(80)));
    assert!(sc.fractions.len() >= 4);
    assert_eq!(sc.fractions[0].first_turn, 0);
    assert_eq!(sc.fractions.last().expect("last").last_turn, 79);
    for (i, f) in sc.fractions.iter().enumerate() {
        assert_eq!(f.index, i);
        assert!(body(f).chars().count() <= CAP, "fraction {i} too wide");
        assert!(f.text.starts_with(&format!("# ctx turns={}-{} ", f.first_turn, f.last_turn)));
        assert!(f.text.lines().next().expect("header").ends_with("session=s1"));
    }
    for pair in sc.fractions.windows(2) {
        assert_eq!(pair[1].first_turn, pair[0].last_turn);
    }
}

fn shared_overlap(prev: &str, next: &str) -> usize {
    (1..=OVERLAP)
        .rev()
        .find(|&k| {
            let head: String = next.chars().take(k).collect();
            head.chars().count() == k && prev.ends_with(&head)
        })
        .unwrap_or(0)
}

#[test]
fn consecutive_fractions_share_a_bounded_overlap() {
    let sc = scan_str(&jsonl(&conversation(80)));
    for pair in sc.fractions.windows(2) {
        let k = shared_overlap(body(&pair[0]), body(&pair[1]));
        assert!((1..=OVERLAP).contains(&k),"overlap {k} between {} and {}", pair[0].index, pair[1].index);
    }
}

#[test]
fn the_overlap_is_whole_lines_when_lines_fit() {
    let short: Vec<String> = (0..200)
        .map(|i| {
            let text = format!("turn {i}: {}", words(i).split(' ').take(10).collect::<Vec<_>>().join(" "));
            if i % 2 == 0 {
                user(&text)
            } else {
                assistant(&text)
            }
        })
        .collect();
    let sc = scan_str(&jsonl(&short));
    assert!(sc.fractions.len() >= 3);
    for f in sc.fractions.iter().skip(1) {
        let first = body(f).lines().next().expect("line");
        assert!(first.starts_with("[user]") || first.starts_with("[assistant]"), "{first:.40}");
    }
}

#[test]
fn cuts_fall_on_turn_boundaries_when_turns_are_small() {
    let sc = scan_str(&jsonl(&conversation(80)));
    for pair in sc.fractions.windows(2) {
        let k = shared_overlap(body(&pair[0]), body(&pair[1]));
        let rest: String = body(&pair[1]).chars().skip(k).collect();
        assert!(rest.starts_with("\n[user]") || rest.starts_with("\n[assistant]"), "{rest:.40}");
    }
}

#[test]
fn a_single_huge_turn_is_split_without_splitting_chars() {
    let huge = "\u{e9}\u{1F600}".repeat(5000);
    let sc = scan_str(&jsonl(&[user("lead in"), assistant(&huge), user("lead out")]));
    assert!(sc.fractions.len() >= 4);
    let mut emoji = 0;
    for f in &sc.fractions {
        assert!(body(f).chars().count() <= CAP);
        assert!(!f.text.contains('\u{FFFD}'));
        emoji += f.text.matches('\u{1F600}').count();
    }
    assert!(emoji >= 5000);
    let all: String = sc.fractions.iter().map(|f| f.text.as_str()).collect();
    assert!(all.contains("lead in") && all.contains("lead out"));
}

#[test]
fn scanning_twice_is_byte_identical() {
    let s = jsonl(&conversation(60));
    assert_eq!(scan_str(&s), scan_str(&s));
}

#[test]
fn growth_keeps_every_closed_fraction_byte_identical() {
    let small = scan_str(&jsonl(&conversation(40)));
    let big = scan_str(&jsonl(&conversation(80)));
    assert!(big.fractions.len() > small.fractions.len());
    let stable = small.fractions.len() - 1;
    assert_eq!(small.fractions[..stable], big.fractions[..stable]);
}

#[test]
fn header_carries_the_record_timestamp_or_a_dash() {
    let ts = line("user", json!("stamped"), Some("2026-03-04T05:06:07.000Z"));
    let sc = scan_str(&jsonl(&[ts, assistant("plain")]));
    let header = sc.fractions[0].text.lines().next().expect("header");
    assert_eq!(header, "# ctx turns=0-1 ts=2026-03-04T05:06:07.000Z session=s1");
    let none = scan_str(&jsonl(&[user("a"), assistant("b")]));
    assert_eq!(none.fractions[0].text.lines().next().expect("header"), "# ctx turns=0-1 ts=- session=s1");
}

#[test]
fn compaction_summary_is_split_out_of_the_fractions() {
    let mut summary: Value = serde_json::from_str(&user(&format!("{SUMMARY_PREFIX}. Summary: zebra crossing"))).expect("json");
    summary["isCompactSummary"] = json!(true);
    let s = jsonl(&[user("hello"), assistant("hi"), summary.to_string(), user("next")]);
    let sc = scan_str(&s);
    assert_eq!(sc.summaries.len(), 1);
    assert_eq!(sc.summaries[0].n, 1);
    assert!(sc.summaries[0].text.starts_with("# ctx-summary n=1 "));
    assert!(sc.summaries[0].text.contains("zebra crossing"));
    let all: String = sc.fractions.iter().map(|f| f.text.as_str()).collect();
    assert!(!all.contains("zebra"));
    assert!(all.contains("hello") && all.contains("next"));
}

#[test]
fn fraction_ring_keeps_the_newest_with_absolute_indexes() {
    let s = jsonl(&conversation(80));
    let all = scan_str(&s);
    let ring = scan_bounded(Cursor::new(s.as_bytes()), "s1", 3, MAX_READ_BYTES);
    assert_eq!(ring.fractions.len(), 3);
    assert!(ring.truncated);
    assert_eq!(ring.fractions, all.fractions[all.fractions.len() - 3..]);
}

#[test]
fn byte_cap_stops_the_scan_and_flags_truncation() {
    let s = jsonl(&conversation(80));
    let cut = scan_bounded(Cursor::new(s.as_bytes()), "s1", MAX_FRACTIONS, 4000);
    assert!(cut.truncated);
    let full = scan_str(&s);
    assert!(cut.fractions.len() < full.fractions.len());
    assert!(!scan_str(&s).truncated);
}

#[test]
fn round_trip_finds_a_turn_three_sentence_through_a_ctx_source() {
    let mut lines = conversation(30);
    lines[3] = assistant("The vermilion gasket resonates at forty four hundred hertz under load.");
    let sc = scan_str(&jsonl(&lines));
    let rag = mem_rag();
    let report = capture_into(&rag, "s1", &sc, MAX_WRITES).expect("capture");
    assert_eq!(report.fractions, sc.fractions.len());
    assert_eq!(report.written, sc.fractions.len());
    let files = files_for(&rag, "vermilion gasket resonates");
    assert!(!files.is_empty());
    assert!(files[0].starts_with("ctx/s1/"), "{files:?}");
    assert!(files[0].ends_with(".md"));
}

#[test]
fn capturing_twice_writes_nothing_the_second_time() {
    let sc = scan_str(&jsonl(&conversation(50)));
    let rag = mem_rag();
    let first = capture_into(&rag, "s1", &sc, MAX_WRITES).expect("first");
    let chunks = rag.stats().expect("stats").chunks;
    let second = capture_into(&rag, "s1", &sc, MAX_WRITES).expect("second");
    assert_eq!(first.written, sc.fractions.len());
    assert_eq!(second.written, 0);
    assert_eq!(second.unchanged, sc.fractions.len());
    assert_eq!(rag.stats().expect("stats").chunks, chunks);
}

#[test]
fn a_grown_transcript_rewrites_only_the_tail() {
    let small = scan_str(&jsonl(&conversation(40)));
    let big = scan_str(&jsonl(&conversation(80)));
    let rag = mem_rag();
    capture_into(&rag, "s1", &small, MAX_WRITES).expect("small");
    let grown = capture_into(&rag, "s1", &big, MAX_WRITES).expect("big");
    assert!(grown.unchanged >= small.fractions.len() - 1);
    assert_eq!(grown.written + grown.unchanged, big.fractions.len());
    assert!(grown.written <= big.fractions.len() - small.fractions.len() + 1);
    assert!(grown.written >= 1);
}

#[test]
fn identical_transcripts_give_identical_databases() {
    let s = jsonl(&conversation(50));
    let a = mem_rag();
    let b = mem_rag();
    capture_into(&a, "s1", &scan_str(&s), MAX_WRITES).expect("a");
    capture_into(&b, "s1", &scan_str(&s), MAX_WRITES).expect("b");
    let ha = a.search("scheduler ledger orbit", &SearchOptions::default()).expect("a");
    let hb = b.search("scheduler ledger orbit", &SearchOptions::default()).expect("b");
    assert_eq!(ha, hb);
    let (sa, sb) = (a.stats().expect("a"), b.stats().expect("b"));
    assert_eq!((sa.chunks, sa.files, sa.text_bytes), (sb.chunks, sb.files, sb.text_bytes));
}

#[test]
fn the_write_cap_keeps_the_newest_fractions() {
    let mut lines = conversation(80);
    lines[0] = user("The ancient quokka protocol opens the first gate.");
    lines[79] = assistant("The final zeppelin handshake closes the last gate.");
    let sc = scan_str(&jsonl(&lines));
    let rag = mem_rag();
    let report = capture_into(&rag, "s1", &sc, 2).expect("capture");
    assert_eq!(report.written, 2);
    let last = fraction_source("s1", sc.fractions.len() - 1);
    assert!(files_for(&rag, "final zeppelin handshake").contains(&last));
    assert!(!files_for(&rag, "ancient quokka protocol").contains(&fraction_source("s1", 0)));
    let again = capture_into(&rag, "s1", &sc, 2).expect("again");
    assert_eq!(again.written, 2);
    assert!(again.unchanged >= 2);
}

#[test]
fn the_summary_is_indexed_under_its_own_source() {
    let mut summary: Value = serde_json::from_str(&user(&format!("{SUMMARY_PREFIX}. Summary: the pangolin migration plan"))).expect("json");
    summary["isCompactSummary"] = json!(true);
    let sc = scan_str(&jsonl(&[user("hello"), assistant("hi"), summary.to_string()]));
    let rag = mem_rag();
    let report = capture_into(&rag, "s1", &sc, MAX_WRITES).expect("capture");
    assert_eq!(report.summaries, 1);
    assert!(files_for(&rag, "pangolin migration plan").contains(&summary_source("s1", 1)));
    let again = capture_into(&rag, "s1", &sc, MAX_WRITES).expect("again");
    assert_eq!(again.written, 0);
}
