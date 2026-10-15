mod common;

use std::time::{Duration, Instant};

use common::*;
use uriel_mcp::compact::{decode_pack, decode_table};
use uriel_place::replay::{replay_digest, replay_placements, Scenario};
use uriel_place::{AccessKind, Config, Event, Level, Region, RegionId};
use serde_json::{json, Value};

const NEW_TOOLS: &[(&str, &[&str])] = &[
    ("calibrate", &[]),
    ("why_hot", &["node"]),
    ("pin", &["node"]),
    ("unpin", &["node"]),
    ("replay", &["events"]),
    ("view", &["question", "budget_tokens"]),
    ("tier_stats", &[]),
];

const BACKOFF: &str = "exponential backoff retry payment gateway";
const MIXED: &str = "scheduler job priority backoff invoice settings";

struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn proxy(s: &str) -> usize {
    s.chars().count().div_ceil(4)
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn planned(p: &Proj) -> Session {
    p.session(&["--placement", "planned"], &[])
}

fn indexed(p: &Proj, extra: &[&str]) -> Session {
    let mut s = p.session(extra, &[]);
    s.ok("rag_index", json!({}));
    s
}

fn log(seed: u64) -> Vec<Value> {
    let mut r = Lcg(seed * 7919 + 13);
    let n = 30 + r.below(60);
    (0..n)
        .map(|_| {
            let top = r.below(70) + 1;
            let region = r.below(top);
            match r.below(100) {
                0..=54 => json!({"op": "read", "region": region}),
                55..=64 => json!({"op": "write", "region": region}),
                65..=74 => json!({"op": "scan", "region": region}),
                75..=80 => json!({"op": "pin", "region": region}),
                81..=85 => json!({"op": "unpin", "region": region}),
                86..=90 => json!({"op": "resize", "region": region, "size": 1024u64 << r.below(5)}),
                _ => {
                    let level = ["normal", "elevated", "high", "critical"][r.below(4) as usize];
                    json!({"op": "pressure", "level": level})
                }
            }
        })
        .collect()
}

fn to_event(v: &Value) -> Event {
    let region = RegionId(v["region"].as_u64().unwrap_or(0) as u32);
    match v["op"].as_str().unwrap_or("") {
        "read" => Event::Access {
            region,
            kind: AccessKind::Read,
        },
        "write" => Event::Access {
            region,
            kind: AccessKind::Write,
        },
        "scan" => Event::Access {
            region,
            kind: AccessKind::Scan,
        },
        "pin" => Event::Pin { region },
        "unpin" => Event::Unpin { region },
        "resize" => Event::Resize {
            region,
            size: v["size"].as_u64().unwrap_or(0),
        },
        _ => Event::Pressure {
            level: match v["level"].as_str() {
                Some("elevated") => Level::Elevated,
                Some("high") => Level::High,
                Some("critical") => Level::Critical,
                _ => Level::Normal,
            },
        },
    }
}

fn scenario(events: &[Value]) -> Scenario {
    let regions: Vec<Region> = (0..64)
        .map(|i| Region {
            id: RegionId(i),
            size: 4096,
            community: i / 8,
        })
        .collect();
    Scenario {
        config: Config {
            hot_bytes: 64 * 4096 / 4,
            warm_bytes: 64 * 4096 / 4,
            ..Config::default()
        },
        regions,
        edges: Vec::new(),
        events: events.iter().map(to_event).collect(),
        plan_every: 4,
    }
}

fn replay(s: &mut Session, events: &[Value]) -> Value {
    s.ok("replay", json!({"events": events}))
}

fn fill(s: &mut Session, n: u32) {
    s.text(
        "write_neo4j_cypher",
        json!({"query": format!("UNWIND range(1, {n}) AS i CREATE (:F {{i: i}})")}),
    );
}

fn node_id(s: &mut Session, i: u32) -> u64 {
    let r = s.ok(
        "read_neo4j_cypher",
        json!({"query": format!("MATCH (n:F {{i: {i}}}) RETURN id(n) AS id")}),
    );
    r[0]["id"].as_u64().expect("node id")
}

fn touch(s: &mut Session, i: u32) {
    s.ok(
        "read_neo4j_cypher",
        json!({"query": format!("MATCH (n:F {{i: {i}}}) RETURN n")}),
    );
}

fn view_json(s: &mut Session, question: &str, budget: usize) -> Value {
    s.ok(
        "view",
        json!({"question": question, "budget_tokens": budget}),
    )
}

#[test]
fn tool_list_carries_every_new_tool_with_an_object_schema() {
    let p = Proj::new();
    let mut s = p.session(&[], &[]);
    let list = s.rpc("tools/list", json!({}));
    let tools = list["result"]["tools"].as_array().expect("tools");
    for (name, required) in NEW_TOOLS {
        let t = tools
            .iter()
            .find(|t| t["name"] == *name)
            .unwrap_or_else(|| panic!("{name} missing"));
        let schema = &t["inputSchema"];
        assert_eq!(schema["type"], "object", "{name}");
        assert!(schema["properties"].is_object(), "{name}");
        assert_eq!(schema["additionalProperties"], false, "{name}");
        assert!(t["description"].as_str().unwrap_or("").len() > 40, "{name}");
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
        assert!(schema["properties"]["format"].is_object(), "{name}.format");
    }
}

#[test]
fn only_pin_and_unpin_are_marked_mutating() {
    let p = Proj::new();
    let mut s = p.session(&[], &[]);
    let list = s.rpc("tools/list", json!({}));
    let mut seen = 0;
    for t in list["result"]["tools"].as_array().expect("tools") {
        let name = t["name"].as_str().expect("name");
        if NEW_TOOLS.iter().any(|(n, _)| *n == name) {
            let mutating = name == "pin" || name == "unpin";
            assert_eq!(t["annotations"]["readOnlyHint"], !mutating, "{name}");
            seen += 1;
        }
    }
    assert_eq!(seen, NEW_TOOLS.len());
}

#[test]
fn read_only_hides_pin_and_unpin_and_keeps_the_other_new_tools() {
    let p = Proj::new();
    let mut s = p.session(&["--read-only"], &[]);
    let names = s.names();
    for hidden in ["pin", "unpin"] {
        assert!(!names.contains(&hidden.to_string()), "{hidden} visible");
    }
    for (shown, _) in NEW_TOOLS {
        if *shown != "pin" && *shown != "unpin" {
            assert!(names.contains(&shown.to_string()), "{shown} missing");
        }
    }
}

#[test]
fn read_only_denies_pin_and_unpin_with_a_tool_error() {
    let p = Proj::new();
    let mut s = p.session(&["--read-only", "--placement", "planned"], &[]);
    for tool in ["pin", "unpin"] {
        let e = s.err(tool, json!({"node": 3}));
        assert!(e.contains("--read-only"), "{tool}: {e}");
    }
}

#[test]
fn every_new_tool_answers_a_database_that_was_never_indexed() {
    let p = Proj::new();
    let mut s = planned(&p);
    for (tool, args) in [
        ("tier_stats", json!({})),
        ("why_hot", json!({"node": 7})),
        ("pin", json!({"node": 7})),
        ("unpin", json!({"node": 7})),
        ("replay", json!({"events": []})),
        ("calibrate", json!({"budget_secs": 0.3})),
    ] {
        let t = s.text(tool, args);
        assert!(!t.is_empty(), "{tool}");
    }
    assert_eq!(s.text("view", json!({"question": BACKOFF, "budget_tokens": 200})), "#0");
    let v = view_json(&mut s, BACKOFF, 200);
    assert_eq!(v["hits"], json!([]), "{v}");
    assert_eq!(v["tokens"], 0);
}

#[test]
fn bad_argument_shapes_are_tool_errors_for_every_new_tool() {
    let p = Proj::new();
    let mut s = p.session(&[], &[]);
    for (name, _) in NEW_TOOLS {
        let r = s.rpc("tools/call", json!({"name": name, "arguments": [1, 2]}));
        assert_eq!(r["result"]["isError"], true, "{name}: {r}");
        assert!(r.get("error").is_none(), "{name}: {r}");
    }
}

#[test]
fn tier_stats_default_output_is_compact_and_decodes() {
    let p = Proj::new();
    let mut s = indexed(&p, &[]);
    let t = s.text("tier_stats", json!({}));
    assert!(t.starts_with("#1"), "{t}");
    let table = decode_table(&t).expect("decode");
    assert!(table.columns.contains(&"stored_bytes".to_string()), "{t}");
    assert!(table.columns.contains(&"logical_bytes".to_string()), "{t}");
}

#[test]
fn tier_stats_json_matches_the_compact_cells() {
    let p = Proj::new();
    let mut s = indexed(&p, &[]);
    let compact = s.text("tier_stats", json!({}));
    let j = s.ok("tier_stats", json!({}));
    let row = &decode_table(&compact).expect("decode").to_rows()[0];
    assert_eq!(row["stored_bytes"], j["stored_bytes"]);
    assert_eq!(row["logical_bytes"], j["logical_bytes"]);
}

#[test]
fn tier_stats_reports_bytes_on_disk_against_logical_bytes() {
    let p = Proj::new();
    let mut s = indexed(&p, &[]);
    let j = s.ok("tier_stats", json!({}));
    let stored = j["stored_bytes"].as_u64().expect("stored");
    let logical = j["logical_bytes"].as_u64().expect("logical");
    assert!(stored > 0 && logical > 0, "{j}");
    let ratio = j["compression_ratio"].as_f64().expect("ratio");
    assert!((ratio - logical as f64 / stored as f64).abs() < 1e-9, "{j}");
    assert!(j["hot"]["entries"].is_u64() && j["warm"]["bytes"].is_u64(), "{j}");
}

#[test]
fn tier_stats_on_an_unindexed_database_holds_no_bytes() {
    let p = Proj::new();
    let mut s = p.session(&["--read-only"], &[]);
    let j = s.ok("tier_stats", json!({}));
    assert_eq!(j["stored_bytes"], 0, "{j}");
    assert_eq!(j["logical_bytes"], 0, "{j}");
}

#[test]
fn tier_stats_rejects_a_bad_format_as_a_tool_error() {
    let p = Proj::new();
    let mut s = p.session(&[], &[]);
    for bad in [json!({"format": "xml"}), json!({"format": 3})] {
        s.err("tier_stats", bad);
    }
}

#[test]
fn calibrate_returns_knees_and_a_hot_size_within_its_budget() {
    let p = Proj::new();
    let mut s = p.session(&[], &[]);
    let t = Instant::now();
    let r = s.ok("calibrate", json!({"budget_secs": 1}));
    assert!(t.elapsed() < Duration::from_secs(4), "{:?}", t.elapsed());
    assert!(r["suggest_hot_bytes"].as_u64().unwrap_or(0) >= 4096, "{r}");
    assert!(r["points"].as_u64().unwrap_or(0) >= 4, "{r}");
    for k in r["knees"].as_array().expect("knees") {
        assert!(k["from_bytes"].as_u64() < k["to_bytes"].as_u64(), "{k}");
        assert!(k["ratio_x100"].as_u64().unwrap_or(0) > 130, "{k}");
    }
    assert!(r["elapsed_ms"].as_u64().unwrap_or(u64::MAX) < 3500, "{r}");
}

#[test]
fn calibrate_persists_its_json_next_to_the_database() {
    let p = Proj::new();
    let mut s = p.session(&["--db", &p.db()], &[]);
    let r = s.ok("calibrate", json!({"budget_secs": 0.5}));
    let file = p.dir.path().join("calibration.json");
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(&file).expect("file"))
        .expect("calibration json");
    assert_eq!(saved["schema"], "uriel-place.calibration.v1");
    assert_eq!(saved["suggested_hot_bytes"], r["suggest_hot_bytes"]);
    assert_eq!(
        r["persisted"].as_str().map(std::path::PathBuf::from),
        Some(file)
    );
}

