mod common;

use common::{a11y, Rng};
use uriel_page::*;

const ALPHABET: &[&str] = &[
    "a", "Z", " ", "<", ">", "&", "\"", "'", "é", "ß", "漢", "م", "ש", "😀", "\u{202e}", "\n", "-",
    "x", "9",
];

fn text(r: &mut Rng) -> String {
    (0..r.below(12))
        .map(|_| ALPHABET[r.below(ALPHABET.len())])
        .collect()
}

fn state(r: &mut Rng) -> Option<State> {
    [
        None,
        Some(State::Ok),
        Some(State::Watch),
        Some(State::Alert),
        Some(State::Idle),
    ][r.below(5)]
}

fn block(r: &mut Rng, depth: usize) -> Block {
    let top = if depth >= MAX_DEPTH { 7 } else { 9 };
    match r.below(top) {
        0 => Block::Vitals {
            tiles: (0..r.below(5))
                .map(|_| Tile {
                    label: text(r),
                    value: text(r),
                    unit: Some(text(r)),
                    state: state(r),
                    hint: None,
                })
                .collect(),
        },
        1 => {
            let cols = 1 + r.below(4);
            let rows = (0..r.below(5))
                .map(|_| {
                    (0..cols)
                        .map(|_| match r.below(3) {
                            0 => Cell::text(text(r)),
                            1 => Cell::chip(
                                text(r),
                                [Chip::Ok, Chip::Watch, Chip::Alert, Chip::Idle, Chip::Done]
                                    [r.below(5)],
                            ),
                            _ => Cell::link(text(r), if r.below(2) == 0 { "#a" } else { "x.html" }),
                        })
                        .collect()
                })
                .collect();
            Block::Table {
                caption: text(r),
                columns: (0..cols).map(|_| text(r)).collect(),
                rows,
            }
        }
        2 => Block::Checklist {
            title: text(r),
            items: (0..r.below(5))
                .map(|_| Item {
                    text: text(r),
                    done: r.below(2) == 0,
                    note: Some(text(r)),
                })
                .collect(),
        },
        3 => {
            let total = r.below(50) as u32;
            Block::Progress {
                label: text(r),
                done: if total == 0 {
                    0
                } else {
                    r.below(total as usize + 1) as u32
                },
                total,
            }
        }
        4 => Block::Timeline {
            title: text(r),
            entries: (0..r.below(4))
                .map(|_| Entry {
                    at: text(r),
                    who: text(r),
                    text: text(r),
                    state: state(r),
                })
                .collect(),
        },
        5 => Block::Notes {
            title: text(r),
            paragraphs: (0..r.below(3)).map(|_| text(r)).collect(),
        },
        6 => Block::Rule,
        7 => Block::Section {
            title: text(r),
            blocks: (0..r.below(4)).map(|_| block(r, depth + 1)).collect(),
        },
        _ => Block::Columns {
            blocks: (0..1 + r.below(3)).map(|_| block(r, depth + 1)).collect(),
        },
    }
}

#[test]
fn random_specs_render_without_panic_and_pass_the_a11y_checker() {
    let mut r = Rng(0x9E37_79B9_7F4A_7C15);
    for i in 0..400 {
        let page = Page {
            title: text(&mut r),
            subtitle: if i % 2 == 0 { Some(text(&mut r)) } else { None },
            chart_id: Some(text(&mut r)),
            theme: [Theme::Auto, Theme::Light, Theme::Dark][i % 3],
            nav: (0..i % 3)
                .map(|k| NavLink {
                    label: text(&mut r),
                    href: "p.html".into(),
                    current: k == 0,
                })
                .collect(),
            blocks: (0..r.below(6)).map(|_| block(&mut r, 0)).collect(),
            footer: Some(text(&mut r)),
            ..Page::default()
        };
        validate(&page).unwrap_or_else(|e| panic!("case {i}: {e}"));
        let html = render(&page);
        a11y(&html).unwrap_or_else(|e| panic!("case {i}: {e}"));
        assert_eq!(html, render(&page));
    }
}

