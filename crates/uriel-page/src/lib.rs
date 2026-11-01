#![forbid(unsafe_code)]

pub mod demo;
mod error;
mod render;
pub mod spec;
mod validate;

pub use error::PageError;
pub use render::{escape, render, slug, CSS};
pub use spec::{
    Block, Cell, Chip, ChipCell, Entry, Item, LinkCell, NavLink, Page, State, Theme, Tile,
};
pub use validate::{href_allowed, validate};
pub use validate::{
    MAX_BLOCKS, MAX_CELLS, MAX_COLUMNS, MAX_DEPTH, MAX_INPUT_BYTES, MAX_ROWS, MAX_TEXT_CHARS,
};

use sha2::{Digest, Sha256};

pub fn digest(html: &str) -> String {
    let h = Sha256::digest(html.as_bytes());
    let mut s = String::with_capacity(64);
    for b in h.iter() {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

pub fn render_json(json: &str) -> Result<String, PageError> {
    if json.len() > MAX_INPUT_BYTES {
        return Err(PageError::InputTooLarge {
            max: MAX_INPUT_BYTES,
        });
    }
    let page: Page = serde_json::from_str(json).map_err(|e| PageError::Parse(e.to_string()))?;
    validate(&page)?;
    Ok(render(&page))
}