#[test]
fn calibrate_writes_nothing_under_read_only_or_in_memory() {
    let p = Proj::new();
    let mut ro = p.session(&["--db", &p.db(), "--read-only"], &[]);
    let r = ro.ok("calibrate", json!({"budget_secs": 0.3}));
    assert!(r["persisted"].is_null(), "{r}");
    let mut mem = p.session(&[], &[]);
    let r = mem.ok("calibrate", json!({"budget_secs": 0.3}));
    assert!(r["persisted"].is_null(), "{r}");
    assert!(!p.dir.path().join("calibration.json").exists());
}

#[test]
fn calibrate_rejects_hostile_budgets_without_running() {
    let p = Proj::new();
    let mut s = p.session(&[], &[]);
    let t = Instant::now();
    for bad in [
        json!({"budget_secs": 0}),
        json!({"budget_secs": -3}),
        json!({"budget_secs": "9"}),
        json!({"budget_secs": true}),
        json!({"budget_secs": [1]}),
        json!({"budget_secs": {}}),
        json!({"budget_secs": 0.3, "format": "xml"}),
    ] {
        s.err("calibrate", bad);
    }
    assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
}

#[test]
fn calibrate_default_output_is_compact() {
    let p = Proj::new();
    let mut s = p.session(&[], &[]);
    let t = s.text("calibrate", json!({"budget_secs": 0.3}));
    let table = decode_table(&t).expect("decode");
    assert!(table.columns.contains(&"suggest_hot_bytes".to_string()), "{t}");
}

