use std::sync::atomic::AtomicU64;

use serde_json::json;
use uriel_rag::SoulSection;

use crate::config::Terse;
use crate::tools::{no_args, Context, ToolDef, ToolResult};

pub const STATS: &str = "terse_stats";

pub const INSTRUCTION: &str = "";

#[derive(Debug, Default)]
pub struct Meter {
    before: AtomicU64,
    after: AtomicU64,
}

impl Meter {
    pub fn add(&self, _before: &str, _after: &str) {
        let _ = (&self.before, &self.after);
    }

    pub fn totals(&self) -> (u64, u64) {
        (0, 0)
    }
}

pub const TOOLS: &[ToolDef] = &[ToolDef {
    name: STATS,
    title: "Terse savings",
    description: "Report the tokens the terse encoding saved in this session: terse_before_tokens, terse_after_tokens and saved_ratio.",
    writes: false,
    schema: no_args,
}];

pub fn data_text(text: &str) -> String {
    text.to_string()
}

pub fn shape(_ctx: &Context, _name: &str, text: String) -> String {
    text
}

pub fn lite(_ctx: &Context, text: &str) -> String {
    text.to_string()
}

pub fn prose_sections(_ctx: &Context, sections: Vec<SoulSection>) -> (Vec<SoulSection>, bool) {
    (sections, false)
}

pub fn stats(ctx: &Context) -> ToolResult {
    Ok(json!({
        "terse": ctx.terse.name(),
        "terse_before_tokens": 0,
        "terse_after_tokens": 0,
        "saved_ratio": 0.0,
    })
    .to_string())
}

pub fn fingerprint_key(mode: &str, _terse: Terse) -> String {
    mode.to_string()
}

pub fn instructions(base: &str, _terse: Terse) -> String {
    base.to_string()
}

#[cfg(test)]
mod tests {
    use serde_json::Value as Json;
    use uriel_terse::data::decode;
    use uriel_terse::{estimate_tokens, Value};

    use super::*;

    fn lcg(seed: &mut u64) -> u64 {
        *seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        *seed >> 33
    }

    fn word(seed: &mut u64) -> String {
        const BITS: &[&str] = &[
            "a", "b,c", "x:y", "$z", "!w", "", " pad ", "true", "12", "1e5", "-", "q\"r", "\u{e9}",
            "k=v", "n^m", "[1]", "{}",
        ];
        BITS[(lcg(seed) as usize) % BITS.len()].to_string()
    }

    fn gen(seed: &mut u64, depth: usize) -> Json {
        match (lcg(seed) % 7, depth) {
            (0, _) => Json::Null,
            (1, _) => Json::Bool(lcg(seed) % 2 == 0),
            (2, _) => json!(lcg(seed) as i64 - 1000),
            (3, _) => Json::String(word(seed)),
            (4 | 5, 0..=3) => Json::Array((0..lcg(seed) % 5).map(|_| gen(seed, depth + 1)).collect()),
            (_, 0..=3) => Json::Object(
                (0..lcg(seed) % 5)
                    .map(|_| (word(seed), gen(seed, depth + 1)))
                    .collect(),
            ),
            _ => Json::Null,
        }
    }

    #[test]
    fn non_json_text_is_returned_unchanged() {
        for t in ["", "#2\na\tb", "// src/x.rs:1-2 #3\nfn x() {}", "not json", "{\"a\":", "[1,]"] {
            assert_eq!(data_text(t), t);
        }
    }

    #[test]
    fn a_json_table_shrinks_and_decodes_back_to_the_same_value() {
        let rows: Vec<Json> = (0..20)
            .map(|i| json!({"file": format!("crates/uriel-rag/src/m{i}.rs"), "line": i, "kind": "fn"}))
            .collect();
        let text = Json::Array(rows).to_string();
        let out = data_text(&text);
        assert!(estimate_tokens(&out) < estimate_tokens(&text), "{out}");
        assert_eq!(decode(&out).expect("decode"), Value::parse(&text).expect("parse"));
    }

    #[test]
    fn pretty_printed_json_is_accepted() {
        let text = serde_json::to_string_pretty(&json!({"a": [1, 2, 3], "b": {"c": "d"}})).unwrap();
        let out = data_text(&text);
        assert!(out.len() < text.len());
        assert_eq!(decode(&out).expect("decode"), Value::parse(&text).expect("parse"));
    }

    #[test]
    fn json_that_does_not_shrink_is_returned_unchanged() {
        for t in ["\"x\"", "5", "null", "true", "[]", "{}", "[1]", "{\"a\":1}"] {
            let out = data_text(t);
            assert!(out == t || estimate_tokens(&out) < estimate_tokens(t), "{t} -> {out}");
        }
        assert_eq!(data_text("\"x\""), "\"x\"");
        assert_eq!(data_text("5"), "5");
    }

    #[test]
    fn hostile_depth_and_size_never_panic_and_pass_through() {
        let deep = format!("{}{}", "[".repeat(200_000), "]".repeat(200_000));
        assert_eq!(data_text(&deep), deep);
        let nested = format!("{}1{}", "[".repeat(150), "]".repeat(150));
        assert_eq!(data_text(&nested), nested);
        let wide = format!("[{}]", vec!["\"ab\""; 50_000].join(","));
        let out = data_text(&wide);
        assert!(estimate_tokens(&out) < estimate_tokens(&wide));
        assert_eq!(decode(&out).expect("decode"), Value::parse(&wide).expect("parse"));
        let huge = "[1e999999999999999999999999]";
        let out = data_text(huge);
        let back = decode(&out).ok().or_else(|| Value::parse(&out).ok());
        assert_eq!(back, Value::parse(huge).ok());
    }

    #[test]
    fn two_thousand_random_documents_round_trip_or_pass_through() {
        let mut seed = 0x9e3779b97f4a7c15u64;
        let mut shrank = 0;
        for _ in 0..2000 {
            let doc = gen(&mut seed, 0);
            for text in [doc.to_string(), serde_json::to_string_pretty(&doc).unwrap()] {
                let out = data_text(&text);
                if out == text {
                    continue;
                }
                shrank += 1;
                assert!(estimate_tokens(&out) < estimate_tokens(&text), "{text} -> {out}");
                assert_eq!(
                    decode(&out).expect("decode"),
                    Value::parse(&text).expect("parse"),
                    "{text} -> {out}"
                );
            }
        }
        assert!(shrank > 500, "{shrank}");
    }

    #[test]
    fn the_meter_adds_token_estimates_of_both_sides() {
        let m = Meter::default();
        m.add("abcdefgh", "abcd");
        m.add("abcd", "ab");
        assert_eq!(m.totals(), (3, 2));
    }
}
