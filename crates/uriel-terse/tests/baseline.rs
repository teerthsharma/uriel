mod common;

use common::SplitMix;
use uriel_terse::data::{decode, encode};
use uriel_terse::{compact, Level, Report, Value};

const README: &str = include_str!("../../../README.md");
const PLAN: &str = include_str!("../../../PLAN.md");
const DESIGN: &str = include_str!("../../../docs/DESIGN.md");
const SEAL_DB: &str = include_str!("../../../docs/SEAL-DB-V2.md");
const BRAND: &str = include_str!("../../../docs/BRAND.md");
const NOTES: &str = include_str!("fixtures/notes.txt");
const PATHS: &str = include_str!("fixtures/paths.txt");

fn paragraphs(doc: &str, want: usize) -> Vec<String> {
    let all: Vec<String> = doc
        .replace("\r\n", "\n")
        .split("\n\n")
        .map(|p| p.trim().to_string())
        .filter(|p| {
            p.len() >= 160
                && p.len() <= 1500
                && !p.starts_with(['|', '`', '<', '!', '#', '['])
                && !p.contains("```")
        })
        .collect();
    if all.len() <= want {
        return all;
    }
    (0..want).map(|i| all[i * all.len() / want].clone()).collect()
}

struct Sample {
    class: &'static str,
    text: String,
}

fn samples() -> Vec<Sample> {
    let mut out = Vec::new();
    let mut add = |class: &'static str, texts: Vec<String>| {
        out.extend(texts.into_iter().map(|text| Sample { class, text }));
    };
    add("README", paragraphs(README, 16));
    add("PLAN", paragraphs(PLAN, 10));
    let mut docs = paragraphs(DESIGN, 4);
    docs.extend(paragraphs(SEAL_DB, 4));
    docs.extend(paragraphs(BRAND, 2));
    add("docs", docs);
    add(
        "notes (synthetic)",
        NOTES.split("\n\n").map(|n| n.trim().to_string()).collect(),
    );
    out
}

fn num(s: impl ToString) -> Value {
    Value::Number(s.to_string())
}

fn string(s: impl ToString) -> Value {
    Value::String(s.to_string())
}