#[test]
fn replay_returns_a_sha256_digest_and_final_counts() {
    let p = Proj::new();
    let mut s = p.session(&[], &[]);
    let r = replay(&mut s, &log(1));
    let d = r["digest"].as_str().expect("digest");
    assert_eq!(d.len(), 64, "{r}");
    assert!(d.chars().all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()), "{r}");
    let (h, w, c) = (
        r["hot"].as_u64().expect("hot"),
        r["warm"].as_u64().expect("warm"),
        r["cold"].as_u64().expect("cold"),
    );
    assert_eq!(h + w + c, r["regions"].as_u64().expect("regions"), "{r}");
    assert_eq!(r["events"], log(1).len());
}

#[test]
fn replay_gives_the_same_digest_for_the_same_log_over_20_logs() {
    let p = Proj::new();
    let mut s = p.session(&[], &[]);
    let mut seen = std::collections::BTreeSet::new();
    for seed in 0..20 {
        let events = log(seed);
        let a = replay(&mut s, &events);
        let b = replay(&mut s, &events);
        assert_eq!(a, b, "seed {seed}");
        seen.insert(a["digest"].as_str().expect("digest").to_string());
    }
    assert!(seen.len() >= 18, "{} distinct digests of 20", seen.len());
}

#[test]
fn replay_digest_is_the_same_across_server_restarts() {
    let p = Proj::new();
    let events = log(5);
    let first = replay(&mut p.session(&[], &[]), &events);
    let second = replay(&mut p.session(&[], &[]), &events);
    assert_eq!(first, second);
}

