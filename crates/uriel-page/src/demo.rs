use crate::spec::{Block, Cell, Chip, Entry, Item, NavLink, Page, State, Tile};

fn nav(current: &str) -> Vec<NavLink> {
    [
        ("Chart", "index.html"),
        ("Research", "department-research.html"),
    ]
    .iter()
    .map(|(label, href)| NavLink {
        label: (*label).to_string(),
        href: (*href).to_string(),
        current: *href == current,
    })
    .collect()
}

fn tile(label: &str, value: &str, unit: Option<&str>, state: State, hint: &str) -> Tile {
    Tile {
        label: label.into(),
        value: value.into(),
        unit: unit.map(Into::into),
        state: Some(state),
        hint: Some(hint.into()),
    }
}

fn item(text: &str, done: bool, note: &str) -> Item {
    Item {
        text: text.into(),
        done,
        note: Some(note.into()),
    }
}

fn entry(at: &str, who: &str, text: &str, state: State) -> Entry {
    Entry {
        at: at.into(),
        who: who.into(),
        text: text.into(),
        state: Some(state),
    }
}

fn row(name: Cell, status: (&str, Chip), owner: &str, next: &str) -> Vec<Cell> {
    vec![
        name,
        Cell::chip(status.0, status.1),
        Cell::text(owner),
        Cell::text(next),
    ]
}

pub fn index() -> Page {
    Page {
        title: "Uriel: team board".into(),
        subtitle: Some(
            "A small light on a large graph. Four departments, one gate, replayable from the log."
                .into(),
        ),
        chart_id: Some("URIEL-0002".into()),
        ward: Some("Engine, 20 crates".into()),
        attending: Some("Owner, with the router".into()),
        nav: nav("index.html"),
        blocks: vec![
            Block::Vitals {
                tiles: vec![
                    tile("Crates", "20", None, State::Ok, "one workspace"),
                    tile("Tests", "2,001", Some("fns"), State::Ok, "counted by #[test]"),
                    tile(
                        "Tool-list cost",
                        "61-71",
                        Some("% lower"),
                        State::Ok,
                        "6.6k to 2.6k, 10.6k to 3.0k tokens",
                    ),
                    tile("Worker slots", "3", Some("default"), State::Watch, "1 to 16, RAM bound"),
                ],
            },
            Block::Progress {
                label: "Revamp goals charted".into(),
                done: 9,
                total: 12,
            },
            Block::Table {
                caption: "Departments".into(),
                columns: vec![
                    "Department".into(),
                    "Status".into(),
                    "Owner".into(),
                    "Next order".into(),
                ],
                rows: vec![
                    row(
                        Cell::link("Research", "department-research.html"),
                        ("On duty", Chip::Ok),
                        "Research agent",
                        "Measure token cost on the frozen queries",
                    ),
                    row(
                        Cell::text("Build"),
                        ("Slots full", Chip::Watch),
                        "Worker pool",
                        "Run RED-first tests for the next leaf",
                    ),
                    row(
                        Cell::text("Review"),
                        ("Needs a person", Chip::Alert),
                        "Owner",
                        "Read the attention list",
                    ),
                    row(
                        Cell::text("Docs"),
                        ("Idle", Chip::Idle),
                        "Docs agent",
                        "Wait for the next merged change",
                    ),
                ],
            },
            Block::Columns {
                blocks: vec![
                    Block::Checklist {
                        title: "Goal system, three tiers".into(),
                        items: vec![
                            item("Goal: one request from the one allowed identity", true, "/afk 2h with a goal"),
                            item("Sub-goals: charted into at most 12 leaves", true, "House, else the model"),
                            item("Tickets: gate, eval, accept, then a worker slot", false, "Capacity follows free RAM"),
                        ],
                    },
                    Block::Timeline {
                        title: "Orders and notes".into(),
                        entries: vec![
                            entry("Day 1", "Router", "Gate admitted the request.", State::Ok),
                            entry("Day 1", "Build", "Three workers running, two leaves queued.", State::Watch),
                            entry("Day 2", "Review", "A change waits for a person.", State::Alert),
                            entry("Day 2", "Docs", "No merged change to document.", State::Idle),
                        ],
                    },
                ],
            },
            Block::Rule,
            Block::Notes {
                title: "Chart notes".into(),
                paragraphs: vec![
                    "The same spec renders to the same bytes on every machine. This page is regenerated by a test and compared with the committed file.".into(),
                    "Department status on this sample board is illustrative. Crate, test and token figures come from this repository.".into(),
                ],
            },
        ],
        footer: Some("Generated by uriel-page. No scripts, no external requests.".into()),
        ..Page::default()
    }
}

pub fn department_research() -> Page {
    Page {
        title: "Research department".into(),
        subtitle: Some(
            "Measure first, claim second. Every figure names the command that produced it.".into(),
        ),
        chart_id: Some("URIEL-0002-R".into()),
        ward: Some("Research".into()),
        attending: Some("Research agent".into()),
        nav: nav("department-research.html"),
        blocks: vec![
            Block::Vitals {
                tiles: vec![
                    tile(
                        "Open studies",
                        "2",
                        None,
                        State::Watch,
                        "token cost, cache hit rate",
                    ),
                    tile(
                        "Cache hit rate",
                        "65.8",
                        Some("%"),
                        State::Ok,
                        "Zipf 1.0, gate on",
                    ),
                    tile(
                        "Compact v3",
                        "8.31",
                        Some("% fewer tokens"),
                        State::Ok,
                        "o200k, 100 frozen queries",
                    ),
                ],
            },
            Block::Section {
                title: "Orders".into(),
                blocks: vec![
                    Block::Table {
                        caption: "Studies in progress".into(),
                        columns: vec!["Study".into(), "Status".into(), "Evidence".into()],
                        rows: vec![
                            vec![
                                Cell::text("Tool-list cost"),
                                Cell::chip("Measured", Chip::Done),
                                Cell::text("tools/list plus instructions, 4 chars per token"),
                            ],
                            vec![
                                Cell::text("Tier hit rate"),
                                Cell::chip("Reading results", Chip::Watch),
                                Cell::text("tier_stats over 20k nodes"),
                            ],
                            vec![
                                Cell::text("Real tokenizer run"),
                                Cell::chip("Needs tiktoken", Chip::Alert),
                                Cell::text("token_dump, ignored by default"),
                            ],
                        ],
                    },
                    Block::Checklist {
                        title: "Before a number goes in a README".into(),
                        items: vec![
                            item(
                                "Run it on the same machine twice",
                                true,
                                "Medians, not best runs",
                            ),
                            item(
                                "Name the command that reproduces it",
                                true,
                                "Column Evidence",
                            ),
                            item("State what was not measured", false, "Open"),
                        ],
                    },
                ],
            },
            Block::Timeline {
                title: "Lab notes".into(),
                entries: vec![
                    entry(
                        "Mon",
                        "Research",
                        "Froze 100 queries for the token study.",
                        State::Ok,
                    ),
                    entry(
                        "Tue",
                        "Research",
                        "Compact v3 decodes to the same hits.",
                        State::Ok,
                    ),
                    entry(
                        "Wed",
                        "Review",
                        "Asked for the tokenizer run to be repeated.",
                        State::Watch,
                    ),
                ],
            },
            Block::Notes {
                title: "Open questions".into(),
                paragraphs: vec![
                    "Does the cache gate still pay off when a scan burst arrives mid-run?".into(),
                ],
            },
        ],
        footer: Some("Sample department page built with uriel-page.".into()),
        ..Page::default()
    }
}
