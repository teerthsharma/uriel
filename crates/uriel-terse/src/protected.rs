#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Fence,
    InlineCode,
    Url,
    Path,
    Identifier,
    Hash,
    Version,
    Number,
    Quoted,
    Latex,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub kind: Kind,
}

pub fn scan(_text: &str) -> Vec<Span> {
    unimplemented!()
}
