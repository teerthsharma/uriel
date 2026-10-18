use std::path::Path;

use serde_json::{json, Value};
use uriel_mcp::build_server;
use uriel_mcp::config::{parse_args_envs, Cli};
use uriel_mcp::server::Server;
use uriel_terse::data::decode;
use uriel_terse::{estimate_tokens, Value as Terse};

mod common;

use common::Proj;

const EXCLUDED: &[&str] = &["rag_expand", "terse_stats"];

fn server(root: &Path, flags: &[&str], mode: &str) -> Server {
    let root = root.to_string_lossy().into_owned();
    let mut args = vec!["--in-memory", "--mode", mode, "--root", &root];
    args.extend_from_slice(flags);
    let Cli::Run(cfg) = parse_args_envs(args, None, None).expect("args") else {
        panic!("not a run config")
    };
    build_server(&cfg).expect("server")
}

fn rpc(s: &Server, method: &str, params: Value) -> Value {
    let line = json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}).to_string();
    serde_json::from_str(&s.handle_line(&line).expect("response")).expect("json")
}

fn call(s: &Server, name: &str, args: Value) -> (String, bool) {
    let r = rpc(s, "tools/call", json!({"name": name, "arguments": args}));
    match r["result"]["content"][0]["text"].as_str() {
        Some(t) => (t.to_string(), r["result"]["isError"].as_bool().expect("isError")),
        None => (r["error"]["message"].as_str().expect("error message").to_string(), true),
    }
}

fn text(s: &Server, name: &str, args: Value) -> String {
    let (t, e) = call(s, name, args);
    assert!(!e, "{name} failed: {t}");
    t
}

fn json_of(text: &str) -> Value {
    match serde_json::from_str(text) {
        Ok(v) => v,
        Err(_) => {
            let v = decode(text).unwrap_or_else(|e| panic!("neither json nor terse ({e}): {text}"));
            serde_json::from_str(&v.to_json()).expect("json")
        }
    }
}

fn instructions(s: &Server) -> String {
    rpc(s, "initialize", json!({}))["result"]["instructions"]
        .as_str()
        .expect("instructions")
        .to_string()
}