fn obj(fields: Vec<(&str, Value)>) -> Value {
    Value::Object(fields.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}

fn gist(i: usize) -> String {
    let paras = paragraphs(README, 40);
    let p = &paras[i % paras.len()];
    p.chars().take(180).collect::<String>().replace('\n', " ")
}

fn hits(rng: &mut SplitMix, n: usize, view: bool) -> (Vec<Value>, String) {
    let paths: Vec<&str> = PATHS.lines().collect();
    let mut score = 0.95f32;
    let mut text = String::new();
    let mut list = Vec::new();
    for i in 0..n {
        score -= (rng.below(400) as f32) / 10_000.0 + 0.001;
        let file = paths[rng.below(paths.len())];
        let start = 1 + rng.below(900);
        let end = start + 5 + rng.below(60);
        let id = 1000 + rng.below(90_000);
        text.push_str(&format!("// {file}:{start}-{end} #{id}\n{}\n", gist(i)));
        let mut fields = vec![
            ("chunk_id", num(id)),
            ("file", string(file)),
            ("start_line", num(start)),
            ("end_line", num(end)),
            ("score", num(score)),
        ];
        if view {
            fields.push(("level", num(1 + (i.min(4) + 1) / 2)));
        } else {
            fields.push(("via", string(if rng.below(6) == 0 { "graph" } else { "direct" })));
            fields.push(("remembered", Value::Bool(false)));
        }
        list.push(obj(fields));
    }
    (list, text)
}

fn rag_search(seed: u64, n: usize, with_text: bool) -> Value {
    let mut rng = SplitMix(seed);
    let (list, text) = hits(&mut rng, n, false);
    let tokens = text.len().div_ceil(4);
    let mut fields = Vec::new();
    if with_text {
        fields.push(("text", string(text)));
    }
    fields.push(("hits", Value::Array(list)));
    fields.push(("tokens", num(tokens)));
    fields.push(("budget_tokens", num(2000)));
    fields.push(("omitted", num(rng.below(30))));
    obj(fields)
}

fn view(seed: u64, n: usize, islands: bool) -> Value {
    let mut rng = SplitMix(seed);
    let (list, text) = hits(&mut rng, n, true);
    let tokens = text.len().div_ceil(4);
    let mut fields = vec![
        ("text", string(text)),
        ("hits", Value::Array(list)),
        ("tokens", num(tokens)),
        ("budget_tokens", num(4000)),
        ("omitted", num(rng.below(20))),
        ("candidates", num(n + 9)),
        ("selected", num(n)),
    ];
    if islands {
        let sections = (0..3)
            .map(|label| {
                obj(vec![
                    ("label", num(label)),
                    ("candidates", num(4 + rng.below(8))),
                    ("budget_tokens", num(1000 + rng.below(500))),
                    ("selected", num(3 + rng.below(4))),
                    ("kept", num(2 + rng.below(4))),
                    ("tokens", num(300 + rng.below(700))),
                ])
            })
            .collect();
        fields.push(("islands", Value::Array(sections)));
    }
    obj(fields)
}

fn fixtures() -> Vec<(String, Value)> {
    let mut out = Vec::new();
    for (i, n) in [3usize, 8, 20, 50].into_iter().enumerate() {
        out.push((format!("rag_search n={n} +text"), rag_search(10 + i as u64, n, true)));
    }
    for (i, n) in [8usize, 20, 50].into_iter().enumerate() {
        out.push((format!("rag_search n={n} hits only"), rag_search(20 + i as u64, n, false)));
    }
    for (i, n) in [4usize, 12, 30].into_iter().enumerate() {
        out.push((format!("view n={n} +text"), view(30 + i as u64, n, false)));
    }
    out.push(("view n=12 +islands".to_string(), view(40, 12, true)));
    out
}

fn mean(xs: &[f64]) -> f64 {
    xs.iter().sum::<f64>() / xs.len() as f64
}

fn min(xs: &[f64]) -> f64 {
    xs.iter().cloned().fold(f64::INFINITY, f64::min)
}

#[test]
fn prose_baseline_table() {
    let all = samples();
    assert!(all.len() >= 40, "only {} samples", all.len());
    if let Ok(dir) = std::env::var("URIEL_TERSE_DUMP") {
        for (i, s) in all.iter().enumerate() {
            std::fs::write(format!("{dir}/prose_{i:02}.txt"), &s.text).unwrap();
        }
    }
    let classes = ["README", "PLAN", "docs", "notes (synthetic)"];
    let levels = [Level::Lite, Level::Full, Level::Ultra];
    println!("prose: {} samples, saved_ratio = 1 - after/before, tokens = ceil(chars/4)", all.len());
    println!("{:<20} {:>3} {:>16} {:>16} {:>16}", "class", "n", "Lite mean/worst", "Full mean/worst", "Ultra mean/worst");
    let mut totals: Vec<Vec<f64>> = vec![Vec::new(); 3];
    for class in classes.iter().map(|c| *c).chain(["ALL"]) {
        let rows: Vec<&Sample> = all
            .iter()
            .filter(|s| class == "ALL" || s.class == class)
            .collect();
        let mut cells = Vec::new();
        for (li, level) in levels.iter().enumerate() {
            let ratios: Vec<f64> = rows
                .iter()
                .map(|s| Report::measure(&s.text, &compact(&s.text, *level)).saved_ratio)
                .collect();
            assert!(min(&ratios) >= 0.0, "{class} {level:?} made text longer");
            if class == "ALL" {
                totals[li] = ratios.clone();
            }
            cells.push(format!("{:>6.1}% /{:>5.1}%", mean(&ratios) * 100.0, min(&ratios) * 100.0));
        }
        println!("{:<20} {:>3} {:>16} {:>16} {:>16}", class, rows.len(), cells[0], cells[1], cells[2]);
    }
    assert!(mean(&totals[0]) <= mean(&totals[1]) && mean(&totals[1]) <= mean(&totals[2]));
    assert!(mean(&totals[1]) > 0.02, "Full saves too little: {}", mean(&totals[1]));
}

#[test]
fn data_baseline_table() {
    let dump = std::env::var("URIEL_TERSE_DUMP").ok();
    println!("data: before = minified JSON, after = data::encode, tokens = ceil(chars/4)");
    println!("{:<28} {:>7} {:>7} {:>8}", "fixture", "before", "after", "saved");
    let mut ratios = Vec::new();
    let mut hit_only = Vec::new();
    for (name, value) in fixtures() {
        let json = value.to_json();
        let enc = encode(&value).unwrap();
        assert_eq!(decode(&enc).unwrap(), value, "{name}");
        let r = Report::measure(&json, &enc);
        println!("{:<28} {:>7} {:>7} {:>7.1}%", name, r.before, r.after, r.saved_ratio * 100.0);
        if let Some(dir) = &dump {
            let slug: String = name.chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).collect();
            std::fs::write(format!("{dir}/json_{slug}.json"), &json).unwrap();
            std::fs::write(format!("{dir}/json_{slug}.enc"), &enc).unwrap();
        }
        assert!(r.saved_ratio > -0.01, "{name}");
        if name.contains("hits only") || name.contains("islands") {
            hit_only.push(r.saved_ratio);
        }
        ratios.push(r.saved_ratio);
    }
    println!("mean {:.1}%  worst {:.1}%  hits-only mean {:.1}%", mean(&ratios) * 100.0, min(&ratios) * 100.0, mean(&hit_only) * 100.0);
    assert!(mean(&hit_only) > 0.10, "{}", mean(&hit_only));
}
