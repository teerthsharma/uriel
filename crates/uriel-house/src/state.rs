//! In-memory state of a House. The seal graph is the durable copy (see `store`).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::error::HouseError;
use crate::governor::CostModel;
use crate::types::*;

/// The Need (G0).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct NeedRec {
    pub project: String,
    pub goal: String,
    pub acceptance: Vec<String>,
}

/// Why a goal was retired and what replaced or killed it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Retire {
    /// `cost`, `refuted`, `guard-target-retired`, `replaced`.
    pub reason: String,
    /// The winning goal (for `cost`) or the refuting fact id.
    pub by: String,
}

/// A goal of the merged tree plus its runtime status.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct GoalRec {
    /// Spec with the parent resolved and `needs` completed (guards, ALT aliases).
    pub goal: Goal,
    pub owner: Fellow,
    pub status: GoalStatus,
    pub retired: Option<Retire>,
    /// `refuted` (waiting for a re-chart) or `reported` (an agent said so).
    pub blocked_reason: Option<String>,
    /// Declared RAM charged while Active.
    pub cost_mb: u64,
}

impl GoalRec {
    pub fn class(&self) -> JobClass {
        class_of(&self.goal)
    }
}

/// Governor class of a goal's test.
pub(crate) fn class_of(goal: &Goal) -> JobClass {
    goal.test
        .as_ref()
        .and_then(|t| JobClass::parse(&t.class))
        .unwrap_or(JobClass::Llm)
}

/// ALT group: the target and its Cameron alternatives.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AltGroup {
    pub members: Vec<GoalId>,
    pub winner: Option<GoalId>,
}

/// A refuted goal waiting for its fellow to re-chart it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Pending {
    pub fellow: Fellow,
    pub attempts: u8,
    /// Ids in the owner's chart when the refutation was first merged.
    pub snapshot: BTreeSet<GoalId>,
}

/// One recorded test run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct RunRec {
    pub key: String,
    pub status: TestStatus,
    pub secs: f64,
    pub ram_mb: u64,
    pub seq: u64,
}

/// A test bound to a goal, with its history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct TestRec {
    pub spec: TestSpec,
    /// The goal this test proves.
    pub goal: GoalId,
    pub covers: BTreeSet<String>,
    pub quarantined: bool,
    #[serde(skip)]
    pub runs: Vec<RunRec>,
}

/// One Need and the state that belongs to it alone.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct NeedState {
    pub id: GoalId,
    pub rec: NeedRec,
    pub charts: BTreeMap<Fellow, Chart>,
    pub merged: bool,
    pub merge_stale: bool,
    pub rejections: BTreeMap<Fellow, u8>,
}

impl NeedState {
    pub fn new(id: GoalId, rec: NeedRec) -> NeedState {
        NeedState {
            id,
            rec,
            charts: BTreeMap::new(),
            merged: false,
            merge_stale: false,
            rejections: BTreeMap::new(),
        }
    }
}

/// What changed since the last flush.
#[derive(Debug, Default)]
pub(crate) struct Dirty {
    pub meta: bool,
    pub goals: BTreeSet<GoalId>,
    /// Structural edges must be rebuilt (merge).
    pub structure: bool,
    pub tests: BTreeSet<String>,
    pub runs: Vec<(String, RunRec)>,
    pub cost: Vec<(JobClass, u64, u64)>,
    pub covers: Vec<(String, String)>,
    pub facts: Vec<String>,
}

/// Everything the House knows.
#[derive(Debug, Default)]
pub(crate) struct State {
    pub needs: Vec<NeedState>,
    pub facts: Vec<Fact>,
    pub goals: BTreeMap<GoalId, GoalRec>,
    pub alt_groups: BTreeMap<GoalId, AltGroup>,
    pub pending: BTreeMap<GoalId, Pending>,
    pub leap: BTreeSet<GoalId>,
    pub leaps_used: BTreeSet<GoalId>,
    pub tests: BTreeMap<String, TestRec>,
    pub cost: CostModel,
    pub seq: u64,
    pub dirty: Dirty,
}

/// The persisted part of [`State`] kept in the `Project` node.
#[derive(Debug, Serialize, Deserialize, Default)]
pub(crate) struct Meta {
    pub need: Option<NeedRec>,
    pub charts: BTreeMap<Fellow, Chart>,
    pub facts: Vec<Fact>,
    pub merged: bool,
    pub merge_stale: bool,
    pub alt_groups: BTreeMap<GoalId, AltGroup>,
    pub pending: BTreeMap<GoalId, Pending>,
    pub leap: BTreeSet<GoalId>,
    #[serde(default)]
    pub leaps_used: BTreeSet<GoalId>,
    pub rejections: BTreeMap<Fellow, u8>,
    pub seq: u64,
    #[serde(default)]
    pub more: Vec<NeedState>,
}

