#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("bad id '{0}': use 1 to 64 characters from A-Z a-z 0-9 _ . : -")]
    BadId(String),
    #[error("unknown run '{0}'")]
    UnknownRun(String),
    #[error("unknown agent '{0}'")]
    UnknownAgent(String),
    #[error("agent '{0}' is not running")]
    NotRunning(String),
    #[error("too many runs: {0} are still active and none can be evicted")]
    TooManyRuns(usize),
}
