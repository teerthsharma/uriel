#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("the idea is empty after sanitising")]
    EmptyIdea,
    #[error("unknown plan '{0}': call genius_plan first")]
    UnknownPlan(String),
    #[error("unknown vertex '{0}': use abduce, analogize, question, refute, eliminate or monitor")]
    BadVertex(String),
    #[error("round {got} is not open: the plan is in {expected}")]
    WrongRound { got: String, expected: String },
    #[error("vertex '{vertex}' already submitted round {round}")]
    Duplicate { vertex: String, round: u8 },
    #[error("at most {0} claims per submission")]
    TooManyClaims(usize),
    #[error("claim {claim}: field '{field}' is {problem}")]
    Field {
        claim: String,
        field: &'static str,
        problem: &'static str,
    },
    #[error("claim id '{0}' must look like {1}")]
    BadClaimId(String, String),
    #[error("claim id '{0}' is used twice")]
    DuplicateClaimId(String),
    #[error("claim {0}: a wild claim must be cheap to test (cost S or M); a crazy idea you cannot test is a hallucination, so downgrade it to a question or find a cheaper test")]
    WildNotCheap(String),
    #[error("claim {0}: a wild claim cannot be near-certain (confidence above 800 per mille)")]
    WildTooSure(String),
    #[error("claim {0}: an observation needs at least one piece of evidence (file:line, url or chunk id)")]
    ObservationNeedsEvidence(String),
    #[error("claim {claim}: target '{target}' is not a round-1 claim of a vertex you were shown")]
    BadTarget { claim: String, target: String },
    #[error("claim {0}: an attack must carry evidence or a falsifier")]
    AttackNeedsGround(String),
    #[error("round 2 needs at least one claim that targets a claim you were shown")]
    NoEngagement,
    #[error("experiments are accepted once both rounds are complete; the plan is in {0}")]
    TooEarly(String),
    #[error("experiment names unknown or untestable claim '{0}' (only conjecture and wild claims can be tested)")]
    NotTestable(String),
    #[error("claim '{0}' already has a recorded outcome")]
    AlreadyTested(String),
    #[error("experiment on '{0}' needs evidence: a test result without evidence is hearsay")]
    ExperimentNeedsEvidence(String),
    #[error("malformed submission: {0}")]
    Malformed(String),
}

impl Error {
    pub fn code(&self) -> &'static str {
        match self {
            Error::EmptyIdea => "empty_idea",
            Error::UnknownPlan(_) => "unknown_plan",
            Error::BadVertex(_) => "bad_vertex",
            Error::WrongRound { .. } => "wrong_round",
            Error::Duplicate { .. } => "duplicate_submission",
            Error::TooManyClaims(_) => "too_many_claims",
            Error::Field { .. } => "bad_field",
            Error::BadClaimId(..) => "bad_claim_id",
            Error::DuplicateClaimId(_) => "duplicate_claim_id",
            Error::WildNotCheap(_) => "wild_not_cheap",
            Error::WildTooSure(_) => "wild_too_sure",
            Error::ObservationNeedsEvidence(_) => "observation_needs_evidence",
            Error::BadTarget { .. } => "bad_target",
            Error::AttackNeedsGround(_) => "attack_needs_ground",
            Error::NoEngagement => "no_engagement",
            Error::TooEarly(_) => "too_early",
            Error::NotTestable(_) => "not_testable",
            Error::AlreadyTested(_) => "already_tested",
            Error::ExperimentNeedsEvidence(_) => "experiment_needs_evidence",
            Error::Malformed(_) => "malformed",
        }
    }
}