#[test]
fn replay_digest_equals_the_library_digest_over_20_logs() {
    let p = Proj::new();
    let mut s = p.session(&[], &[]);
    for seed in 0..20 {
        let events = log(seed);
        let r = replay(&mut s, &events);
        let sc = scenario(&events);
        assert_eq!(r["digest"], hex(&replay_digest(&sc)), "seed {seed}");
        let last = replay_placements(&sc).pop().expect("placement");
        assert_eq!(r["hot"], last.hot.len(), "seed {seed}");
        assert_eq!(r["warm"], last.warm.len(), "seed {seed}");
        assert_eq!(r["cold"], last.cold.len(), "seed {seed}");
    }
}

#[test]
fn replay_digest_changes_when_one_more_event_is_appended() {
    let p = Proj::new();
    let mut s = p.session(&[], &[]);
    for seed in 0..20 {
        let mut events = log(seed);
        let before = replay(&mut s, &events)["digest"].clone();
        events.push(json!({"op": "read", "region": 1}));
        assert_ne!(before, replay(&mut s, &events)["digest"], "seed {seed}");
    }
}

#[test]
fn replay_of_an_empty_log_leaves_every_region_cold() {
    let p = Proj::new();
    let mut s = p.session(&[], &[]);
    let r = replay(&mut s, &[]);
    assert_eq!((r["hot"].clone(), r["warm"].clone()), (json!(0), json!(0)), "{r}");
    assert_eq!(r["cold"], r["regions"], "{r}");
    assert_eq!(r["digest"].as_str().map(str::len), Some(64));
}

#[test]
fn replay_counts_events_on_unknown_regions_as_ignored() {
    let p = Proj::new();
    let mut s = p.session(&[], &[]);
    let events = vec![
        json!({"op": "read", "region": 3}),
        json!({"op": "read", "region": 4_000_000}),
        json!({"op": "pin", "region": 99}),
    ];
    let r = replay(&mut s, &events);
    assert_eq!(r["ignored"], 2, "{r}");
}

#[test]
fn replay_critical_pressure_shrinks_the_hot_set() {
    let p = Proj::new();
    let mut s = p.session(&[], &[]);
    let mut events: Vec<Value> = (0..40)
        .flat_map(|r| (0..3).map(move |_| json!({"op": "read", "region": r})))
        .collect();
    let normal = replay(&mut s, &events)["hot"].as_u64().expect("hot");
    events.push(json!({"op": "pressure", "level": "critical"}));
    let critical = replay(&mut s, &events)["hot"].as_u64().expect("hot");
    assert!(normal >= 8, "{normal}");
    assert!(critical <= 2 && critical < normal, "{critical} vs {normal}");
}

