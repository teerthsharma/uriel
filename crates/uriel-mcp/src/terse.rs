use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::json;
use uriel_rag::{Hit, SoulSection, MEMORY_PREFIX};
use uriel_terse::data::{decode, encode};
use uriel_terse::{compact, estimate_tokens, Level, Report, Value};

use crate::config::Terse;
use crate::prefix::is_led;
use crate::tools::{no_args, Context, ToolDef, ToolResult};

pub const STATS: &str = "terse_stats";

pub const INSTRUCTION: &str = "Terse JSON results: indent nests; `k: v`; `k[n]: a,b`; `k[n]{c1,c2=const,c3^prefix}:` then n rows (const columns omitted, prefix prepended); `$x=key` lines first name keys; `- ` list items; a leading `!` means plain JSON.";

const RAW_TEXT_TOOLS: &[&str] = &["rag_expand", STATS];

const PROSE_FILES: &[&str] = &[".md", ".markdown", ".txt", ".rst"];

#[derive(Debug, Default)]
pub struct Meter {
    before: AtomicU64,
    after: AtomicU64,
}

impl Meter {
    pub fn add(&self, before: &str, after: &str) {
        self.add_pair((estimate_tokens(before), estimate_tokens(after)));
    }

    pub fn add_pair(&self, (before, after): (usize, usize)) {
        self.before.fetch_add(before as u64, Ordering::Relaxed);
        self.after.fetch_add(after as u64, Ordering::Relaxed);
    }

    pub fn totals(&self) -> (u64, u64) {
        (
            self.before.load(Ordering::Relaxed),
            self.after.load(Ordering::Relaxed),
        )
    }
}

pub const TOOLS: &[ToolDef] = &[ToolDef {
    name: STATS,
    title: "Terse savings",
    description: "Report the tokens the terse encoding saved in this session: terse_before_tokens, terse_after_tokens and saved_ratio, over the results it shortened or passed through unchanged.",
    writes: false,
    schema: no_args,
}];

fn pack(value: &Value, text: &str) -> String {
    match encode(value) {
        Ok(enc)
            if estimate_tokens(&enc) < estimate_tokens(text)
                && decode(&enc).is_ok_and(|back| back == *value) =>
        {
            enc
        }
        _ => text.to_string(),
    }
}

pub fn data_text(text: &str) -> String {
    match Value::parse(text) {
        Ok(value) => pack(&value, text),
        Err(_) => text.to_string(),
    }
}

pub fn shape(ctx: &Context, name: &str, text: String) -> String {
    if ctx.terse == Terse::Off || RAW_TEXT_TOOLS.contains(&name) || is_led(&text) {
        return text;
    }
    let Ok(value) = Value::parse(&text) else {
        return text;
    };
    let out = pack(&value, &text);
    ctx.meter.add(&text, &out);
    out
}

pub fn lite_text(ctx: &Context, text: &str) -> String {
    if ctx.terse != Terse::Prose {
        return text.to_string();
    }
    let out = compact(text, Level::Lite);
    if out.is_empty() || out.chars().count() >= text.chars().count() {
        return text.to_string();
    }
    out
}

pub fn remembered(ctx: &Context, mut hits: Vec<Hit>) -> Vec<Hit> {
    if ctx.terse != Terse::Prose {
        return hits;
    }
    for hit in hits.iter_mut().filter(|h| h.file.starts_with(MEMORY_PREFIX)) {
        let out = lite_text(ctx, &hit.memory);
        ctx.meter.add(&hit.memory, &out);
        hit.memory = out;
    }
    hits
}

fn is_prose_file(file: &str) -> bool {
    let lower = file.to_ascii_lowercase();
    PROSE_FILES.iter().any(|ext| lower.ends_with(ext))
}

pub fn prose_sections(ctx: &Context, mut sections: Vec<SoulSection>) -> (Vec<SoulSection>, bool) {
    if ctx.terse != Terse::Prose {
        return (sections, false);
    }
    let mut changed = false;
    for section in sections.iter_mut().filter(|s| is_prose_file(&s.file)) {
        let out = lite_text(ctx, &section.text);
        ctx.meter.add(&section.text, &out);
        changed |= out != section.text;
        section.text = out;
    }
    (sections, changed)
}

pub fn stats(ctx: &Context) -> ToolResult {
    let (before, after) = ctx.meter.totals();
    let report = Report::from_counts(before as usize, after as usize);
    Ok(json!({
        "terse": ctx.terse.name(),
        "terse_before_tokens": report.before,
        "terse_after_tokens": report.after,
        "saved_ratio": report.saved_ratio,
    })
    .to_string())
}

pub fn fingerprint_key(mode: &str, terse: Terse) -> String {
    match terse {
        Terse::Off => mode.to_string(),
        other => format!("{mode}+terse:{}", other.name()),
    }
}

pub fn instructions(base: &str, terse: Terse) -> String {
    match terse {
        Terse::Off => base.to_string(),
        _ => format!("{base} {INSTRUCTION}"),
    }
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
            (1, _) => Json::Bool(lcg(seed).is_multiple_of(2)),
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
    fn a_value_the_encoder_refuses_falls_back_to_the_text() {
        let mut deep = Value::Null;
        for _ in 0..1000 {
            deep = Value::Array(vec![deep]);
        }
        assert_eq!(pack(&deep, "TEXT"), "TEXT");
        assert_eq!(pack(&Value::Number("0x10".to_string()), "0x10"), "0x10");
        assert_eq!(pack(&Value::Object(vec![("k".to_string(), Value::Number("1.".to_string()))]), "RAW"), "RAW");
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
