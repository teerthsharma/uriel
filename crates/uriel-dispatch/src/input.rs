use serde::{Deserialize, Serialize};

use crate::policy::Policy;
use crate::types::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Input {
    Tick,
    SetPolicy(Policy),
    Pressure {
        free_mb: u64,
        total_mb: u64,
    },
    Ingest(Trigger),
    EvalDelivered {
        eval: EvalId,
    },
    EvalResult {
        eval: EvalId,
        verdicts: Vec<(TriggerId, Verdict)>,
    },
    EvalFailed {
        eval: EvalId,
    },
    StopOwned {
        trigger: TriggerId,
        ticket: TicketId,
    },
    ManualStop {
        ticket: TicketId,
    },
    Reeval {
        thread: ThreadId,
    },
    Dismiss {
        thread: ThreadId,
    },
    PrepareDone {
        ticket: TicketId,
    },
    PrepareFailed {
        ticket: TicketId,
    },
    TurnEnded {
        ticket: TicketId,
        background: bool,
    },
    BackgroundDone {
        ticket: TicketId,
    },
    Report {
        ticket: TicketId,
        report: Report,
        attention: Option<String>,
    },
    Ack {
        ticket: TicketId,
        trigger: TriggerId,
    },
    HandBack {
        ticket: TicketId,
        trigger: TriggerId,
        reason: String,
    },
    Follow {
        ticket: TicketId,
        thread: ThreadId,
        kind: SubKind,
    },
    Unfollow {
        ticket: TicketId,
        thread: ThreadId,
    },
    DropThread {
        thread: ThreadId,
    },
    ThreadClosed {
        thread: ThreadId,
    },
    DeliveryFailed {
        ticket: TicketId,
    },
    DeliveryOk {
        ticket: TicketId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum Effect {
    StartEval {
        eval: EvalId,
        thread: ThreadId,
        version: u64,
        batch: Vec<Candidate>,
    },
    EvalExpired {
        eval: EvalId,
        retry: bool,
    },
    Decided(DecisionRecord),
    TicketOpened {
        ticket: TicketId,
        trigger: TriggerId,
    },
    Prepare {
        ticket: TicketId,
    },
    StartTurn {
        ticket: TicketId,
    },
    Deliver {
        ticket: TicketId,
        trigger: TriggerId,
    },
    Redeliver {
        ticket: TicketId,
    },
    KillJobs {
        ticket: TicketId,
    },
    Abort {
        ticket: TicketId,
    },
    HandOff {
        ticket: TicketId,
    },
    Ended {
        ticket: TicketId,
        end: End,
    },
    Resumed {
        ticket: TicketId,
    },
    PolicyChanged {
        ticket: TicketId,
        changes: Vec<Change>,
    },
    FollowUpNotice {
        trigger: TriggerId,
        tickets: Vec<TicketId>,
    },
    Unpoll {
        thread: ThreadId,
    },
    Rejected {
        what: &'static str,
        why: Refusal,
    },
}
