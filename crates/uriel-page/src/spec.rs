use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Page {
    pub title: String,
    #[serde(default)]
    pub subtitle: Option<String>,
    #[serde(default)]
    pub chart_id: Option<String>,
    #[serde(default)]
    pub ward: Option<String>,
    #[serde(default)]
    pub attending: Option<String>,
    #[serde(default)]
    pub theme: Theme,
    #[serde(default)]
    pub nav: Vec<NavLink>,
    #[serde(default)]
    pub blocks: Vec<Block>,
    #[serde(default)]
    pub footer: Option<String>,
}

impl Page {
    pub fn new(title: impl Into<String>) -> Self {
        Page {
            title: title.into(),
            ..Page::default()
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    #[default]
    Auto,
    Light,
    Dark,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NavLink {
    pub label: String,
    pub href: String,
    #[serde(default)]
    pub current: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Ok,
    Watch,
    Alert,
    Idle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Chip {
    Ok,
    Watch,
    Alert,
    Idle,
    Done,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tile {
    pub label: String,
    pub value: String,
    #[serde(default)]
    pub unit: Option<String>,
    #[serde(default)]
    pub state: Option<State>,
    #[serde(default)]
    pub hint: Option<String>,
}

impl Tile {
    pub fn new(label: impl Into<String>, value: impl Into<String>) -> Self {
        Tile {
            label: label.into(),
            value: value.into(),
            unit: None,
            state: None,
            hint: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Cell {
    Text(String),
    Chip(ChipCell),
    Link(LinkCell),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChipCell {
    pub text: String,
    pub chip: Chip,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkCell {
    pub text: String,
    pub href: String,
}

impl Cell {
    pub fn text(t: impl Into<String>) -> Self {
        Cell::Text(t.into())
    }

    pub fn chip(t: impl Into<String>, chip: Chip) -> Self {
        Cell::Chip(ChipCell {
            text: t.into(),
            chip,
        })
    }

    pub fn link(t: impl Into<String>, href: impl Into<String>) -> Self {
        Cell::Link(LinkCell {
            text: t.into(),
            href: href.into(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Item {
    pub text: String,
    #[serde(default)]
    pub done: bool,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub at: String,
    pub who: String,
    pub text: String,
    #[serde(default)]
    pub state: Option<State>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Block {
    Vitals {
        tiles: Vec<Tile>,
    },
    Table {
        caption: String,
        columns: Vec<String>,
        rows: Vec<Vec<Cell>>,
    },
    Checklist {
        title: String,
        items: Vec<Item>,
    },
    Progress {
        label: String,
        done: u32,
        total: u32,
    },
    Timeline {
        title: String,
        entries: Vec<Entry>,
    },
    Notes {
        title: String,
        paragraphs: Vec<String>,
    },
    Section {
        title: String,
        blocks: Vec<Block>,
    },
    Columns {
        blocks: Vec<Block>,
    },
    Rule,
}