#[test]
fn replay_honours_regions_and_sizes_given_by_the_caller() {
    let p = Proj::new();
    let mut s = p.session(&[], &[]);
    let r = s.ok(
        "replay",
        json!({"events": [{"op": "read", "region": 2}], "regions": 10, "region_size": 512, "plan_every": 1}),
    );
    assert_eq!(r["regions"], 10, "{r}");
    assert_eq!(r["hot"], 1, "{r}");
    assert_eq!(r["plans"], 2, "{r}");
}

#[test]
fn replay_default_output_is_compact_and_decodes() {
    let p = Proj::new();
    let mut s = p.session(&[], &[]);
    let t = s.text("replay", json!({"events": log(2)}));
    let rows = decode_table(&t).expect("decode").to_rows();
    assert_eq!(rows[0]["digest"].as_str().map(str::len), Some(64), "{t}");
}

#[test]
fn replay_rejects_hostile_arguments_as_tool_errors() {
    let p = Proj::new();
    let mut s = p.session(&[], &[]);
    for bad in [
        json!({}),
        json!({"events": "read"}),
        json!({"events": [1]}),
        json!({"events": [{"op": "explode", "region": 1}]}),
        json!({"events": [{"region": 1}]}),
        json!({"events": [{"op": "read"}]}),
        json!({"events": [{"op": "read", "region": -1}]}),
        json!({"events": [{"op": "read", "region": 4294967296u64}]}),
        json!({"events": [{"op": "read", "region": 1.5}]}),
        json!({"events": [{"op": "resize", "region": 1}]}),
        json!({"events": [{"op": "resize", "region": 1, "size": -4}]}),
        json!({"events": [{"op": "pressure", "level": "meltdown"}]}),
        json!({"events": [{"op": "pressure"}]}),
        json!({"events": [], "regions": 65537}),
        json!({"events": [], "regions": -1}),
        json!({"events": [], "region_size": 0}),
        json!({"events": [], "plan_every": 0}),
        json!({"events": [], "hot_bytes": "x"}),
        json!({"events": [], "format": "yaml"}),
    ] {
        let e = s.err("replay", bad.clone());
        assert!(!e.is_empty(), "{bad}");
    }
    assert_eq!(replay(&mut s, &[])["events"], 0);
}

#[test]
fn replay_refuses_a_log_longer_than_the_cap() {
    let p = Proj::new();
    let mut s = p.session(&[], &[]);
    let events: Vec<Value> = (0..100_001)
        .map(|_| json!({"op": "read", "region": 1}))
        .collect();
    let e = s.err("replay", json!({"events": events}));
    assert!(e.contains("100000"), "{e}");
}

#[test]
fn why_hot_under_local_answers_with_tier_stats_and_a_note() {
    let p = Proj::new();
    let mut s = indexed(&p, &[]);
    let r = s.ok("why_hot", json!({"node": 3}));
    assert_eq!(r["mode"], "local", "{r}");
    assert_eq!(r["plan"], false, "{r}");
    assert!(r["note"].as_str().unwrap_or("").contains("no plan exists"), "{r}");
    assert!(r["tier_stats"]["stored_bytes"].as_u64().unwrap_or(0) > 0, "{r}");
}

#[test]
fn why_hot_for_an_untouched_node_is_cold_with_no_access() {
    let p = Proj::new();
    let mut s = planned(&p);
    let r = s.ok("why_hot", json!({"node": 5}));
    assert_eq!(r["mode"], "planned", "{r}");
    assert_eq!(r["tier"], "cold", "{r}");
    assert_eq!(r["heat"], 0.0, "{r}");
    assert_eq!(r["pinned"], false, "{r}");
    assert!(r["last_access"].is_null(), "{r}");
    assert_eq!(r["prefetch"], false, "{r}");
    assert_eq!(r["topology_weight"], 0.0, "{r}");
    assert_eq!(r["region"], 0, "{r}");
}

#[test]
fn why_hot_names_the_region_of_a_node_id() {
    let p = Proj::new();
    let mut s = planned(&p);
    assert_eq!(s.ok("why_hot", json!({"node": 15}))["region"], 0);
    assert_eq!(s.ok("why_hot", json!({"node": 16}))["region"], 1);
    assert_eq!(s.ok("why_hot", json!({"node": 161}))["region"], 10);
}

#[test]
fn why_hot_after_a_read_reports_hot_with_heat_and_last_access() {
    let p = Proj::new();
    let mut s = planned(&p);
    fill(&mut s, 40);
    let id = node_id(&mut s, 5);
    touch(&mut s, 5);
    let r = s.ok("why_hot", json!({"node": id}));
    assert_eq!(r["tier"], "hot", "{r}");
    assert!(r["heat"].as_f64().unwrap_or(0.0) > 0.5, "{r}");
    let first = r["last_access"].as_u64().expect("last_access");
    touch(&mut s, 5);
    let again = s.ok("why_hot", json!({"node": id}));
    assert!(again["last_access"].as_u64().expect("last_access") > first, "{again}");
    assert!(again["heat"].as_f64() > r["heat"].as_f64(), "{again} vs {r}");
}

