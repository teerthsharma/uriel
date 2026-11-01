use uriel_page::*;

fn table(rows: usize, cols: usize) -> Block {
    Block::Table {
        caption: "c".into(),
        columns: vec!["h".into(); cols],
        rows: vec![vec![Cell::text("x"); cols]; rows],
    }
}

fn nest(depth: usize) -> Block {
    let mut b = Block::Rule;
    for _ in 0..depth {
        b = Block::Section {
            title: "s".into(),
            blocks: vec![b],
        };
    }
    b
}

fn page(b: Vec<Block>) -> Page {
    Page {
        title: "t".into(),
        blocks: b,
        ..Page::default()
    }
}

#[test]
fn oversized_text_is_an_error() {
    let mut p = Page::new("x".repeat(MAX_TEXT_CHARS + 1));
    assert!(matches!(validate(&p), Err(PageError::Limit { .. })));
    p.title = "x".repeat(MAX_TEXT_CHARS);
    assert!(validate(&p).is_ok());
}

#[test]
fn too_many_rows_columns_cells_blocks_are_errors() {
    assert!(matches!(
        validate(&page(vec![table(MAX_ROWS + 1, 1)])),
        Err(PageError::Limit { .. })
    ));
    assert!(matches!(
        validate(&page(vec![table(1, MAX_COLUMNS + 1)])),
        Err(PageError::Limit { .. })
    ));
    assert!(matches!(
        validate(&page(vec![table(MAX_ROWS, MAX_COLUMNS)])),
        Err(PageError::Limit {
            what: "table cells",
            ..
        })
    ));
    assert!(matches!(
        validate(&page(vec![Block::Rule; MAX_BLOCKS + 1])),
        Err(PageError::Limit { what: "blocks", .. })
    ));
    assert!(validate(&page(vec![Block::Rule; MAX_BLOCKS])).is_ok());
}

#[test]
fn total_text_budget_is_enforced() {
    let big = "y".repeat(MAX_TEXT_CHARS);
    let p = page(vec![Block::Notes {
        title: "n".into(),
        paragraphs: vec![big; 140],
    }]);
    assert!(matches!(
        validate(&p),
        Err(PageError::Limit {
            what: "total text bytes",
            ..
        })
    ));
}

#[test]
fn nesting_deeper_than_the_cap_is_an_error() {
    assert!(validate(&page(vec![nest(MAX_DEPTH)])).is_ok());
    assert!(matches!(
        validate(&page(vec![nest(MAX_DEPTH + 1)])),
        Err(PageError::Limit {
            what: "nesting depth",
            ..
        })
    ));
}

#[test]
fn render_truncates_instead_of_recursing_past_the_cap() {
    let html = render(&page(vec![nest(300)]));
    assert!(html.matches("<section").count() <= MAX_DEPTH);
}

#[test]
fn malformed_shapes_are_errors() {
    let ragged = Block::Table {
        caption: "c".into(),
        columns: vec!["a".into(), "b".into()],
        rows: vec![vec![Cell::text("x")]],
    };
    assert!(matches!(
        validate(&page(vec![ragged])),
        Err(PageError::Shape(_))
    ));
    let none = Block::Table {
        caption: "c".into(),
        columns: vec![],
        rows: vec![],
    };
    assert!(matches!(
        validate(&page(vec![none])),
        Err(PageError::Shape(_))
    ));
    let over = Block::Progress {
        label: "p".into(),
        done: 3,
        total: 2,
    };
    assert!(matches!(
        validate(&page(vec![over])),
        Err(PageError::Shape(_))
    ));
}

#[test]
fn render_json_rejects_huge_garbage_and_deep_json_without_panicking() {
    assert!(matches!(
        render_json(&" ".repeat(MAX_INPUT_BYTES + 1)),
        Err(PageError::InputTooLarge { .. })
    ));
    assert!(matches!(
        render_json(&"[".repeat(500_000)),
        Err(PageError::Parse(_))
    ));
    let deep = format!(
        "{}{}",
        r#"{"type":"section","title":"s","blocks":["#.repeat(5000),
        "]}".repeat(5000)
    );
    assert!(render_json(&format!(r#"{{"title":"t","blocks":[{deep}]}}"#)).is_err());
    for bad in [
        "",
        "null",
        "[]",
        "{",
        "{\"title\":1}",
        "{\"title\":\"x\",\"blocks\":[{\"type\":\"nope\"}]}",
        "\u{0}",
    ] {
        assert!(render_json(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn worst_case_valid_page_stays_within_a_bounded_size() {
    let big = "&".repeat(MAX_TEXT_CHARS);
    let p = page(vec![Block::Notes {
        title: "n".into(),
        paragraphs: vec![big; 100],
    }]);
    validate(&p).unwrap();
    assert!(render(&p).len() < 2 * 1024 * 1024);
}

#[test]
fn empty_page_renders_a_valid_document_with_a_placeholder() {
    let html = render(&Page::new("Empty"));
    assert!(html.contains("<h1 dir=\"auto\">Empty</h1>"));
    assert!(html.contains("Nothing is charted on this page yet."));
}

#[test]
fn cells_parse_from_text_chip_and_link_objects() {
    let c: Vec<Cell> =
        serde_json::from_str(r##"["a", {"text":"b","chip":"watch"}, {"text":"c","href":"#x"}]"##)
            .unwrap();
    assert_eq!(
        c,
        vec![
            Cell::text("a"),
            Cell::chip("b", Chip::Watch),
            Cell::link("c", "#x")
        ]
    );
}
