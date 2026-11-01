use std::collections::{BTreeMap, BTreeSet};

use crate::types::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Open,
    Ended(End),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Parked,
    Queued { seq: u64, person: bool },
    Preparing { since: Ms },
    Turn,
    Background { since: Ms },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Assignment {
    Pending,
    Running,
    Ended,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputFail {
    pub attempts: u32,
    pub next_at: Ms,
}

#[derive(Debug, Clone)]
pub struct Ticket {
    pub id: TicketId,
    pub mandate: Mandate,
    pub status: Status,
    pub subs: BTreeMap<ThreadId, SubKind>,
    pub phase: Phase,
    pub slot: Option<SlotKind>,
    pub assignments: Vec<Assignment>,
    pub latest: Option<Report>,
    pub attention: Option<String>,
    pub person_since_report: bool,
    pub fresh_inputs: u32,
    pub owned: BTreeSet<TriggerId>,
    pub input_fail: Option<InputFail>,
    pub prepared: bool,
    pub opened_at: Ms,
    pub changed_at: Ms,
}

impl Ticket {
    pub fn stage(&self) -> Option<Stage> {
        if self.status != Status::Open {
            return None;
        }
        let queued = matches!(self.phase, Phase::Queued { .. } | Phase::Preparing { .. });
        let stage = if self.input_fail.is_some() {
            Stage::PersonDelivery
        } else if self.attention.is_some() {
            Stage::PersonAnswer
        } else if self.latest == Some(Report::Ready) && !self.person_since_report {
            Stage::PersonReview
        } else if matches!(self.latest, Some(Report::Blocked | Report::Failed))
            && !self.person_since_report
        {
            Stage::PersonBlocked
        } else if queued || matches!(self.assignments.last(), None | Some(Assignment::Pending)) {
            Stage::Queued
        } else if self.latest == Some(Report::Waiting)
            || self.assignments.last() == Some(&Assignment::Ended)
        {
            Stage::Idle
        } else {
            Stage::Working
        };
        Some(stage)
    }

    pub fn holds_slot(&self) -> bool {
        self.slot.is_some()
    }
}