fn tool_names(s: &Server) -> Vec<String> {
    rpc(s, "tools/list", json!({}))["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .map(|t| t["name"].as_str().expect("name").to_string())
        .collect()
}

fn calls() -> Vec<(&'static str, Value)> {
    let fmt = json!("json");
    vec![
        ("rag_index", json!({})),
        ("get_neo4j_schema", json!({})),
        (
            "read_neo4j_cypher",
            json!({"query": "MATCH (c:Chunk) RETURN c.file AS file, c.start_line AS start ORDER BY file, start", "format": fmt}),
        ),
        (
            "read_neo4j_cypher",
            json!({"query": "MATCH (n) RETURN labels(n) AS labels, count(n) AS n ORDER BY n DESC", "format": fmt}),
        ),
        (
            "write_neo4j_cypher",
            json!({"query": "CREATE (:Probe {name: 'p1', kind: 'x'}), (:Probe {name: 'p2', kind: 'x'})"}),
        ),
        (
            "create_entities",
            json!({"entities": [
                {"name": "scheduler", "type": "Module", "observations": ["picks the next job", "orders by priority and age"]},
                {"name": "gateway", "type": "Module", "observations": ["retries with backoff"]},
            ]}),
        ),
        (
            "create_relations",
            json!({"relations": [{"source": "scheduler", "target": "gateway", "relationType": "CALLS"}]}),
        ),
        ("read_graph", json!({})),
        ("search_memories", json!({"query": "backoff"})),
        ("find_memories_by_name", json!({"names": ["scheduler", "gateway"]})),
        ("house_status", json!({})),
        ("house_config", json!({})),
        (
            "research_orbit_audit",
            json!({"injective": true, "relation": [
                {"entity": "a", "answer": "x"}, {"entity": "b", "answer": "y"},
                {"entity": "c", "answer": "x"}, {"entity": "d", "answer": "z"},
            ]}),
        ),
        (
            "research_pooling_audit",
            json!({"recoveries": [{"entity": "a", "recovery": 0.9}, {"entity": "b", "recovery": 0.4}],
                "relation": [
                    {"entity": "a", "answer": "x"}, {"entity": "b", "answer": "x"},
                    {"entity": "c", "answer": "y"},
                ]}),
        ),
        (
            "seal_remember",
            json!({"text": "The scheduler retries the payment gateway with a backoff delay of 200ms.", "tags": ["scheduler"]}),
        ),
        ("seal_recall", json!({"query": "scheduler backoff", "format": fmt})),
        ("rag_search", json!({"query": "backoff delay", "format": fmt})),
        ("rag_search", json!({"query": "invoice customer billing"})),
        ("view", json!({"question": "scheduler priority", "budget_tokens": 1500, "format": fmt})),
        ("soul", json!({"format": fmt})),
        ("session_reset", json!({})),
        ("rag_feedback", json!({"used": [1], "unused": [2]})),
    ]
}

const LEAD: &str = "{\"prefix_id\"";

fn scrub(text: &str) -> Value {
    let mut v: Value = serde_json::from_str(text).expect("json");
    if let Some(m) = v.as_object_mut() {
        m.remove("prefix_id");
        m.remove("preamble");
    }
    v
}

fn equivalent(name: &str, off: &str, got: &str) {
    if off.starts_with(LEAD) {
        assert!(got.starts_with(LEAD), "{name}: {got}");
        assert_eq!(scrub(off), scrub(got), "{name}");
        return;
    }
    match Terse::parse(off) {
        Ok(expected) => {
            let direct = Terse::parse(got).ok().filter(|v| *v == expected);
            let decoded = decode(got).ok().filter(|v| *v == expected);
            assert!(direct.is_some() || decoded.is_some(), "{name}: {got:?} does not carry {off:?}");
            assert!(
                estimate_tokens(got) <= estimate_tokens(off),
                "{name}: encoding grew {} to {}",
                estimate_tokens(off),
                estimate_tokens(got)
            );
        }
        Err(_) => assert_eq!(got, off, "{name}: non-JSON output must be untouched"),
    }
}

#[test]
fn every_json_result_in_data_mode_carries_the_same_value_as_off() {
    let p = Proj::new();
    let off = server(p.dir.path(), &[], "house");
    let data = server(p.dir.path(), &["--terse", "data"], "house");
    let mut shrank = 0;
    for (name, args) in calls() {
        let (a, a_err) = call(&off, name, args.clone());
        let (b, b_err) = call(&data, name, args);
        assert_eq!(a_err, b_err, "{name}");
        equivalent(name, &a, &b);
        if estimate_tokens(&b) < estimate_tokens(&a) {
            shrank += 1;
        }
    }
    assert!(shrank >= 8, "only {shrank} results shrank");
}

#[test]
fn off_changes_no_byte_and_adds_no_tool_or_instruction() {
    let p = Proj::new();
    let plain = server(p.dir.path(), &[], "house");
    let off = server(p.dir.path(), &["--terse", "off"], "house");
    for (name, args) in calls() {
        let compact_search = name == "rag_search" && args.get("format").is_none();
        let (got, _) = call(&off, name, args.clone());
        assert_eq!(call(&plain, name, args), (got.clone(), false), "{name}");
        assert!(compact_search || serde_json::from_str::<Value>(&got).is_ok(), "{name} is not plain json: {got}");
    }
    assert_eq!(instructions(&plain), instructions(&off));
    assert!(!instructions(&off).contains("Terse"));
    assert!(!tool_names(&off).contains(&"terse_stats".to_string()));
    let (msg, is_error) = call(&off, "terse_stats", json!({}));
    assert!(is_error, "{msg}");
    assert!(msg.contains("Unknown tool"), "{msg}");
}

#[test]
fn compact_text_results_are_never_touched_in_data_mode() {
    let p = Proj::new();
    let off = server(p.dir.path(), &[], "normal");
    let data = server(p.dir.path(), &["--terse", "data"], "normal");
    for s in [&off, &data] {
        text(s, "rag_index", json!({}));
    }
    for (name, args) in [
        ("rag_search", json!({"query": "backoff delay"})),
        ("rag_search", json!({"query": "scheduler priority", "format": "compact"})),
        ("seal_recall", json!({"query": "scheduler"})),
        ("view", json!({"question": "invoice", "budget_tokens": 1500, "format": "compact"})),
        ("soul", json!({"format": "compact"})),
        ("read_neo4j_cypher", json!({"query": "MATCH (c:Chunk) RETURN c.file AS f ORDER BY f"})),
        ("rag_expand", json!({"chunk_id": 1, "level": 0})),
    ] {
        assert_eq!(call(&off, name, args.clone()), call(&data, name, args), "{name}");
    }
}

#[test]
fn error_results_stay_plain_json() {
    let p = Proj::new();
    let off = server(p.dir.path(), &[], "normal");
    let data = server(p.dir.path(), &["--terse", "data"], "normal");
    let a = call(&off, "house_status", json!({}));
    let b = call(&data, "house_status", json!({}));
    assert!(a.1 && b.1);
    assert_eq!(a, b);
    let parsed: Value = serde_json::from_str(&b.0).expect("plain json");
    assert_eq!(parsed["error"], "wrong_mode");
    let c = call(&data, "read_neo4j_cypher", json!({"query": "MATCH (n RETURN n"}));
    let d = call(&off, "read_neo4j_cypher", json!({"query": "MATCH (n RETURN n"}));
    assert!(c.1);
    assert_eq!(c, d);
}

#[test]
fn the_instruction_line_is_appended_for_every_variant_and_only_when_on() {
    let p = Proj::new();
    for mode in ["normal", "house"] {
        for ro in [false, true] {
            let flags_off: Vec<&str> = if ro { vec!["--read-only"] } else { vec![] };
            let base = instructions(&server(p.dir.path(), &flags_off, mode));
            for level in ["data", "prose"] {
                let mut flags = flags_off.clone();
                flags.extend(["--terse", level]);
                let got = instructions(&server(p.dir.path(), &flags, mode));
                let extra = got
                    .strip_prefix(&base)
                    .unwrap_or_else(|| panic!("{mode} ro={ro}: {got}"))
                    .trim();
                assert!(extra.starts_with("Terse"), "{extra}");
                assert!(!extra.contains('\n'));
                assert!(estimate_tokens(extra) <= 60, "{} tokens: {extra}", estimate_tokens(extra));
                for word in ["k[n]", "{", "^", "$", "!"] {
                    assert!(extra.contains(word), "grammar line lacks {word}: {extra}");
                }
            }
        }
    }
}

#[test]
fn the_scaffold_fingerprint_never_shares_an_id_across_terse_levels() {
    let p = Proj::new();
    let id = |flags: &[&str]| server(p.dir.path(), flags, "normal").scaffold_fingerprint().expect("fingerprint");
    let off = id(&[]);
    let data = id(&["--terse", "data"]);
    let prose = id(&["--terse", "prose"]);
    assert_ne!(off, data);
    assert_ne!(off, prose);
    assert_ne!(data, prose);
    assert_eq!(off, id(&["--terse", "off"]));
    assert_eq!(data, id(&["--terse", "data"]));
}

#[test]
fn the_stats_tool_counts_exactly_what_was_shaped() {
    let p = Proj::new();
    let off = server(p.dir.path(), &[], "house");
    let data = server(p.dir.path(), &["--terse", "data"], "house");
    assert!(tool_names(&data).contains(&"terse_stats".to_string()));
    let mut before = 0usize;
    let mut after = 0usize;
    for (name, args) in calls() {
        let a = text(&off, name, args.clone());
        let b = text(&data, name, args);
        if !EXCLUDED.contains(&name) && !a.starts_with(LEAD) && Terse::parse(&a).is_ok() {
            before += estimate_tokens(&a);
            after += estimate_tokens(&b);
        }
    }
    let stats: Value = serde_json::from_str(&text(&data, "terse_stats", json!({}))).expect("json");
    assert_eq!(stats["terse"], "data");
    assert_eq!(stats["terse_before_tokens"], before, "{stats}");
    assert_eq!(stats["terse_after_tokens"], after, "{stats}");
    assert!(before > after && after > 0, "{before} {after}");
    let ratio = stats["saved_ratio"].as_f64().expect("ratio");
    assert!((ratio - (1.0 - after as f64 / before as f64)).abs() < 1e-9, "{stats}");
    let again: Value = serde_json::from_str(&text(&data, "terse_stats", json!({}))).expect("json");
    assert_eq!(again["terse_before_tokens"], stats["terse_before_tokens"], "the stats call is not itself counted");
}

#[test]
fn the_stable_prefix_lead_is_left_byte_exact() {
    let p = Proj::new();
    let off = server(p.dir.path(), &[], "normal");
    let data = server(p.dir.path(), &["--terse", "data"], "normal");
    for s in [&off, &data] {
        text(s, "rag_index", json!({}));
    }
    let a = text(&off, "view", json!({"question": "scheduler", "budget_tokens": 1500, "format": "json"}));
    let b = text(&data, "view", json!({"question": "scheduler", "budget_tokens": 1500, "format": "json"}));
    assert!(a.starts_with("{\"prefix_id\""), "{a}");
    assert!(b.starts_with("{\"prefix_id\""), "{b}");
    let (pa, pb): (Value, Value) = (serde_json::from_str(&a).unwrap(), serde_json::from_str(&b).unwrap());
    assert_eq!(pa["stable_prefix_bytes"], pb["stable_prefix_bytes"]);
    assert_ne!(pa["prefix_id"], pb["prefix_id"], "terse on and off never share a prefix id");
}

const FINDING: &str = "We should basically make sure that the scheduler really does always retry the payment gateway with a backoff delay of 200ms, in order to avoid the thundering herd problem, because it is just a very important thing to remember.";

#[test]
fn prose_compacts_a_returned_finding_but_never_the_stored_one() {
    let p = Proj::new();
    let data = server(p.dir.path(), &["--terse", "data"], "normal");
    let prose = server(p.dir.path(), &["--terse", "prose"], "normal");
    for s in [&data, &prose] {
        text(s, "rag_index", json!({}));
        text(s, "seal_remember", json!({"text": FINDING, "tags": ["scheduler"]}));
        let rows = json_of(&text(
            s,
            "read_neo4j_cypher",
            json!({"query": "MATCH (d:Decision) RETURN d.text AS text", "format": "json"}),
        ));
        assert_eq!(rows[0]["text"], FINDING, "storage stays byte-exact");
    }
    let q = json!({"query": "thundering herd backoff", "fresh": true, "ladder": false});
    let d = text(&data, "seal_recall", q.clone());
    let r = text(&prose, "seal_recall", q);
    assert!(estimate_tokens(&r) < estimate_tokens(&d), "{d}\n---\n{r}");
    for kept in ["200ms", "backoff", "scheduler"] {
        assert!(r.contains(kept), "{kept} lost: {r}");
    }
    let stats = json_of(&text(&prose, "terse_stats", json!({})));
    assert!(stats["terse_after_tokens"].as_u64().unwrap() < stats["terse_before_tokens"].as_u64().unwrap());
}

#[test]
fn prose_never_touches_code_chunks_or_citations() {
    let p = Proj::new();
    let data = server(p.dir.path(), &["--terse", "data"], "normal");
    let prose = server(p.dir.path(), &["--terse", "prose"], "normal");
    for s in [&data, &prose] {
        text(s, "rag_index", json!({}));
    }
    for (name, args) in [
        ("rag_search", json!({"query": "backoff delay", "fresh": true})),
        ("rag_search", json!({"query": "backoff delay", "format": "json", "fresh": true})),
        ("rag_expand", json!({"chunk_id": 1, "level": 0})),
        ("view", json!({"question": "invoice", "budget_tokens": 1500, "format": "compact"})),
    ] {
        assert_eq!(call(&data, name, args.clone()), call(&prose, name, args), "{name}");
    }
}

const FILLER: &str = "you should basically make sure that the gateway client really does always retry with a backoff delay, in order to avoid the thundering herd problem, because it is just a very important thing.";

fn filler_project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("dir");
    std::fs::create_dir_all(dir.path().join("docs")).unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    let guide: String = (0..12).map(|i| format!("Step {i}: {FILLER}

")).collect();
    std::fs::write(dir.path().join("docs/guide.md"), format!("# Retry guide
{guide}")).unwrap();
    let comments: String = (0..12).map(|i| format!("    // Step {i}: {FILLER}
")).collect();
    std::fs::write(
        dir.path().join("src/retry.rs"),
        format!("pub fn retry_with_backoff(attempt: u32) -> u64 {{
{comments}    100u64 << attempt.min(8)
}}
"),
    )
    .unwrap();
    dir
}

fn section<'a>(guide: &'a str, file: &str) -> &'a str {
    let start = guide.find(&format!("// {file}:")).unwrap_or_else(|| panic!("no {file} in {guide}"));
    let rest = &guide[start..];
    let end = rest[3..].find("\n// ").map_or(rest.len(), |i| i + 3);
    &rest[..end]
}

#[test]
fn prose_compacts_documentation_sections_of_the_soul_but_not_code() {
    let dir = filler_project();
    let line = format!("Step 3: {FILLER}");
    assert_ne!(uriel_terse::compact(&line, uriel_terse::Level::Lite), line, "the fixture must be compactable");
    let data = server(dir.path(), &["--terse", "data"], "normal");
    let prose = server(dir.path(), &["--terse", "prose"], "normal");
    for s in [&data, &prose] {
        text(s, "rag_index", json!({}));
    }
    let a = text(&data, "soul", json!({"budget_tokens": 4000}));
    let b = text(&prose, "soul", json!({"budget_tokens": 4000}));
    assert!(estimate_tokens(&b) < estimate_tokens(&a), "{a}
---
{b}");
    assert_eq!(section(&a, "src/retry.rs"), section(&b, "src/retry.rs"));
    assert_ne!(section(&a, "docs/guide.md"), section(&b, "docs/guide.md"));
    let only_code = json!({"budget_tokens": 4000, "focus": "src"});
    let c = text(&data, "soul", only_code.clone());
    assert!(c.contains("basically"), "{c}");
    assert_eq!(c, text(&prose, "soul", only_code));
}

#[test]
fn prose_leaves_code_hits_of_a_search_untouched() {
    let dir = filler_project();
    let data = server(dir.path(), &["--terse", "data"], "normal");
    let prose = server(dir.path(), &["--terse", "prose"], "normal");
    for s in [&data, &prose] {
        text(s, "rag_index", json!({}));
    }
    for q in ["retry_with_backoff attempt", "retry with a backoff delay gateway client"] {
        for format in ["compact", "json"] {
            let args = json!({"query": q, "fresh": true, "format": format});
            let a = text(&data, "rag_search", args.clone());
            assert_eq!(a, text(&prose, "rag_search", args), "{q} {format}");
            assert!(a.contains("src/retry.rs"), "{a}");
        }
    }
}

#[test]
fn rag_expand_returns_json_file_text_byte_exact_in_every_level() {
    let dir = tempfile::tempdir().expect("dir");
    let body = "{\n  \"alpha\": [1, 2, 3],\n  \"beta\": {\"gamma\": \"delta\", \"eps\": \"zeta\"},\n  \"rows\": [{\"k\": 1, \"v\": \"a\"}, {\"k\": 2, \"v\": \"b\"}]\n}\n";
    std::fs::write(dir.path().join("conf.json"), body).unwrap();
    let expand = |level: &str| {
        let s = server(dir.path(), &["--terse", level], "normal");
        text(&s, "rag_index", json!({}));
        text(&s, "rag_expand", json!({"chunk_id": 1, "level": 0}))
    };
    let plain = expand("off");
    assert!(Terse::parse(&plain).is_ok(), "fixture must be json: {plain}");
    assert_eq!(plain.trim_end(), body.trim_end());
    assert_eq!(expand("data"), plain);
    assert_eq!(expand("prose"), plain);
}

fn without_format(mut args: Value) -> Value {
    if let Some(m) = args.as_object_mut() {
        m.remove("format");
    }
    args
}

#[test]
fn data_and_prose_never_cost_more_than_off_for_any_tool_at_its_default_format() {
    let p = Proj::new();
    let servers = [
        server(p.dir.path(), &[], "house"),
        server(p.dir.path(), &["--terse", "data"], "house"),
        server(p.dir.path(), &["--terse", "prose"], "house"),
    ];
    let mut per_tool: std::collections::BTreeMap<&str, [usize; 3]> = std::collections::BTreeMap::new();
    for (name, args) in calls() {
        let args = without_format(args);
        for (i, s) in servers.iter().enumerate() {
            let t = text(s, name, args.clone());
            per_tool.entry(name).or_insert([0; 3])[i] += estimate_tokens(&t);
        }
    }
    let mut saved = 0;
    for (name, [off, data, prose]) in &per_tool {
        assert!(data <= off, "{name}: data {data} > off {off}");
        assert!(prose <= off, "{name}: prose {prose} > off {off}");
        saved += off - data;
    }
    assert!(saved > 0, "{per_tool:?}");
    let total = |i: usize| per_tool.values().map(|t| t[i]).sum::<usize>();
    assert!(total(1) <= total(0) && total(2) <= total(0));
}
