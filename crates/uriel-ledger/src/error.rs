#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("bad id '{0}': use 1 to 64 characters from A-Z a-z 0-9 _ . : -")]
    BadId(String),
    #[error("goal '{0}' needs a non-empty title")]
    EmptyTitle(String),
    #[error("a batch holds at most {max} goals, got {got}")]
    BatchTooLarge { max: usize, got: usize },
    #[error("a ledger holds at most {max} goals; this batch would make {got}")]
    TooManyGoals { max: usize, got: usize },
    #[error("goal '{id}' lists {got} dependencies; the limit is {max}")]
    TooManyNeeds { id: String, max: usize, got: usize },
    #[error("goal '{0}' depends on itself")]
    SelfNeed(String),
    #[error("goal '{id}' needs '{need}', which is not in the ledger")]
    UnknownNeed { id: String, need: String },
    #[error("the dependencies contain a cycle")]
    Cycle,
    #[error("goal '{0}' is already defined with different content; goals are append-only")]
    Changed(String),
    #[error("unknown goal '{0}'")]
    UnknownGoal(String),
    #[error("goal '{0}' was not issued; only an issued goal can be finished")]
    NotIssued(String),
    #[error("malformed ledger: {0}")]
    Malformed(String),
}

impl Error {
    /// Stable machine-readable code, used as the `error` field by the MCP layer.
    pub fn code(&self) -> &'static str {
        match self {
            Error::BadId(_) => "bad_id",
            Error::EmptyTitle(_) => "empty_title",
            Error::BatchTooLarge { .. } => "batch_too_large",
            Error::TooManyGoals { .. } => "too_many_goals",
            Error::TooManyNeeds { .. } => "too_many_needs",
            Error::SelfNeed(_) => "self_need",
            Error::UnknownNeed { .. } => "unknown_need",
            Error::Cycle => "cycle",
            Error::Changed(_) => "changed",
            Error::UnknownGoal(_) => "unknown_goal",
            Error::NotIssued(_) => "not_issued",
            Error::Malformed(_) => "malformed",
        }
    }
}
