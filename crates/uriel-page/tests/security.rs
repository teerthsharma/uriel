mod common;

use common::{a11y, hrefs, tags, XSS};
use uriel_page::*;

fn hostile(p: &str) -> Page {
    let s = || p.to_string();
    Page {
        title: s(),
        subtitle: Some(s()),
        chart_id: Some(s()),
        ward: Some(s()),
        attending: Some(s()),
        nav: vec![NavLink {
            label: s(),
            href: "#ok".into(),
            current: true,
        }],
        blocks: vec![
            Block::Vitals {
                tiles: vec![Tile {
                    label: s(),
                    value: s(),
                    unit: Some(s()),
                    state: Some(State::Alert),
                    hint: Some(s()),
                }],
            },
            Block::Table {
                caption: s(),
                columns: vec![s(), s(), s()],
                rows: vec![vec![
                    Cell::text(s()),
                    Cell::chip(s(), Chip::Done),
                    Cell::link(s(), "#a"),
                ]],
            },
            Block::Checklist {
                title: s(),
                items: vec![Item {
                    text: s(),
                    done: true,
                    note: Some(s()),
                }],
            },
            Block::Progress {
                label: s(),
                done: 1,
                total: 2,
            },
            Block::Timeline {
                title: s(),
                entries: vec![Entry {
                    at: s(),
                    who: s(),
                    text: s(),
                    state: Some(State::Watch),
                }],
            },
            Block::Notes {
                title: s(),
                paragraphs: vec![s()],
            },
            Block::Section {
                title: s(),
                blocks: vec![],
            },
        ],
        footer: Some(s()),
        ..Page::default()
    }
}

#[test]
fn xss_payloads_never_appear_unescaped_in_any_field() {
    for p in XSS {
        let page = hostile(p);
        validate(&page).unwrap();
        let html = render(&page);
        if p.contains('<') || p.contains('"') || p.contains('\'') {
            assert!(!html.contains(p), "raw payload leaked: {p}");
        }
        a11y(&html).unwrap_or_else(|e| panic!("{p}: {e}"));
        assert!(!html.to_ascii_lowercase().contains("<script"));
        for t in tags(&html) {
            assert!(
                t.names_in_order.iter().all(|k| !k.starts_with("on")),
                "{p}: on* attribute on <{}>",
                t.name
            );
        }
    }
}

#[test]
fn nul_and_control_characters_become_replacement_characters() {
    let mut out = String::new();
    escape("a\u{0}b\u{7}c", &mut out);
    assert_eq!(out, "a\u{FFFD}b\u{FFFD}c");
}

#[test]
fn href_policy_allows_relative_anchor_http_https_mailto() {
    for h in [
        "index.html",
        "../a/b.html?x=1#y",
        "./a",
        "/abs/path",
        "#top",
        "http://example.org",
        "HTTPS://example.org/a",
        "mailto:a@b.co",
        "a/b:c",
    ] {
        assert!(href_allowed(h), "{h}");
    }
}

#[test]
fn href_policy_rejects_script_data_and_obfuscations() {
    for h in [
        "",
        "javascript:alert(1)",
        "JaVaScRiPt:alert(1)",
        "java\nscript:alert(1)",
        "java\tscript:alert(1)",
        " javascript:alert(1)",
        "\u{1}javascript:alert(1)",
        "data:text/html,<script>",
        "vbscript:x",
        "file:///etc/passwd",
        "ftp://x",
        "//evil.example/x",
        "\\\\evil\\share",
        "http://",
        "mailto:",
        "a:b",
        "tel:123",
    ] {
        assert!(!href_allowed(h), "{h:?}");
    }
}

#[test]
fn validation_rejects_unsafe_hrefs_in_nav_and_cells() {
    let mut p = Page::new("x");
    p.nav.push(NavLink {
        label: "a".into(),
        href: "javascript:1".into(),
        current: false,
    });
    assert_eq!(
        validate(&p),
        Err(PageError::UnsafeHref { field: "nav href" })
    );
    let mut p = Page::new("x");
    p.blocks.push(Block::Table {
        caption: "c".into(),
        columns: vec!["a".into()],
        rows: vec![vec![Cell::link("t", "data:text/html,1")]],
    });
    assert_eq!(
        validate(&p),
        Err(PageError::UnsafeHref { field: "cell href" })
    );
}

#[test]
fn render_never_links_an_unsafe_href_even_without_validation() {
    let mut p = Page::new("x");
    p.nav.push(NavLink {
        label: "bad".into(),
        href: "javascript:alert(1)".into(),
        current: false,
    });
    p.blocks.push(Block::Table {
        caption: "c".into(),
        columns: vec!["a".into()],
        rows: vec![vec![Cell::link("t", "javascript:alert(1)")]],
    });
    let html = render(&p);
    assert!(!html.contains("javascript:"));
    assert!(hrefs(&html).iter().all(|h| h == "#main"));
}

#[test]
fn href_values_are_attribute_escaped() {
    let mut p = Page::new("x");
    p.nav.push(NavLink {
        label: "a".into(),
        href: "a.html?x=1&y=\"2\"".into(),
        current: false,
    });
    let html = render(&p);
    assert!(html.contains("href=\"a.html?x=1&amp;y=&quot;2&quot;\""));
}

#[test]
fn output_has_no_external_url_except_user_hrefs() {
    let mut p = hostile("plain");
    p.nav.push(NavLink {
        label: "e".into(),
        href: "https://example.org/a".into(),
        current: false,
    });
    let html = render(&p);
    let mut rest = html.clone();
    for h in hrefs(&html) {
        let escaped = h.replace('&', "&amp;");
        rest = rest.replace(&escaped, "");
    }
    for needle in ["http://", "https://", "url(", "@import", "//cdn", "src="] {
        assert!(!rest.contains(needle), "{needle}");
    }
}

#[test]
fn output_never_contains_a_script_or_font_request() {
    let html = render(&demo::index());
    for needle in ["<script", "@font-face", "<link", "<img", "<iframe"] {
        assert!(!html.contains(needle), "{needle}");
    }
}

#[test]
fn unknown_fields_are_rejected_instead_of_ignored() {
    assert!(render_json(r#"{"title":"x","onclick":"1"}"#).is_err());
    assert!(render_json(
        r#"{"title":"x","blocks":[{"type":"notes","title":"a","paragraphs":[],"x":1}]}"#
    )
    .is_err());
}