#[test]
fn why_hot_is_cold_for_a_node_that_was_never_read() {
    let p = Proj::new();
    let mut s = planned(&p);
    fill(&mut s, 60);
    let hot = node_id(&mut s, 2);
    let far = node_id(&mut s, 55);
    touch(&mut s, 2);
    assert_eq!(s.ok("why_hot", json!({"node": hot}))["tier"], "hot");
    assert_eq!(s.ok("why_hot", json!({"node": far}))["tier"], "cold");
}

#[test]
fn why_hot_reports_the_neighbour_of_a_hot_node_in_the_prefetch_set() {
    let p = Proj::new();
    let mut s = planned(&p);
    fill(&mut s, 60);
    s.text(
        "write_neo4j_cypher",
        json!({"query": "MATCH (x:F {i: 1}), (y:F {i: 50}) CREATE (x)-[:R]->(y)"}),
    );
    let y = node_id(&mut s, 50);
    s.ok(
        "read_neo4j_cypher",
        json!({"query": "MATCH (x:F {i: 1})-[r:R]->(y:F {i: 50}) RETURN x, r"}),
    );
    let r = s.ok("why_hot", json!({"node": y}));
    assert_eq!(r["prefetch"], true, "{r}");
    assert_eq!(r["tier"], "cold", "{r}");
}

#[test]
fn pin_makes_the_region_hot_and_pinned_without_any_read() {
    let p = Proj::new();
    let mut s = planned(&p);
    let pinned = s.ok("pin", json!({"node": 40}));
    assert_eq!(pinned["pinned"], true, "{pinned}");
    assert_eq!(pinned["applied"], true, "{pinned}");
    assert_eq!(pinned["region"], 2, "{pinned}");
    let r = s.ok("why_hot", json!({"node": 40}));
    assert_eq!(r["pinned"], true, "{r}");
    assert_eq!(r["tier"], "hot", "{r}");
}

#[test]
fn unpin_clears_the_pin_flag() {
    let p = Proj::new();
    let mut s = planned(&p);
    s.ok("pin", json!({"node": 40}));
    let r = s.ok("unpin", json!({"node": 40}));
    assert_eq!(r["pinned"], false, "{r}");
    assert_eq!(s.ok("why_hot", json!({"node": 40}))["pinned"], false);
}

#[test]
fn unpin_of_a_node_that_was_never_pinned_is_not_an_error() {
    let p = Proj::new();
    let mut s = planned(&p);
    let r = s.ok("unpin", json!({"node": 9}));
    assert_eq!(r["pinned"], false, "{r}");
    assert_eq!(s.ok("why_hot", json!({"node": 9}))["tier"], "cold");
}

#[test]
fn pin_under_local_is_reported_as_not_applied() {
    let p = Proj::new();
    let mut s = p.session(&[], &[]);
    let r = s.ok("pin", json!({"node": 4}));
    assert_eq!(r["pinned"], false, "{r}");
    assert_eq!(r["applied"], false, "{r}");
    assert!(r["note"].as_str().unwrap_or("").contains("no plan exists"), "{r}");
}

#[test]
fn node_arguments_that_are_not_ids_are_tool_errors() {
    let p = Proj::new();
    for extra in [&[][..], &["--placement", "planned"]] {
        let mut s = p.session(extra, &[]);
        for tool in ["why_hot", "pin", "unpin"] {
            for bad in [
                json!({}),
                json!({"node": -1}),
                json!({"node": "x"}),
                json!({"node": 1.5}),
                json!({"node": [1]}),
                json!({"node": null}),
                json!({"node": 68719476736u64}),
                json!({"node": u64::MAX}),
                json!({"node": 1, "format": "xml"}),
            ] {
                let e = s.err(tool, bad.clone());
                assert!(!e.is_empty(), "{tool} {bad}");
            }
        }
        s.ok("why_hot", json!({"node": 68719476735u64}));
    }
}

#[test]
fn why_hot_accepts_a_node_id_written_as_a_string() {
    let p = Proj::new();
    let mut s = planned(&p);
    assert_eq!(s.ok("why_hot", json!({"node": "33"}))["region"], 2);
}

