use serde::{Deserialize, Serialize};

pub type Ms = u64;

macro_rules! string_id {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            pub fn new(s: impl Into<String>) -> $name {
                $name(s.into())
            }
        }
    };
}

string_id!(TriggerId);
string_id!(ThreadId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TicketId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EvalId(pub u64);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Actor {
    pub id: String,
    pub bot: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Trigger {
    pub id: TriggerId,
    pub source: String,
    pub scope: String,
    pub thread: ThreadId,
    pub actor: Option<Actor>,
    pub addressed: bool,
}

impl Trigger {
    pub fn is_person(&self) -> bool {
        self.actor.as_ref().is_some_and(|a| !a.bot)
    }

    pub fn is_bot(&self) -> bool {
        self.actor.as_ref().is_some_and(|a| a.bot)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Strength {
    Inferred,
    Explicit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum CellRef {
    ListedUnaddressed,
    UnlistedAddressed,
    UnlistedUnaddressed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Basis {
    Followup,
    Listed,
    Table(CellRef),
    Bot,
    Inferred,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mandate {
    pub policy_version: u64,
    pub scope: String,
    pub strength: Strength,
    pub rules: Vec<String>,
    pub sender: Option<String>,
    pub source: String,
    pub receiving_scope: String,
    pub thread: ThreadId,
    pub basis: Basis,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Refusal {
    UnknownSource,
    SourceOff,
    UnknownScope,
    NotReceivedAll,
    Ignored,
    ModeOff,
    BotNotListed,
    BotNotAddressed,
    BotCannotStop,
    NotExplicit,
    NoMatchingRule,
    UnknownRule,
    DisabledRule,
    NoSuchTicket,
    TicketStopped,
    TicketRevoked,
    TicketOwned,
    TicketEnded,
    NotOwnedByTicket,
    NotPerson,
    StaleEval,
    NoSlot,
    OriginPinned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Revoke {
    SourceGone,
    SenderRemoved,
    BotRemoved,
    CellWeakened,
    ProactiveOff,
    RuleDisabled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Verdict {
    Accept {
        scope: String,
        rules: Vec<String>,
        assessed: bool,
    },
    Attach {
        ticket: TicketId,
    },
    Stop {
        ticket: TicketId,
    },
    Ignore {
        from_card: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Why {
    Timeout,
    StalePolicy,
    AssessmentFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Decision {
    Accept(TicketId),
    Attach(TicketId),
    Stop(TicketId),
    ManualStop(TicketId),
    Ignore { from_card: bool },
    WouldAccept { rules: Vec<String> },
    Unavailable(Why),
    Refused(Refusal),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionRecord {
    pub at: Ms,
    pub thread: ThreadId,
    pub trigger: Option<TriggerId>,
    pub version: u64,
    pub decision: Decision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Disposition {
    Filtered(Refusal),
    Duplicate,
    Evaluated(EvalId),
    FannedOut(Vec<TicketId>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Report {
    Progress,
    Waiting,
    Ready,
    Blocked,
    Failed,
    Dropped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum SubKind {
    Origin,
    Attached,
    Context,
}

impl SubKind {
    pub fn authority(self) -> bool {
        !matches!(self, SubKind::Context)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum End {
    Done,
    Closed,
    Stopped,
    Revoked(Revoke),
}

impl End {
    pub fn resumable(self) -> bool {
        matches!(self, End::Done | End::Closed)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Stage {
    PersonDelivery,
    PersonAnswer,
    PersonReview,
    PersonBlocked,
    Queued,
    Idle,
    Working,
}

impl Stage {
    pub fn needs_person(self) -> bool {
        matches!(
            self,
            Stage::PersonDelivery
                | Stage::PersonAnswer
                | Stage::PersonReview
                | Stage::PersonBlocked
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Change {
    Rule(String),
    Revoked(Revoke),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    pub trigger: TriggerId,
    pub strength: Strength,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SlotKind {
    Regular,
    Reserve,
}
