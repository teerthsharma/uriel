mod common;

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use common::Session;
use serde_json::{json, Value};
use uriel_mcp::prefix_tree::PrefixLearner;

const WORDS: [&str; 12] = [
    "routes", "frames", "ledger", "window", "cursor", "digest", "packet", "socket", "buffer",
    "kernel", "schema", "stream",
];

fn write_corpus(dir: &Path) {
    for i in 0..30usize {
        let n = 2 + (i * 7) % 9;
        let doc = (0..n)
            .map(|k| WORDS[(i * 5 + k * 3) % WORDS.len()])
            .collect::<Vec<_>>()
            .join(" ");
        let next = (i + 1) % 30;
        let sub = if i % 2 == 0 { "core" } else { "edge" };
        fs::create_dir_all(dir.join(sub)).expect("mkdir");
        fs::write(
            dir.join(sub).join(format!("stage_{i}.rs")),
            format!(
                "/// Handle {doc} for stage {i}.\npub fn handle_stage_{i}(input: &[u8]) -> usize {{\n    handle_stage_{next}(input) + input.len()\n}}\n"
            ),
        )
        .expect("write");
    }
}

fn corpus() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("corpus");
    write_corpus(dir.path());
    dir
}

fn started(flag: &str, corpus: &Path) -> Session {
    let args: Vec<String> = ["--in-memory"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let mut s = Session::start(&args, &[("URIEL_STABLE_PREFIX", flag)]);
    s.text("rag_index", json!({"path": corpus.to_string_lossy()}));
    s
}

const QUERIES: [&str; 4] = [
    "handle routes frames for stage",
    "ledger window cursor digest",
    "socket buffer kernel schema",
    "completely unrelated zzzq words",
];

fn view_json(s: &mut Session, question: &str) -> String {
    s.text(
        "view",
        json!({"question": question, "budget_tokens": 600, "format": "json"}),
    )
}

fn stable_bytes(text: &str) -> usize {
    let v: Value = serde_json::from_str(text).expect("json");
    usize::try_from(v["stable_prefix_bytes"].as_u64().expect("stable_prefix_bytes")).expect("fits")
}

#[test]
fn json_view_and_soul_lead_with_the_scaffold_bound_preamble() {
    let dir = corpus();
    let mut s = started("on", dir.path());
    let view = view_json(&mut s, QUERIES[0]);
    let soul = s.text("soul", json!({"format": "json", "budget_tokens": 800}));
    for text in [&view, &soul] {
        assert!(text.starts_with("{\"prefix_id\":\""), "{}", &text[..60.min(text.len())]);
        let v: Value = serde_json::from_str(text).expect("json");
        let id = v["prefix_id"].as_str().expect("prefix_id");
        assert_eq!(id.len(), 64);
        assert!(v["preamble"].as_str().expect("preamble").contains(id));
    }
    let n = stable_bytes(&view);
    assert_eq!(n, stable_bytes(&soul));
    assert_eq!(view.as_bytes()[..n], soul.as_bytes()[..n]);
    assert!(view.is_char_boundary(n) && view.as_bytes()[n - 1] == b',');
}

#[test]
fn two_queries_on_one_index_share_an_identical_stable_prefix() {
    let dir = corpus();
    let mut s = started("on", dir.path());
    let responses: Vec<String> = QUERIES.iter().map(|q| view_json(&mut s, q)).collect();
    let n = stable_bytes(&responses[0]);
    assert!(n > 64);
    for r in &responses {
        assert_eq!(stable_bytes(r), n);
        assert_eq!(r.as_bytes()[..n], responses[0].as_bytes()[..n]);
    }
    assert_ne!(responses[0], responses[1]);
}

#[test]
fn the_stable_prefix_is_byte_identical_across_twenty_server_restarts() {
    let dir = corpus();
    let heads: BTreeSet<Vec<u8>> = (0..20)
        .map(|i| {
            let mut s = started("on", dir.path());
            let r = view_json(&mut s, QUERIES[i % QUERIES.len()]);
            let n = stable_bytes(&r);
            r.as_bytes()[..n].to_vec()
        })
        .collect();
    assert_eq!(heads.len(), 1);
}

#[test]
fn the_prefix_learner_finds_at_least_the_preamble_in_served_responses() {
    let dir = corpus();
    let mut s = started("on", dir.path());
    let mut learner = PrefixLearner::new(0.95);
    let mut n = 0;
    for q in QUERIES {
        let r = view_json(&mut s, q);
        n = stable_bytes(&r);
        let id = serde_json::from_str::<Value>(&r).expect("json")["prefix_id"]
            .as_str()
            .expect("id")
            .to_string();
        assert!(!learner.observe(&id, r.as_bytes()));
    }
    assert!(learner.stable_len() >= n, "{} < {n}", learner.stable_len());
}

#[test]
fn indexing_new_content_changes_the_prefix_id_and_reopens_the_learner() {
    let dir = corpus();
    let mut s = started("on", dir.path());
    let id_of = |r: &str| serde_json::from_str::<Value>(r).expect("json")["prefix_id"].as_str().expect("id").to_string();
    let before = view_json(&mut s, QUERIES[0]);
    fs::write(
        dir.path().join("extra.rs"),
        "/// Brand new vocabulary quasar nebula pulsar.\npub fn extra() -> usize {\n    1\n}\n",
    )
    .expect("write");
    s.text("rag_index", json!({"path": dir.path().to_string_lossy()}));
    let after = view_json(&mut s, QUERIES[0]);
    assert_ne!(id_of(&before), id_of(&after));
    let mut learner = PrefixLearner::new(0.95);
    assert!(!learner.observe(&id_of(&before), before.as_bytes()));
    assert!(learner.observe(&id_of(&after), after.as_bytes()));
    assert_eq!(learner.len(), 1);
}

#[test]
fn compact_responses_are_byte_identical_with_the_prefix_on_and_off() {
    let dir = corpus();
    let mut on = started("on", dir.path());
    let mut off = started("off", dir.path());
    for q in QUERIES {
        let args = json!({"question": q, "budget_tokens": 600, "format": "compact"});
        assert_eq!(on.text("view", args.clone()), off.text("view", args));
    }
    let args = json!({"format": "compact", "budget_tokens": 800});
    assert_eq!(on.text("soul", args.clone()), off.text("soul", args));
}

#[test]
fn json_responses_are_the_plain_serialisation_with_the_prefix_off() {
    let dir = corpus();
    let mut off = started("off", dir.path());
    let view = view_json(&mut off, QUERIES[0]);
    let soul = off.text("soul", json!({"format": "json", "budget_tokens": 800}));
    for text in [view, soul] {
        let v: Value = serde_json::from_str(&text).expect("json");
        assert!(v.get("prefix_id").is_none() && v.get("stable_prefix_bytes").is_none());
        assert_eq!(text, v.to_string());
    }
}

#[test]
fn the_variable_content_is_unchanged_by_the_prefix() {
    let dir = corpus();
    let mut on = started("on", dir.path());
    let mut off = started("off", dir.path());
    for q in QUERIES {
        let mut a: Value = serde_json::from_str(&view_json(&mut on, q)).expect("on");
        let b: Value = serde_json::from_str(&view_json(&mut off, q)).expect("off");
        let obj = a.as_object_mut().expect("object");
        for key in ["prefix_id", "preamble", "stable_prefix_bytes"] {
            obj.remove(key);
        }
        assert_eq!(a, b);
    }
}