#[test]
fn checker_rejects_the_defects_it_claims_to_catch() {
    let ok = render(&demo::index());
    assert!(a11y(&ok).is_ok());
    let cases = [
        ok.replace("<caption", "<captio"),
        ok.replacen("scope=\"col\"", "", 1),
        ok.replace("aria-label=\"Chart sections\"", ""),
        ok.replacen("<h2", "<h4", 1).replacen("</h2>", "</h4>", 1),
        ok.replacen("id=\"c1-", "id=\"c2-", 1),
        ok.replacen("<main", "<main onclick=\"x()\"", 1),
        ok.replace(" lang=\"en\"", ""),
        ok.replacen("<style>", "<script></script><style>", 1),
    ];
    for (i, c) in cases.iter().enumerate() {
        assert!(a11y(c).is_err(), "case {i} was not caught");
    }
}

#[test]
fn unicode_rtl_and_emoji_survive_intact_and_are_direction_isolated() {
    let mut p = Page::new("שלום עולם 😀 漢字");
    p.blocks.push(Block::Notes {
        title: "مرحبا".into(),
        paragraphs: vec!["e\u{301} \u{202e}abc".into()],
    });
    let html = render(&p);
    assert!(std::str::from_utf8(html.as_bytes()).is_ok());
    assert!(html.contains("<h1 dir=\"auto\">שלום עולם 😀 漢字</h1>"));
    assert!(html.contains("مرحبا"));
    assert!(html.contains("e\u{301} \u{202e}abc"));
    a11y(&html).unwrap();
}

#[test]
fn document_has_doctype_lang_charset_and_viewport() {
    let html = render(&Page::new("t"));
    assert!(html.starts_with("<!doctype html>\n<html lang=\"en\">"));
    assert!(html.contains("<meta charset=\"utf-8\">"));
    assert!(html.contains("name=\"viewport\""));
    assert!(html.contains("prefers-color-scheme:dark"));
    assert!(html.contains("prefers-reduced-motion"));
    assert!(html.contains("@media print"));
}

#[test]
fn status_is_never_colour_only() {
    let html = render(&demo::index());
    assert!(html.contains("class=\"vh\">(watch)"));
    assert!(html.contains("class=\"g g-al\" aria-hidden=\"true\""));
    assert!(html.contains("<span class=\"vh\">Done: </span>"));
    assert!(html.contains("<span class=\"vh\">Open: </span>"));
    assert!(html.contains("role=\"img\" aria-label=\"Revamp goals charted: 9 of 12\""));
    assert!(html.contains("9 of 12 (75%)"));
}

#[test]
fn headings_clamp_at_h6_without_skipping() {
    let mut b = Block::Notes {
        title: "leaf".into(),
        paragraphs: vec![],
    };
    for _ in 0..MAX_DEPTH {
        b = Block::Section {
            title: "s".into(),
            blocks: vec![b],
        };
    }
    let html = render(&Page {
        title: "t".into(),
        blocks: vec![b],
        ..Page::default()
    });
    assert!(!html.contains("<h7"));
    a11y(&html).unwrap();
}

#[test]
fn ids_derive_from_content_and_position_only() {
    let mut p = Page::new("t");
    p.blocks.push(Block::Notes {
        title: "Hello, World!".into(),
        paragraphs: vec![],
    });
    p.blocks.push(Block::Notes {
        title: "Hello, World!".into(),
        paragraphs: vec![],
    });
    let html = render(&p);
    assert!(html.contains("id=\"c1-hello-world\""));
    assert!(html.contains("id=\"c2-hello-world\""));
    assert_eq!(slug("日本語"), "x");
}

#[test]
fn css_and_demo_pages_stay_within_the_size_budget() {
    assert!(CSS.len() <= 10_000, "css {}", CSS.len());
    assert!(render(&demo::index()).len() < 30 * 1024);
    assert!(render(&demo::department_research()).len() < 30 * 1024);
}
