use std::path::PathBuf;

use serde_json::{json, Value};

mod common;

const BUDGETS: [u64; 3] = [300, 800, 4000];
const K: u64 = 64;

#[test]
#[ignore]
fn dump_compact_and_json_outputs_for_the_frozen_queries() {
    let out = PathBuf::from(std::env::var("TOKEN_DUMP_OUT").expect("TOKEN_DUMP_OUT"));
    std::fs::create_dir_all(&out).expect("out");
    let mut f = common::frozen();
    let put = |name: String, text: &str| std::fs::write(out.join(name), text).expect("write");
    for b in BUDGETS {
        for (i, q) in f.queries.iter().enumerate() {
            let search = |extra: Value| {
                let mut v = json!({"query": q, "budget_tokens": b, "k": K, "fresh": true});
                for (key, x) in extra.as_object().expect("object") {
                    v[key] = x.clone();
                }
                v
            };
            for tool in ["rag_search", "seal_recall"] {
                put(format!("{tool}-{b}-{i:03}.c"), &f.session.text(tool, search(json!({}))));
                put(
                    format!("{tool}-{b}-{i:03}.j"),
                    &f.session.text(tool, search(json!({"format": "json", "ladder": true}))),
                );
            }
            let v = json!({"question": q, "budget_tokens": b, "k": K});
            put(format!("view-{b}-{i:03}.c"), &f.session.text("view", v.clone()));
            let mut vj = v;
            vj["format"] = json!("json");
            put(format!("view-{b}-{i:03}.j"), &f.session.text("view", vj));
        }
        for (j, focus) in std::iter::once(String::new()).chain(f.crates.clone()).enumerate() {
            let mut a = json!({"budget_tokens": b});
            if !focus.is_empty() {
                a["focus"] = json!(focus);
            }
            put(format!("soul-{b}-{j:03}.c"), &f.session.text("soul", a.clone()));
            a["format"] = json!("json");
            put(format!("soul-{b}-{j:03}.j"), &f.session.text("soul", a));
        }
    }
    f.session.text("session_reset", json!({}));
    for (i, q) in f.queries.iter().enumerate() {
        put(
            format!("session-800-{i:03}.c"),
            &f.session.text("rag_search", json!({"query": q, "budget_tokens": 800, "k": K})),
        );
    }
}
