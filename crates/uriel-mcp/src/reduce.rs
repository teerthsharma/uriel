use serde_json::Value as Json;
use uriel_terse::data::{decode, encode};
use uriel_terse::{count_tokens, Value};

pub const MIN_DATA_TOKENS: usize = 300;
pub const MIN_DATA_GAIN_PERCENT: usize = 15;

const EXACT_TOOLS: &[&str] = &["rag_expand", "terse_stats", "more"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Saved {
    pub trim: usize,
    pub data: usize,
}

impl Saved {
    pub fn total(self) -> usize {
        self.trim + self.data
    }
}

pub fn trim(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut blank = 0usize;
    for line in text.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        out.push_str(line);
        out.push('\n');
    }
    let kept = out.trim_matches('\n');
    if text.ends_with('\n') && !kept.is_empty() && !text.trim_end().is_empty() {
        format!("{kept}\n")
    } else {
        kept.to_string()
    }
}

pub fn data(text: &str) -> Option<String> {
    let before = count_tokens(text);
    if before < MIN_DATA_TOKENS {
        return None;
    }
    let value = Value::parse(text).ok()?;
    let enc = encode(&value).ok()?;
    if decode(&enc).ok()? != value {
        return None;
    }
    let after = count_tokens(&enc);
    (after * 100 <= before * (100 - MIN_DATA_GAIN_PERCENT)).then_some(enc)
}

fn explicit_json(args: &Json) -> bool {
    args.get("format").and_then(Json::as_str) == Some("json")
}

pub fn apply(name: &str, args: &Json, text: String, with_data: bool) -> (String, Saved) {
    if EXACT_TOOLS.contains(&name) {
        return (text, Saved::default());
    }
    let mut saved = Saved::default();
    let mut text = text;
    let t = trim(&text);
    if t != text {
        saved.trim = count_tokens(&text).saturating_sub(count_tokens(&t));
        text = t;
    }
    if with_data && !explicit_json(args) {
        if let Some(enc) = data(&text) {
            saved.data = count_tokens(&text).saturating_sub(count_tokens(&enc));
            text = enc;
        }
    }
    (text, saved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn big_json() -> String {
        let rows: Vec<Json> = (0..80)
            .map(|i| json!({"name": format!("entity-{i}"), "type": "Thing", "observations": [], "rank": i}))
            .collect();
        json!({"entities": rows, "relations": []}).to_string()
    }

    #[test]
    fn trim_removes_trailing_blanks_and_collapses_blank_runs_only() {
        assert_eq!(trim("a  \n\n\n\nb\t\n"), "a\n\nb\n");
        assert_eq!(trim("a\nb"), "a\nb");
        assert_eq!(trim("\n\n a\n\n"), " a\n");
        assert_eq!(trim("   \n  "), "");
        assert_eq!(trim(""), "");
    }

    #[test]
    fn trim_is_idempotent_and_keeps_every_non_blank_line() {
        let src = "x  \n\n\n  y\n\t\nz \n";
        let once = trim(src);
        assert_eq!(trim(&once), once);
        let lines = |s: &str| s.lines().map(str::trim_end).filter(|l| !l.is_empty()).map(str::to_string).collect::<Vec<_>>();
        assert_eq!(lines(&once), lines(src));
    }

    #[test]
    fn data_encodes_large_json_losslessly_and_only_when_it_pays() {
        let text = big_json();
        let enc = data(&text).expect("large repetitive json shrinks");
        assert!(count_tokens(&enc) * 100 <= count_tokens(&text) * 85);
        assert_eq!(decode(&enc).unwrap(), Value::parse(&text).unwrap());
        assert!(data(r#"{"a":1}"#).is_none(), "small results are left alone");
        assert!(data(&"not json ".repeat(400)).is_none());
    }

    #[test]
    fn exact_tools_and_explicit_json_are_never_reshaped() {
        let text = big_json();
        for name in EXACT_TOOLS {
            assert_eq!(apply(name, &json!({}), format!("{text}  \n\n\n"), true).0, format!("{text}  \n\n\n"));
        }
        let (out, saved) = apply("read_graph", &json!({"format": "json"}), text.clone(), true);
        assert_eq!(out, text);
        assert_eq!(saved.data, 0);
    }

    #[test]
    fn the_pipeline_is_idempotent_and_reports_what_each_stage_saved() {
        let text = big_json();
        let (a, sa) = apply("read_graph", &json!({}), text.clone(), true);
        let (b, sb) = apply("read_graph", &json!({}), a.clone(), true);
        assert_eq!(a, b);
        assert!(sa.data > 0 && sb.total() == 0, "{sa:?} {sb:?}");
        let (c, sc) = apply("read_graph", &json!({}), "x  \n\n\n\ny".to_string(), true);
        assert_eq!(c, "x\n\ny");
        assert!(sc.trim > 0 && sc.data == 0, "{sc:?}");
    }

    #[test]
    fn data_stage_can_be_switched_off_leaving_trim_only() {
        let (out, saved) = apply("read_graph", &json!({}), big_json(), false);
        assert_eq!(out, big_json());
        assert_eq!(saved.data, 0);
    }
}