#[test]
fn why_hot_default_output_is_compact_and_decodes() {
    let p = Proj::new();
    let mut s = planned(&p);
    let t = s.text("why_hot", json!({"node": 3}));
    let rows = decode_table(&t).expect("decode").to_rows();
    assert_eq!(rows[0]["tier"], "cold", "{t}");
}

#[test]
fn placement_flag_rejects_unknown_modes() {
    let s = Session::start(
        &["--in-memory".to_string(), "--placement".into(), "warp".into()],
        &[],
    );
    assert!(!s.finish());
}

#[test]
fn view_default_output_is_compact_and_every_hit_carries_a_citation() {
    let p = Proj::new();
    let mut s = indexed(&p, &[]);
    let t = s.text("view", json!({"question": MIXED, "budget_tokens": 600}));
    let pack = decode_pack(&t).unwrap_or_else(|e| panic!("{e}: {t}"));
    assert!(!pack.hits.is_empty(), "{t}");
    for h in &pack.hits {
        assert!(!h.file.is_empty() && h.start >= 1 && h.end >= h.start, "{h:?}");
        assert!(h.body.is_some(), "{h:?}");
    }
}

#[test]
fn view_json_lists_hits_with_file_lines_and_chunk_id() {
    let p = Proj::new();
    let mut s = indexed(&p, &[]);
    let r = view_json(&mut s, MIXED, 600);
    let hits = r["hits"].as_array().expect("hits");
    assert!(!hits.is_empty(), "{r}");
    let text = r["text"].as_str().expect("text");
    for h in hits {
        let head = format!(
            "// {}:{}-{} #{}",
            h["file"].as_str().expect("file"),
            h["start_line"],
            h["end_line"],
            h["chunk_id"]
        );
        assert!(text.contains(&head), "missing `{head}` in {text}");
    }
    assert_eq!(r["budget_tokens"], 600);
    assert!(r["candidates"].as_u64() >= r["selected"].as_u64(), "{r}");
}

#[test]
fn view_ranks_the_matching_file_first() {
    let p = Proj::new();
    let mut s = indexed(&p, &[]);
    let r = view_json(&mut s, BACKOFF, 800);
    assert_eq!(r["hits"][0]["file"], "src/backoff.rs", "{r}");
    assert_eq!(r["hits"][0]["level"], 1, "{r}");
}

#[test]
fn view_never_exceeds_the_budget_over_200_random_budgets() {
    let p = Proj::new();
    let mut s = indexed(&p, &[]);
    let mut rng = Lcg(0x5ea1);
    let mut nonempty = 0;
    for _ in 0..200 {
        let budget = 1 + rng.below(700) as usize;
        let t = s.text("view", json!({"question": MIXED, "budget_tokens": budget}));
        assert!(proxy(&t) <= budget, "{} > {budget}: {t}", proxy(&t));
        let pack = decode_pack(&t).unwrap_or_else(|e| panic!("{e}: {t}"));
        nonempty += usize::from(!pack.hits.is_empty());
        let j = s.ok("view", json!({"question": MIXED, "budget_tokens": budget}));
        assert!(j["tokens"].as_u64().unwrap_or(u64::MAX) <= budget as u64, "{j}");
        assert!(proxy(j["text"].as_str().expect("text")) <= budget, "{j}");
    }
    assert!(nonempty >= 100, "{nonempty} of 200 budgets returned a hit");
}

#[test]
fn view_with_a_larger_budget_returns_at_least_as_many_hits() {
    let p = Proj::new();
    let mut s = indexed(&p, &[]);
    let small = view_json(&mut s, MIXED, 60)["hits"].as_array().expect("h").len();
    let large = view_json(&mut s, MIXED, 3000)["hits"].as_array().expect("h").len();
    assert!(large >= small && large >= 3, "{small} vs {large}");
}

#[test]
fn view_is_deterministic_twice_in_a_row() {
    let p = Proj::new();
    let mut s = indexed(&p, &[]);
    let args = json!({"question": MIXED, "budget_tokens": 500, "focus": "invoice"});
    let a = s.text("view", args.clone());
    let b = s.text("view", args);
    assert_eq!(a, b);
    let j = view_json(&mut s, MIXED, 500);
    assert_eq!(j, view_json(&mut s, MIXED, 500));
}

