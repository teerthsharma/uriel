mod common;

use uriel_page::{demo, digest, render, render_json};

const SINK: &str = r##"{
  "title": "Sink", "subtitle": "all blocks", "chart_id": "K-1", "ward": "W", "attending": "A",
  "theme": "dark",
  "nav": [{"label": "Home", "href": "index.html", "current": true}, {"label": "Ext", "href": "https://example.org/x?a=1&b=2"}],
  "blocks": [
    {"type": "vitals", "tiles": [{"label": "L", "value": "1", "unit": "u", "state": "alert", "hint": "h"}]},
    {"type": "table", "caption": "T", "columns": ["a", "b", "c"], "rows": [["x", {"text": "y", "chip": "done"}, {"text": "z", "href": "#t"}]]},
    {"type": "checklist", "title": "C", "items": [{"text": "i", "done": true, "note": "n"}, {"text": "j"}]},
    {"type": "progress", "label": "P", "done": 1, "total": 3},
    {"type": "timeline", "title": "TL", "entries": [{"at": "9", "who": "w", "text": "t", "state": "watch"}]},
    {"type": "notes", "title": "N", "paragraphs": ["p1", "p2"]},
    {"type": "section", "title": "S", "blocks": [{"type": "columns", "blocks": [{"type": "rule"}, {"type": "notes", "title": "Deep", "paragraphs": []}]}]}
  ],
  "footer": "f"
}"##;

const SINK_SHUFFLED: &str = r##"{
  "footer": "f",
  "blocks": [
    {"tiles": [{"hint": "h", "state": "alert", "unit": "u", "value": "1", "label": "L"}], "type": "vitals"},
    {"rows": [["x", {"chip": "done", "text": "y"}, {"href": "#t", "text": "z"}]], "columns": ["a", "b", "c"], "caption": "T", "type": "table"},
    {"items": [{"note": "n", "done": true, "text": "i"}, {"text": "j"}], "title": "C", "type": "checklist"},
    {"total": 3, "done": 1, "label": "P", "type": "progress"},
    {"entries": [{"state": "watch", "text": "t", "who": "w", "at": "9"}], "title": "TL", "type": "timeline"},
    {"paragraphs": ["p1", "p2"], "title": "N", "type": "notes"},
    {"blocks": [{"blocks": [{"type": "rule"}, {"paragraphs": [], "title": "Deep", "type": "notes"}], "type": "columns"}], "title": "S", "type": "section"}
  ],
  "nav": [{"current": true, "href": "index.html", "label": "Home"}, {"href": "https://example.org/x?a=1&b=2", "label": "Ext"}],
  "theme": "dark", "attending": "A", "ward": "W", "chart_id": "K-1", "subtitle": "all blocks", "title": "Sink"
}"##;

#[test]
fn rendering_twice_gives_identical_bytes() {
    assert_eq!(render(&demo::index()), render(&demo::index()));
}

#[test]
fn shuffled_json_key_order_gives_identical_bytes() {
    assert_eq!(
        render_json(SINK).unwrap(),
        render_json(SINK_SHUFFLED).unwrap()
    );
}

#[test]
fn golden_digest_index() {
    assert_eq!(
        digest(&render(&demo::index())),
        include_str!("golden/index.sha256").trim()
    );
}

#[test]
fn golden_digest_department_research() {
    assert_eq!(
        digest(&render(&demo::department_research())),
        include_str!("golden/department-research.sha256").trim()
    );
}

#[test]
fn golden_digest_kitchen_sink_json() {
    assert_eq!(
        digest(&render_json(SINK).unwrap()),
        include_str!("golden/sink.sha256").trim()
    );
}

#[test]
fn kitchen_sink_passes_a11y_checker() {
    common::a11y(&render_json(SINK).unwrap()).unwrap();
}

#[test]
fn digest_is_sha256_hex() {
    assert_eq!(
        digest(""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
}

#[test]
fn dark_theme_pins_the_data_attribute_and_scheme() {
    let h = render_json(SINK).unwrap();
    assert!(h.contains("<html lang=\"en\" data-theme=\"dark\">"));
    assert!(h.contains("name=\"color-scheme\" content=\"dark\""));
}
