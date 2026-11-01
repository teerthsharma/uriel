use crate::error::PageError;
use crate::spec::{Block, Cell, Page};

pub const MAX_INPUT_BYTES: usize = 1 << 20;
pub const MAX_DEPTH: usize = 6;
pub const MAX_BLOCKS: usize = 500;
pub const MAX_TEXT_CHARS: usize = 2000;
pub const MAX_TOTAL_TEXT_BYTES: usize = 256 * 1024;
pub const MAX_NAV: usize = 16;
pub const MAX_TILES: usize = 24;
pub const MAX_COLUMNS: usize = 16;
pub const MAX_ROWS: usize = 500;
pub const MAX_CELLS: usize = 5000;
pub const MAX_ITEMS: usize = 300;
pub const MAX_ENTRIES: usize = 300;
pub const MAX_PARAGRAPHS: usize = 200;
pub const MAX_COLUMN_BLOCKS: usize = 12;
pub const MAX_HREF_CHARS: usize = 2048;

pub fn href_allowed(href: &str) -> bool {
    if href.is_empty() || href.chars().count() > MAX_HREF_CHARS {
        return false;
    }
    if href
        .chars()
        .any(|c| c.is_control() || c == '\\' || c == ' ')
    {
        return false;
    }
    if href.starts_with('#') {
        return true;
    }
    if href.starts_with("//") {
        return false;
    }
    let lower = href.to_ascii_lowercase();
    if lower.starts_with("https://") || lower.starts_with("http://") {
        return lower.len() > 8
            && !lower.starts_with("http:///")
            && !lower.starts_with("https:///");
    }
    if lower.starts_with("mailto:") {
        return lower.len() > 7;
    }
    let head_end = href.find(['/', '?', '#']).unwrap_or(href.len());
    !href[..head_end].contains(':')
}

struct Counter {
    blocks: usize,
    cells: usize,
    text_bytes: usize,
}

impl Counter {
    fn text(&mut self, what: &'static str, s: &str) -> Result<(), PageError> {
        if s.chars().count() > MAX_TEXT_CHARS {
            return Err(PageError::Limit {
                what,
                max: MAX_TEXT_CHARS,
            });
        }
        self.text_bytes += s.len();
        if self.text_bytes > MAX_TOTAL_TEXT_BYTES {
            return Err(PageError::Limit {
                what: "total text bytes",
                max: MAX_TOTAL_TEXT_BYTES,
            });
        }
        Ok(())
    }

    fn opt(&mut self, what: &'static str, s: &Option<String>) -> Result<(), PageError> {
        match s {
            Some(s) => self.text(what, s),
            None => Ok(()),
        }
    }
}

fn cap(what: &'static str, n: usize, max: usize) -> Result<(), PageError> {
    if n > max {
        Err(PageError::Limit { what, max })
    } else {
        Ok(())
    }
}

pub fn validate(page: &Page) -> Result<(), PageError> {
    let mut c = Counter {
        blocks: 0,
        cells: 0,
        text_bytes: 0,
    };
    c.text("title", &page.title)?;
    c.opt("subtitle", &page.subtitle)?;
    c.opt("chart_id", &page.chart_id)?;
    c.opt("ward", &page.ward)?;
    c.opt("attending", &page.attending)?;
    c.opt("footer", &page.footer)?;
    cap("nav links", page.nav.len(), MAX_NAV)?;
    for n in &page.nav {
        c.text("nav label", &n.label)?;
        if !href_allowed(&n.href) {
            return Err(PageError::UnsafeHref { field: "nav href" });
        }
    }
    blocks(&page.blocks, 0, &mut c)
}

fn blocks(list: &[Block], depth: usize, c: &mut Counter) -> Result<(), PageError> {
    for b in list {
        block(b, depth, c)?;
    }
    Ok(())
}

fn block(b: &Block, depth: usize, c: &mut Counter) -> Result<(), PageError> {
    c.blocks += 1;
    cap("blocks", c.blocks, MAX_BLOCKS)?;
    match b {
        Block::Vitals { tiles } => {
            cap("tiles", tiles.len(), MAX_TILES)?;
            for t in tiles {
                c.text("tile label", &t.label)?;
                c.text("tile value", &t.value)?;
                c.opt("tile unit", &t.unit)?;
                c.opt("tile hint", &t.hint)?;
            }
        }
        Block::Table {
            caption,
            columns,
            rows,
        } => {
            c.text("table caption", caption)?;
            if columns.is_empty() {
                return Err(PageError::Shape("table needs at least one column"));
            }
            cap("table columns", columns.len(), MAX_COLUMNS)?;
            cap("table rows", rows.len(), MAX_ROWS)?;
            for col in columns {
                c.text("column heading", col)?;
            }
            for row in rows {
                if row.len() != columns.len() {
                    return Err(PageError::Shape("row width differs from column count"));
                }
                c.cells += row.len();
                cap("table cells", c.cells, MAX_CELLS)?;
                for cell in row {
                    match cell {
                        Cell::Text(t) => c.text("cell", t)?,
                        Cell::Chip(x) => c.text("cell", &x.text)?,
                        Cell::Link(x) => {
                            c.text("cell", &x.text)?;
                            if !href_allowed(&x.href) {
                                return Err(PageError::UnsafeHref { field: "cell href" });
                            }
                        }
                    }
                }
            }
        }
        Block::Checklist { title, items } => {
            c.text("checklist title", title)?;
            cap("checklist items", items.len(), MAX_ITEMS)?;
            for i in items {
                c.text("item", &i.text)?;
                c.opt("item note", &i.note)?;
            }
        }
        Block::Progress { label, done, total } => {
            c.text("progress label", label)?;
            if done > total {
                return Err(PageError::Shape("progress done exceeds total"));
            }
        }
        Block::Timeline { title, entries } => {
            c.text("timeline title", title)?;
            cap("timeline entries", entries.len(), MAX_ENTRIES)?;
            for e in entries {
                c.text("entry time", &e.at)?;
                c.text("entry author", &e.who)?;
                c.text("entry text", &e.text)?;
            }
        }
        Block::Notes { title, paragraphs } => {
            c.text("notes title", title)?;
            cap("paragraphs", paragraphs.len(), MAX_PARAGRAPHS)?;
            for p in paragraphs {
                c.text("paragraph", p)?;
            }
        }
        Block::Section {
            title,
            blocks: inner,
        } => {
            c.text("section title", title)?;
            if depth + 1 > MAX_DEPTH {
                return Err(PageError::Limit {
                    what: "nesting depth",
                    max: MAX_DEPTH,
                });
            }
            blocks(inner, depth + 1, c)?;
        }
        Block::Columns { blocks: inner } => {
            cap("column blocks", inner.len(), MAX_COLUMN_BLOCKS)?;
            if depth + 1 > MAX_DEPTH {
                return Err(PageError::Limit {
                    what: "nesting depth",
                    max: MAX_DEPTH,
                });
            }
            blocks(inner, depth + 1, c)?;
        }
        Block::Rule => {}
    }
    Ok(())
}
