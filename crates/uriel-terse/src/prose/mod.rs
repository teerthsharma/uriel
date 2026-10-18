#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Lite,
    Full,
    Ultra,
}

pub fn compact(_text: &str, _level: Level) -> String {
    unimplemented!()
}