impl State {
    pub fn need_id(ordinal: usize) -> GoalId {
        match ordinal {
            0 | 1 => GoalId::new("G0"),
            n => GoalId::new(format!("N{n}.G0")),
        }
    }

    pub fn target(&self, sel: Option<&GoalId>) -> Result<usize, HouseError> {
        match sel {
            Some(id) => self
                .needs
                .iter()
                .position(|n| n.id == *id)
                .ok_or_else(|| HouseError::UnknownNeed(id.clone())),
            None => self.needs.len().checked_sub(1).ok_or(HouseError::NoNeed),
        }
    }

    pub fn meta(&self) -> Meta {
        let first = self.needs.first();
        Meta {
            need: first.map(|n| n.rec.clone()),
            charts: first.map(|n| n.charts.clone()).unwrap_or_default(),
            facts: self.facts.clone(),
            merged: first.is_some_and(|n| n.merged),
            merge_stale: first.is_some_and(|n| n.merge_stale),
            alt_groups: self.alt_groups.clone(),
            pending: self.pending.clone(),
            leap: self.leap.clone(),
            leaps_used: self.leaps_used.clone(),
            rejections: first.map(|n| n.rejections.clone()).unwrap_or_default(),
            seq: self.seq,
            more: self.needs.iter().skip(1).cloned().collect(),
        }
    }

    pub fn apply_meta(&mut self, m: Meta) {
        let (charts, merged, merge_stale, rejections) =
            (m.charts, m.merged, m.merge_stale, m.rejections);
        self.needs = m
            .need
            .map(|rec| NeedState {
                id: Self::need_id(1),
                rec,
                charts,
                merged,
                merge_stale,
                rejections,
            })
            .into_iter()
            .chain(m.more)
            .collect();
        self.facts = m.facts;
        self.alt_groups = m.alt_groups;
        self.pending = m.pending;
        self.leap = m.leap;
        self.leaps_used = m.leaps_used;
        self.seq = self.seq.max(m.seq);
    }

    pub fn next_seq(&mut self) -> u64 {
        self.seq += 1;
        self.seq
    }

    /// Children (by `parent`) of `id`, in natural order.
    pub fn children(&self, id: &GoalId) -> Vec<&GoalRec> {
        self.goals
            .values()
            .filter(|r| r.goal.parent.as_ref() == Some(id))
            .collect()
    }

    /// Ancestor goal ids from the root down to (excluding) `id`.
    pub fn ancestors(&self, id: &GoalId) -> Vec<GoalId> {
        let mut out = Vec::new();
        let mut cur = self.goals.get(id).and_then(|r| r.goal.parent.clone());
        let mut guard = 0;
        while let Some(p) = cur {
            out.push(p.clone());
            cur = self.goals.get(&p).and_then(|r| r.goal.parent.clone());
            guard += 1;
            if guard > 1_000 {
                break;
            }
        }
        out.reverse();
        out
    }

    /// `true` when a child no longer holds its parent open.
    pub fn is_done(rec: &GoalRec) -> bool {
        matches!(rec.status, GoalStatus::Reached | GoalStatus::Retired)
    }

    /// Declared RAM cost of a leaf: the learned class average, else its estimate, else the
    /// class default.
    pub fn cost_of(&self, rec: &GoalRec, window_class: JobClass) -> u64 {
        if window_class == JobClass::Llm {
            return 0;
        }
        if let Some(avg) = self.cost.average(window_class) {
            return avg;
        }
        match rec.goal.est {
            Some(e) if e.ram_mb > 0 => e.ram_mb,
            _ => crate::governor::default_cost_mb(window_class),
        }
    }

    /// Resolves every non-leaf whose children are all done and which owes no integration
    /// test. Returns the ids resolved, deepest first.
    pub fn resolve_parents(&mut self) -> Vec<GoalId> {
        let mut resolved = Vec::new();
        loop {
            let mut ready: Vec<GoalId> = Vec::new();
            for (id, rec) in &self.goals {
                if rec.goal.leaf
                    || !matches!(rec.status, GoalStatus::Pending | GoalStatus::Active)
                    || rec.goal.test.is_some()
                {
                    continue;
                }
                let kids = self.children(id);
                if !kids.is_empty() && kids.iter().all(|k| Self::is_done(k)) {
                    ready.push(id.clone());
                }
            }
            if ready.is_empty() {
                return resolved;
            }
            // deepest first so a chain resolves bottom-up in one report
            ready.sort_by_key(|id| std::cmp::Reverse(self.ancestors(id).len()));
            let id = ready.remove(0);
            if let Some(r) = self.goals.get_mut(&id) {
                r.status = GoalStatus::Reached;
            }
            self.dirty.goals.insert(id.clone());
            resolved.push(id);
        }
    }
}