#[test]
fn view_is_deterministic_across_a_restart() {
    let p = Proj::new();
    let args = json!({"question": MIXED, "budget_tokens": 500});
    let mut s = indexed(&p, &["--db", &p.db()]);
    let before = s.text("view", args.clone());
    let before_json = view_json(&mut s, MIXED, 500);
    assert!(s.finish());
    let mut s = p.session(&["--db", &p.db()], &[]);
    assert_eq!(before, s.text("view", args));
    assert_eq!(before_json, view_json(&mut s, MIXED, 500));
}

#[test]
fn view_does_not_mark_chunks_as_delivered_in_the_session() {
    let p = Proj::new();
    let mut s = indexed(&p, &[]);
    s.text("view", json!({"question": BACKOFF, "budget_tokens": 800}));
    let t = s.text("rag_search", json!({"query": BACKOFF}));
    let pack = decode_pack(&t).unwrap_or_else(|e| panic!("{e}: {t}"));
    assert!(pack.hits.iter().all(|h| h.body.is_some()), "{t}");
}

#[test]
fn view_salient_k_defaults_to_zero() {
    let p = Proj::new();
    let mut s = indexed(&p, &[]);
    let base = json!({"question": MIXED, "budget_tokens": 900, "k": 8});
    let mut explicit = base.clone();
    explicit["salient_k"] = json!(0);
    assert_eq!(s.text("view", base.clone()), s.text("view", explicit));
    let mut salient = base;
    salient["salient_k"] = json!(3);
    let t = s.text("view", salient);
    assert!(decode_pack(&t).is_ok(), "{t}");
}

#[test]
fn view_k_limits_the_candidates() {
    let p = Proj::new();
    let mut s = indexed(&p, &[]);
    let r = s.ok(
        "view",
        json!({"question": MIXED, "budget_tokens": 3000, "k": 1}),
    );
    assert_eq!(r["candidates"], 1, "{r}");
    assert_eq!(r["hits"].as_array().map(Vec::len), Some(1), "{r}");
}

#[test]
fn view_rejects_bad_arguments_as_tool_errors() {
    let p = Proj::new();
    let mut s = indexed(&p, &[]);
    let ok = json!({"question": MIXED, "budget_tokens": 100});
    for bad in [
        json!({}),
        json!({"budget_tokens": 100}),
        json!({"question": MIXED}),
        json!({"question": "   ", "budget_tokens": 100}),
        json!({"question": 5, "budget_tokens": 100}),
        json!({"question": MIXED, "budget_tokens": 0}),
        json!({"question": MIXED, "budget_tokens": -5}),
        json!({"question": MIXED, "budget_tokens": "x"}),
        json!({"question": MIXED, "budget_tokens": 1.5}),
        json!({"question": MIXED, "budget_tokens": 100, "k": 0}),
        json!({"question": MIXED, "budget_tokens": 100, "k": 257}),
        json!({"question": MIXED, "budget_tokens": 100, "salient_k": -1}),
        json!({"question": MIXED, "budget_tokens": 100, "salient_k": 257}),
        json!({"question": MIXED, "budget_tokens": 100, "focus": 5}),
        json!({"question": MIXED, "budget_tokens": 100, "format": "xml"}),
    ] {
        s.err("view", bad.clone());
    }
    s.ok("view", ok);
}

#[test]
fn view_survives_a_huge_unicode_question_and_a_huge_budget() {
    let p = Proj::new();
    let mut s = indexed(&p, &[]);
    let long = "é\u{0}\u{1F600}".repeat(20_000);
    let t = s.text("view", json!({"question": long, "budget_tokens": 300, "focus": long}));
    assert!(proxy(&t) <= 300);
    let t = s.text(
        "view",
        json!({"question": BACKOFF, "budget_tokens": u64::MAX}),
    );
    assert!(decode_pack(&t).is_ok(), "{t}");
}

#[test]
fn view_works_under_read_only() {
    let p = Proj::new();
    let mut s = p.session(&["--db", &p.db()], &[]);
    s.ok("rag_index", json!({}));
    assert!(s.finish());
    let mut s = p.session(&["--db", &p.db(), "--read-only"], &[]);
    let r = view_json(&mut s, BACKOFF, 500);
    assert_eq!(r["hits"][0]["file"], "src/backoff.rs", "{r}");
}

#[test]
fn placement_flag_accepts_local_and_planned() {
    let p = Proj::new();
    for mode in ["local", "planned"] {
        let mut s = p.session(&["--placement", mode], &[]);
        let r = s.ok("why_hot", json!({"node": 1}));
        assert_eq!(r["mode"], mode, "{r}");
    }
    let mut s = p.session(&["--placement=planned"], &[]);
    assert_eq!(s.ok("why_hot", json!({"node": 1}))["mode"], "planned");
}