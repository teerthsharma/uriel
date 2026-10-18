//! The [`House`] facade: need, charts, merge, schedule, report.

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard, PoisonError};

use serde_json::{json, Value};

use crate::config::HouseConfig;
use crate::error::HouseError;
use crate::governor::wrap_command;
use crate::merge::{merge as run_merge, MergeInput};
use crate::probe::{SystemProbe, SystemSnapshot};
use crate::replay::{
    decision_digest, DecisionRecord, DispatchRecord, HouseCall, NullSink, ScriptedProbe,
};
use crate::sched;
use crate::sink::EventSink;
use crate::smart;
use crate::state::*;
use crate::store::Store;
use crate::types::*;
use crate::validate::validate_chart;

struct Inner {
    state: State,
    store: Store,
    history: Vec<HouseCall>,
    decisions: Vec<DecisionRecord>,
}

/// The harness. All methods take `&self`; one internal lock serialises them.
pub struct House {
    cfg: HouseConfig,
    probe: Box<dyn SystemProbe>,
    sink: Box<dyn EventSink>,
    inner: Mutex<Inner>,
}

fn rule0(fellow: Fellow, goal: GoalId, message: impl Into<String>) -> Vec<Violation> {
    vec![Violation {
        rule: 0,
        fellow,
        goal,
        message: message.into(),
    }]
}

fn goal_event(id: &GoalId, rec: &GoalRec, status: &str) -> Value {
    goal_event_with(
        id,
        rec,
        status,
        rec.goal.test.as_ref().map(|t| t.name.clone()),
    )
}

/// The test that proves a parent: the bound test of its first live leaf descendant
/// (RED-first proven), because the board demands a RED-first test behind every `reached`.
fn evidence_test(state: &State, id: &GoalId) -> Option<String> {
    let rec = state.goals.get(id)?;
    if rec.goal.leaf {
        return rec.goal.test.as_ref().map(|t| t.name.clone());
    }
    state
        .children(id)
        .into_iter()
        .filter(|c| c.status != GoalStatus::Retired)
        .find_map(|c| evidence_test(state, &c.goal.id))
}

fn goal_event_with(id: &GoalId, rec: &GoalRec, status: &str, test: Option<String>) -> Value {
    let by = rec.goal.tier.map_or("haiku", Tier::model);
    json!({
        "t": "goal",
        "agent": rec.owner.name(),
        "id": id.as_str(),
        "status": status,
        "by": by,
        "haiku": u32::from(by == "haiku"),
        "test": test,
    })
}

fn goal_counts(st: &State, ns: usize) -> Value {
    let mut counts = BTreeMap::from([
        ("pending", 0u64),
        ("active", 0),
        ("reached", 0),
        ("blocked", 0),
        ("retired", 0),
    ]);
    let root = State::need_id(ns);
    let mut total = 0u64;
    for (_, r) in st
        .goals
        .iter()
        .filter(|(id, _)| id.need_ordinal() == ns && **id != root)
    {
        total += 1;
        *counts.entry(sched::status_name(r.status)).or_default() += 1;
    }
    json!({
        "total": total,
        "pending": counts["pending"], "active": counts["active"], "reached": counts["reached"],
        "blocked": counts["blocked"], "retired": counts["retired"],
    })
}

impl House {
    /// Opens a House over `graph`, loading whatever an earlier House stored there.
    pub fn open(
        graph: uriel_core::Graph,
        cfg: HouseConfig,
        probe: Box<dyn SystemProbe>,
        sink: Box<dyn EventSink>,
    ) -> Result<House, HouseError> {
        let (store, state) = Store::open(graph, cfg.cost_window)?;
        Ok(House {
            cfg,
            probe,
            sink,
            inner: Mutex::new(Inner {
                state,
                store,
                history: Vec::new(),
                decisions: Vec::new(),
            }),
        })
    }

    #[allow(missing_docs)]
    pub fn resolve_need(&self, need: Option<&GoalId>) -> Result<GoalId, HouseError> {
        let g = self.lock();
        g.state.target(need).map(|i| g.state.needs[i].id.clone())
    }

    #[allow(missing_docs)]
    pub fn decision_log(&self) -> Vec<DecisionRecord> {
        self.lock().decisions.clone()
    }

    #[allow(missing_docs)]
    pub fn decision_digest(&self) -> [u8; 32] {
        decision_digest(&self.lock().decisions)
    }

    #[allow(missing_docs)]
    pub fn history(&self) -> Vec<HouseCall> {
        self.lock().history.clone()
    }

    #[allow(missing_docs)]
    pub fn replay(
        cfg: HouseConfig,
        graph: uriel_core::Graph,
        probe_script: Vec<SystemSnapshot>,
        history: Vec<HouseCall>,
    ) -> Result<Vec<DecisionRecord>, HouseError> {
        let house = House::open(
            graph,
            cfg,
            Box::new(ScriptedProbe::new(probe_script)),
            Box::new(NullSink),
        )?;
        for call in history {
            house.apply(call);
        }
        Ok(house.decision_log())
    }

    fn apply(&self, call: HouseCall) {
        match call {
            HouseCall::Need {
                project,
                goal,
                acceptance,
            } => {
                let _ = self.need(&project, &goal, &acceptance);
            }
            HouseCall::SubmitChart { fellow, chart } => {
                let _ = self.submit_chart(fellow, chart);
            }
            HouseCall::Merge { need } => {
                let _ = self.merge_in(need.as_ref());
            }
            HouseCall::AddFact { fact, need } => {
                let _ = self.add_fact_in(need.as_ref(), fact);
            }
            HouseCall::Next { need } => {
                let _ = self.next_in(need.as_ref());
            }
            HouseCall::Report {
                goal,
                status,
                evidence,
                need,
            } => {
                let _ = self.report_in(need.as_ref(), &goal, status, evidence);
            }
            HouseCall::RecordTestRun {
                need,
                test,
                status,
                secs,
                peak_ram_mb,
                covered_files,
                key,
            } => {
                let _ = self.record_run(
                    need.as_ref(),
                    &test,
                    status,
                    secs,
                    peak_ram_mb,
                    &covered_files,
                    key,
                );
            }
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn emit_all(&self, events: Vec<Value>) {
        for e in events {
            self.sink.emit(e);
        }
    }

    fn flush(&self, inner: &mut Inner) -> Result<(), HouseError> {
        inner.store.flush(&mut inner.state, &self.cfg.project_root)
    }

    #[allow(missing_docs)]
    pub fn need(
        &self,
        project: &str,
        goal: &str,
        acceptance: &[String],
    ) -> Result<GoalId, HouseError> {
        let mut g = self.lock();
        let inner = &mut *g;
        inner.history.push(HouseCall::Need {
            project: project.to_string(),
            goal: goal.to_string(),
            acceptance: acceptance.to_vec(),
        });
        let root = State::need_id(inner.state.needs.len() + 1);
        inner.state.needs.push(NeedState::new(
            root.clone(),
            NeedRec {
                project: project.to_string(),
                goal: goal.to_string(),
                acceptance: acceptance.to_vec(),
            },
        ));
        inner.state.goals.insert(
            root.clone(),
            GoalRec {
                goal: Goal {
                    id: root.clone(),
                    text: goal.to_string(),
                    leaf: false,
                    ..Goal::default()
                },
                owner: Fellow::Foreman,
                status: GoalStatus::Pending,
                retired: None,
                blocked_reason: None,
                cost_mb: 0,
            },
        );
        inner.state.dirty.meta = true;
        inner.state.dirty.goals.insert(root.clone());
        self.flush(inner)?;
        Ok(root)
    }

    /// Validates `chart` as `fellow`'s own part and stores it, replacing that fellow's
    /// previous chart. A refuted goal's fellow spends one re-chart attempt per accepted chart.
    pub fn submit_chart(&self, fellow: Fellow, chart: Chart) -> Result<(), Vec<Violation>> {
        let mut g = self.lock();
        let inner = &mut *g;
        inner.history.push(HouseCall::SubmitChart {
            fellow,
            chart: chart.clone(),
        });
        let first = State::need_id(1);
        if inner.state.needs.is_empty() {
            return Err(rule0(
                fellow,
                first,
                "no need has been stated; call need() first",
            ));
        }
        if chart.fellow != fellow {
            return Err(rule0(
                fellow,
                first,
                format!("chart is signed {} but submitted as {fellow}", chart.fellow),
            ));
        }
        let Some(idx) = inner
            .state
            .needs
            .iter()
            .position(|n| n.id.as_str() == chart.need)
        else {
            let known: Vec<&str> = inner.state.needs.iter().map(|n| n.id.as_str()).collect();
            return Err(rule0(
                fellow,
                first,
                format!(
                    "chart answers need {}, which does not exist; needs are {}",
                    chart.need,
                    known.join(", ")
                ),
            ));
        };
        let root = inner.state.needs[idx].id.clone();
        let mut violations: Vec<Violation> = chart
            .goals
            .iter()
            .filter(|g| g.id.need_ordinal() != idx + 1)
            .map(|g| Violation {
                rule: 0,
                fellow,
                goal: g.id.clone(),
                message: format!("goal id {} is outside the namespace of need {root}", g.id),
            })
            .collect();
        violations.extend(validate_chart(&chart, &self.cfg));
        if !violations.is_empty() {
            *inner.state.needs[idx].rejections.entry(fellow).or_default() += 1;
            inner.state.dirty.meta = true;
            self.flush(inner)
                .map_err(|e| rule0(fellow, root.clone(), e.to_string()))?;
            return Err(violations);
        }
        let event = json!({
            "t": "chart",
            "agent": fellow.name(),
            "goals": chart.goals.iter().map(|g| json!({
                "id": g.id.as_str(),
                "text": g.text,
                "parent": g.parent.as_ref().or(g.guards.as_ref()).or(g.alt_of.as_ref()).map(GoalId::as_str),
            })).collect::<Vec<_>>(),
        });
        inner.state.needs[idx].charts.insert(fellow, chart);
        for p in inner
            .state
            .pending
            .iter_mut()
            .filter(|(id, p)| id.need_ordinal() == idx + 1 && p.fellow == fellow)
            .map(|(_, p)| p)
        {
            p.attempts = p.attempts.saturating_add(1);
        }
        inner.state.dirty.meta = true;
        self.flush(inner)
            .map_err(|e| rule0(fellow, root, e.to_string()))?;
        self.sink.emit(event);
        Ok(())
    }

    fn merge_inner(
        &self,
        inner: &mut Inner,
        sel: Option<&GoalId>,
    ) -> Result<MergeReport, Vec<Violation>> {
        let idx = inner
            .state
            .target(sel)
            .map_err(|e| rule0(Fellow::Foreman, State::need_id(1), e.to_string()))?;
        let ns = idx + 1;
        let need = inner.state.needs[idx].clone();
        let root = need.id.clone();
        let old: BTreeMap<GoalId, GoalRec> = inner
            .state
            .goals
            .iter()
            .filter(|(id, _)| id.need_ordinal() == ns)
            .map(|(id, r)| (id.clone(), r.clone()))
            .collect();
        let old_pending: BTreeMap<GoalId, Pending> = inner
            .state
            .pending
            .iter()
            .filter(|(id, _)| id.need_ordinal() == ns)
            .map(|(id, p)| (id.clone(), p.clone()))
            .collect();
        let result = run_merge(&MergeInput {
            root: &root,
            need: &need.rec,
            charts: &need.charts,
            facts: &inner.state.facts,
            old: &old,
            old_pending: &old_pending,
            cfg: &self.cfg,
        })
        .and_then(|out| {
            let clashes: Vec<Violation> = out
                .goals
                .iter()
                .filter_map(|(id, r)| {
                    let name = &r.goal.test.as_ref()?.name;
                    let bound = inner.state.tests.get(name)?;
                    (bound.goal.need_ordinal() != ns).then(|| Violation {
                        rule: 8,
                        fellow: r.owner,
                        goal: id.clone(),
                        message: format!(
                            "test name `{name}` is already bound to {} in another need",
                            bound.goal
                        ),
                    })
                })
                .collect();
            if clashes.is_empty() {
                Ok(out)
            } else {
                Err(clashes)
            }
        });
        let out = match result {
            Ok(o) => o,
            Err(vs) => {
                let mut blamed: Vec<Fellow> = vs.iter().map(|v| v.fellow).collect();
                blamed.sort();
                blamed.dedup();
                for f in blamed {
                    *inner.state.needs[idx].rejections.entry(f).or_default() += 1;
                }
                inner.state.dirty.meta = true;
                self.flush(inner)
                    .map_err(|e| rule0(Fellow::Foreman, root.clone(), e.to_string()))?;
                return Err(vs);
            }
        };
        let mut events: Vec<Value> = Vec::new();

        // board events for what this merge decided
        let facts: BTreeMap<&GoalId, &Fact> = inner
            .state
            .facts
            .iter()
            .filter_map(|f| f.refutes.as_ref().map(|r| (r, f)))
            .collect();
        for (id, rec) in &out.goals {
            let before = old.get(id);
            let was_retired = before.is_some_and(|b| b.status == GoalStatus::Retired);
            let was_refuted_block =
                before.is_some_and(|b| b.blocked_reason.as_deref() == Some("refuted"));
            let refuted_here = facts.contains_key(id);
            if refuted_here && !was_retired && !was_refuted_block {
                events.push(
                    json!({"t":"theory","id":id.as_str(),"text":rec.goal.text,"status":"dead"}),
                );
            }
            if rec.status == GoalStatus::Retired && !was_retired {
                if let Some(r) = &rec.retired {
                    match r.reason.as_str() {
                        "cost" => {
                            events.push(json!({"t":"theory","id":id.as_str(),"text":rec.goal.text,"status":"dead"}));
                            events.push(json!({
                                "t":"route","theory":id.as_str(),"shape":"reprice","to":r.by,
                                "because":format!("cost: {} is cheaper under tokens + secs x {}", r.by, self.cfg.alt_secs_weight),
                            }));
                        }
                        "refuted" if refuted_here => {
                            let to = out
                                .alt_groups
                                .values()
                                .find(|g| g.members.contains(id))
                                .and_then(|g| g.winner.clone())
                                .or_else(|| {
                                    out.replaced
                                        .iter()
                                        .find(|(r, _)| r == id)
                                        .map(|(_, n)| n.clone())
                                });
                            if let Some(to) = to {
                                events.push(json!({
                                    "t":"route","theory":id.as_str(),"shape":"reroute","to":to.as_str(),
                                    "because":format!("refuted by {}", r.by),
                                }));
                            }
                        }
                        _ => {}
                    }
                }
                events.push(goal_event(id, rec, "retired"));
            } else if rec.status == GoalStatus::Blocked
                && rec.blocked_reason.as_deref() == Some("refuted")
                && !was_refuted_block
            {
                events.push(goal_event(id, rec, "blocked"));
            }
        }

        inner.state.goals.retain(|id, _| id.need_ordinal() != ns);
        inner.state.goals.extend(out.goals);
        inner.state.alt_groups.retain(|id, _| id.need_ordinal() != ns);
        inner.state.alt_groups.extend(out.alt_groups);
        inner.state.pending.retain(|id, _| id.need_ordinal() != ns);
        inner.state.pending.extend(out.pending);
        inner.state.leap.retain(|id| id.need_ordinal() != ns);
        inner.state.leap.extend(out.leap);
        let slot = &mut inner.state.needs[idx];
        slot.merged = true;
        slot.merge_stale = false;
        slot.rejections.clear();

        // every leaf (and integration parent) test gets a record, keeping any history
        let specs: Vec<(TestSpec, GoalId)> = inner
            .state
            .goals
            .iter()
            .filter(|(id, _)| id.need_ordinal() == ns)
            .filter_map(|(id, r)| r.goal.test.clone().map(|t| (t, id.clone())))
            .collect();
        for (spec, goal) in specs {
            match inner.state.tests.get_mut(&spec.name) {
                Some(t) => {
                    t.spec = spec;
                    t.goal = goal;
                }
                None => {
                    inner.state.tests.insert(
                        spec.name.clone(),
                        TestRec {
                            spec,
                            goal,
                            covers: Default::default(),
                            quarantined: false,
                            runs: Vec::new(),
                        },
                    );
                }
            }
        }
        for id in inner.state.resolve_parents() {
            if let Some(r) = inner.state.goals.get(&id) {
                events.push(goal_event_with(
                    &id,
                    r,
                    "reached",
                    evidence_test(&inner.state, &id),
                ));
            }
        }
        inner.state.dirty.structure = true;
        inner.state.dirty.meta = true;
        self.flush(inner)
            .map_err(|e| rule0(Fellow::Foreman, root, e.to_string()))?;
        self.emit_all(events);
        Ok(out.report)
    }

    /// Merges the stored charts (backbone, guards, ALT groups, refutations), validates the
    /// effective tree and stores it. Reached goals stay reached across re-merges.
    pub fn merge(&self) -> Result<MergeReport, Vec<Violation>> {
        self.merge_in(None)
    }

    #[allow(missing_docs)]
    pub fn merge_in(&self, need: Option<&GoalId>) -> Result<MergeReport, Vec<Violation>> {
        let mut g = self.lock();
        g.history.push(HouseCall::Merge {
            need: need.cloned(),
        });
        self.merge_inner(&mut g, need)
    }

    /// Records a verified fact. A fact that `refutes` a goal takes effect immediately when a
    /// merged tree exists; if the re-merge fails validation the fact is kept and the next
    /// `merge()` reports why (`status().merge_stale` is `true` meanwhile).
    pub fn add_fact(&self, fact: Fact) -> Result<(), HouseError> {
        self.add_fact_in(None, fact)
    }

    #[allow(missing_docs)]
    pub fn add_fact_in(&self, need: Option<&GoalId>, fact: Fact) -> Result<(), HouseError> {
        let mut g = self.lock();
        let inner = &mut *g;
        inner.history.push(HouseCall::AddFact {
            fact: fact.clone(),
            need: need.cloned(),
        });
        if need.is_some() {
            inner.state.target(need)?;
        }
        if let Some(r) = &fact.refutes {
            inner.state.owns(need, r)?;
        }
        if inner.state.facts.iter().any(|f| f.id == fact.id) {
            return Err(HouseError::Invalid(format!(
                "fact {} already recorded",
                fact.id
            )));
        }
        let mut ev = json!({
            "t":"fact","id":fact.id,"by":fact.by,"source":fact.source,"text":fact.text,
        });
        if let Some(r) = &fact.refutes {
            ev["refutes"] = json!(r.as_str());
        }
        inner.state.dirty.facts.push(fact.id.clone());
        inner.state.facts.push(fact);
        inner.state.dirty.meta = true;
        self.flush(inner)?;
        self.sink.emit(ev);
        let merged = inner
            .state
            .target(need)
            .ok()
            .filter(|i| inner.state.needs[*i].merged);
        if let Some(idx) = merged {
            if self.merge_inner(inner, need).is_err() {
                inner.state.needs[idx].merge_stale = true;
                inner.state.dirty.meta = true;
                self.flush(inner)?;
            }
        }
        Ok(())
    }

    /// The next batch: ready goals that fit the governor and have no file conflicts.
    /// Dispatched goals become `Active`.
    pub fn next(&self) -> Result<Batch, HouseError> {
        self.next_in(None)
    }

    #[allow(missing_docs)]
    pub fn next_in(&self, need: Option<&GoalId>) -> Result<Batch, HouseError> {
        let mut g = self.lock();
        let inner = &mut *g;
        inner.history.push(HouseCall::Next {
            need: need.cloned(),
        });
        let idx = inner.state.target(need)?;
        if !inner.state.needs[idx].merged {
            return Err(HouseError::NotMerged);
        }
        let root = inner.state.needs[idx].id.clone();
        let need = inner.state.needs[idx].rec.clone();
        let snap = self.probe.snapshot();
        let plan = sched::plan_for(&inner.state, snap, &self.cfg, Some(idx + 1));
        let mut dispatches = Vec::new();
        let mut events = Vec::new();
        for p in &plan.planned {
            let Some(rec) = inner.state.goals.get(&p.goal) else {
                continue;
            };
            let Some(test) = rec.goal.test.clone() else {
                continue;
            };
            let tier = rec.goal.tier.unwrap_or(Tier::Haiku);
            let model = self.cfg.models.for_tier(tier).to_string();
            let context: Vec<String> = inner
                .state
                .ancestors(&p.goal)
                .iter()
                .filter(|a| **a != root)
                .filter_map(|a| inner.state.goals.get(a).map(|r| r.goal.text.clone()))
                .collect();
            let env: BTreeMap<String, String> = BTreeMap::from([(
                "CARGO_BUILD_JOBS".to_string(),
                self.cfg.cargo_jobs.to_string(),
            )]);
            dispatches.push(Dispatch {
                goal: p.goal.clone(),
                model: model.clone(),
                role: tier.role().to_string(),
                prompt_pack: PromptPack {
                    need: need.goal.clone(),
                    acceptance: need.acceptance.clone(),
                    text: rec.goal.text.clone(),
                    context,
                    reads: rec.goal.reads.clone(),
                    writes: rec.goal.writes.clone(),
                    creates: rec.goal.creates.clone(),
                    class: rec.goal.class.clone(),
                    guards: rec.goal.guards.clone(),
                },
                limits: Limits {
                    env,
                    command: wrap_command(&test.cmd, self.cfg.cargo_jobs),
                },
                test,
                cost_mb: p.cost_mb,
                class: p.class,
                integration: p.integration,
            });
            let task = format!("{}: {}", p.goal, rec.goal.text);
            if tier == Tier::Haiku {
                events.push(
                    json!({"t":"nurse","agent":rec.owner.name(),"n":1,"model":model,"task":task}),
                );
            } else {
                events.push(
                    json!({"t":"dispatch","agent":rec.owner.name(),"model":model,"task":task}),
                );
            }
            events.push(goal_event(&p.goal, rec, "active"));
        }
        for p in &plan.planned {
            if let Some(rec) = inner.state.goals.get_mut(&p.goal) {
                rec.status = GoalStatus::Active;
                rec.cost_mb = p.cost_mb;
                inner.state.dirty.goals.insert(p.goal.clone());
            }
        }
        inner.decisions.push(DecisionRecord {
            call_index: inner.decisions.len() as u64,
            probe: snap,
            dispatched: dispatches
                .iter()
                .map(|d| DispatchRecord {
                    goal: d.goal.clone(),
                    model: d.model.clone(),
                })
                .collect(),
            deferrals: plan.report.deferrals.clone(),
        });
        self.flush(inner)?;
        self.emit_all(events);
        let complete = inner
            .state
            .goals
            .get(&root)
            .is_some_and(|r| r.status == GoalStatus::Reached);
        Ok(Batch {
            dispatches,
            governor: plan.report,
            complete,
        })
    }

    /// Reports a goal's status. `Reached` is accepted only with a bound test whose first
    /// recorded run was RED, whose latest run is GREEN, and which is not quarantined.
    pub fn report(
        &self,
        goal: &GoalId,
        status: GoalStatus,
        evidence: TestEvidence,
    ) -> Result<ReportOutcome, HouseError> {
        self.report_in(None, goal, status, evidence)
    }

    #[allow(missing_docs)]
    pub fn report_in(
        &self,
        need: Option<&GoalId>,
        goal: &GoalId,
        status: GoalStatus,
        evidence: TestEvidence,
    ) -> Result<ReportOutcome, HouseError> {
        let mut g = self.lock();
        let inner = &mut *g;
        inner.history.push(HouseCall::Report {
            goal: goal.clone(),
            status,
            evidence: evidence.clone(),
            need: need.cloned(),
        });
        inner.state.owns(need, goal)?;
        let rec = inner
            .state
            .goals
            .get(goal)
            .cloned()
            .ok_or_else(|| HouseError::UnknownGoal(goal.clone()))?;
        let out = |outcome: Outcome, status: GoalStatus, reason: &str, resolved: Vec<GoalId>| {
            Ok(ReportOutcome {
                outcome,
                status,
                reason: reason.to_string(),
                resolved,
            })
        };
        if rec.status == GoalStatus::Retired {
            return out(Outcome::Rejected, rec.status, "goal is retired", vec![]);
        }
        match status {
            GoalStatus::Reached => self.report_reached(inner, goal, rec, &evidence),
            GoalStatus::Blocked => {
                let r = inner
                    .state
                    .goals
                    .get_mut(goal)
                    .ok_or_else(|| HouseError::UnknownGoal(goal.clone()))?;
                r.status = GoalStatus::Blocked;
                r.blocked_reason = Some("reported".into());
                let ev = goal_event(goal, r, "blocked");
                inner.state.dirty.goals.insert(goal.clone());
                self.flush(inner)?;
                self.sink.emit(ev);
                out(Outcome::Recorded, GoalStatus::Blocked, "", vec![])
            }
            GoalStatus::Pending => {
                if !matches!(rec.status, GoalStatus::Active | GoalStatus::Blocked) {
                    return out(
                        Outcome::Rejected,
                        rec.status,
                        "only an active or blocked goal can be re-queued",
                        vec![],
                    );
                }
                let r = inner
                    .state
                    .goals
                    .get_mut(goal)
                    .ok_or_else(|| HouseError::UnknownGoal(goal.clone()))?;
                if r.blocked_reason.as_deref() == Some("refuted") {
                    return out(
                        Outcome::Rejected,
                        rec.status,
                        "a refuted goal needs a re-chart",
                        vec![],
                    );
                }
                r.status = GoalStatus::Pending;
                r.blocked_reason = None;
                inner.state.dirty.goals.insert(goal.clone());
                self.flush(inner)?;
                out(Outcome::Recorded, GoalStatus::Pending, "", vec![])
            }
            GoalStatus::Active => out(
                Outcome::Rejected,
                rec.status,
                "goals become active through next()",
                vec![],
            ),
            GoalStatus::Retired => out(
                Outcome::Rejected,
                rec.status,
                "retirement is decided by merge()",
                vec![],
            ),
        }
    }

    fn report_reached(
        &self,
        inner: &mut Inner,
        goal: &GoalId,
        rec: GoalRec,
        evidence: &TestEvidence,
    ) -> Result<ReportOutcome, HouseError> {
        let out = |outcome: Outcome, status: GoalStatus, reason: String, resolved: Vec<GoalId>| {
            Ok(ReportOutcome {
                outcome,
                status,
                reason,
                resolved,
            })
        };
        let is_leaf = rec.goal.leaf;
        if rec.status == GoalStatus::Reached {
            return out(
                Outcome::Rejected,
                rec.status,
                "already reached".into(),
                vec![],
            );
        }
        if is_leaf {
            if rec.status != GoalStatus::Active {
                return out(
                    Outcome::Rejected,
                    rec.status,
                    format!(
                        "goal is {}, not active: it was never dispatched",
                        sched::status_name(rec.status)
                    ),
                    vec![],
                );
            }
        } else {
            if rec.goal.test.is_none() {
                return out(
                    Outcome::Rejected,
                    rec.status,
                    "a parent resolves by itself when its children are done".into(),
                    vec![],
                );
            }
            let kids = inner.state.children(goal);
            if kids.is_empty() || !kids.iter().all(|k| State::is_done(k)) {
                return out(
                    Outcome::Rejected,
                    rec.status,
                    "children are not all reached".into(),
                    vec![],
                );
            }
            if rec.status != GoalStatus::Active && rec.status != GoalStatus::Pending {
                return out(
                    Outcome::Rejected,
                    rec.status,
                    "goal cannot be reported now".into(),
                    vec![],
                );
            }
        }
        let unproven = |reason: String| out(Outcome::Unproven, rec.status, reason, vec![]);
        let Some(spec) = rec.goal.test.as_ref() else {
            return unproven("goal has no bound test".into());
        };
        if evidence.test != spec.name {
            return unproven(format!(
                "evidence is for `{}`, the bound test is `{}`",
                evidence.test, spec.name
            ));
        }
        if evidence.status != TestStatus::Green {
            return unproven("evidence status is not green".into());
        }
        let Some(t) = inner.state.tests.get(&spec.name) else {
            return unproven(format!("no record of test `{}`", spec.name));
        };
        if t.quarantined {
            return unproven("test is quarantined by the flake guard".into());
        }
        let (Some(first), Some(last)) = (t.runs.first(), t.runs.last()) else {
            return unproven("no recorded run of the bound test".into());
        };
        if is_leaf && first.status != TestStatus::Red {
            return unproven("the first recorded run was not RED (RED-first)".into());
        }
        if last.status != TestStatus::Green {
            return unproven("the latest recorded run is not green".into());
        }
        let r = inner
            .state
            .goals
            .get_mut(goal)
            .ok_or_else(|| HouseError::UnknownGoal(goal.clone()))?;
        r.status = GoalStatus::Reached;
        let mut events = vec![match inner.state.goals.get(goal) {
            Some(r) if is_leaf => goal_event(goal, r, "reached"),
            Some(r) => goal_event_with(goal, r, "reached", evidence_test(&inner.state, goal)),
            None => return Err(HouseError::UnknownGoal(goal.clone())),
        }];
        inner.state.dirty.goals.insert(goal.clone());
        let resolved = inner.state.resolve_parents();
        for id in &resolved {
            if let Some(p) = inner.state.goals.get(id) {
                events.push(goal_event_with(
                    id,
                    p,
                    "reached",
                    evidence_test(&inner.state, id),
                ));
            }
        }
        let all_done = inner
            .state
            .goals
            .get(&State::need_id(goal.need_ordinal()))
            .is_some_and(|r| r.status == GoalStatus::Reached);
        if all_done {
            let owner = inner.state.needs.get(goal.need_ordinal() - 1);
            for f in owner.into_iter().flat_map(|n| n.charts.keys()) {
                events.push(json!({"t":"done","agent":f.name()}));
            }
        }
        self.flush(inner)?;
        self.emit_all(events);
        out(
            Outcome::Accepted,
            GoalStatus::Reached,
            String::new(),
            resolved,
        )
    }

    /// Records one run of a bound test and learns from it: COVERS edges from
    /// `covered_files`, the result-cache key, the flake guard and a `CostObs`.
    pub fn record_test_run(
        &self,
        test: &str,
        status: TestStatus,
        secs: f64,
        peak_ram_mb: u64,
        covered_files: &[String],
    ) -> Result<(), HouseError> {
        self.record_test_run_in(None, test, status, secs, peak_ram_mb, covered_files)
    }

    #[allow(missing_docs)]
    pub fn record_test_run_in(
        &self,
        need: Option<&GoalId>,
        test: &str,
        status: TestStatus,
        secs: f64,
        peak_ram_mb: u64,
        covered_files: &[String],
    ) -> Result<(), HouseError> {
        self.record_run(need, test, status, secs, peak_ram_mb, covered_files, None)
    }

    #[allow(clippy::too_many_arguments)]
    fn record_run(
        &self,
        need: Option<&GoalId>,
        test: &str,
        status: TestStatus,
        secs: f64,
        peak_ram_mb: u64,
        covered_files: &[String],
        forced_key: Option<String>,
    ) -> Result<(), HouseError> {
        let mut g = self.lock();
        let inner = &mut *g;
        let slot = inner.history.len();
        inner.history.push(HouseCall::RecordTestRun {
            need: need.cloned(),
            test: test.to_string(),
            status,
            secs,
            peak_ram_mb,
            covered_files: covered_files.to_vec(),
            key: None,
        });
        if let Some(bound) = inner.state.tests.get(test) {
            inner.state.owns(need, &bound.goal)?;
        }
        let seq = inner.state.next_seq();
        let st = &mut inner.state;
        let t = st
            .tests
            .get_mut(test)
            .ok_or_else(|| HouseError::UnknownTest(test.to_string()))?;
        for f in covered_files {
            if t.covers.insert(f.clone()) {
                st.dirty.covers.push((test.to_string(), f.clone()));
            }
        }
        for f in covered_files {
            if !st.dirty.covers.iter().any(|(n, x)| n == test && x == f) {
                st.dirty.covers.push((test.to_string(), f.clone()));
            }
        }
        let key = forced_key
            .unwrap_or_else(|| smart::cache_key(&t.spec.cmd, &t.covers, &self.cfg.project_root));
        if let Some(HouseCall::RecordTestRun { key: slot_key, .. }) = inner.history.get_mut(slot) {
            *slot_key = Some(key.clone());
        }
        let mut quarantined_now = false;
        if !t.covers.is_empty() && smart::flips(&t.runs, &key, status) && !t.quarantined {
            t.quarantined = true;
            quarantined_now = true;
        }
        let run = RunRec {
            key,
            status,
            secs,
            ram_mb: peak_ram_mb,
            seq,
        };
        t.runs.push(run.clone());
        let class = JobClass::parse(&t.spec.class).unwrap_or(JobClass::Llm);
        let goal = t.goal.clone();
        st.dirty.runs.push((test.to_string(), run));
        st.dirty.tests.insert(test.to_string());
        if peak_ram_mb > 0 && class != JobClass::Llm {
            st.cost.record(class, peak_ram_mb, self.cfg.cost_window);
            st.dirty.cost.push((class, peak_ram_mb, seq));
        }
        st.dirty.meta = true;
        let agent = st.goals.get(&goal).map_or("House", |r| r.owner.name());
        let mut ev = json!({"t":"test","agent":agent,"status":status.name(),"name":test});
        if quarantined_now {
            ev["quarantined"] = json!(true);
        }
        self.flush(inner)?;
        self.sink.emit(ev);
        Ok(())
    }

    /// A JSON snapshot for the board and the MCP `house_status` tool.
    pub fn status(&self) -> Result<Value, HouseError> {
        self.status_in(None)
    }

    #[allow(missing_docs)]
    pub fn status_in(&self, sel: Option<&GoalId>) -> Result<Value, HouseError> {
        let g = self.lock();
        let st = &g.state;
        let idx = match sel {
            None if st.needs.is_empty() => None,
            _ => Some(st.target(sel)?),
        };
        let need = idx.map(|i| &st.needs[i]);
        let ns = idx.map_or(0, |i| i + 1);
        let root = need.map_or_else(|| State::need_id(1), |n| n.id.clone());
        let mine = |id: &GoalId| id.need_ordinal() == ns;
        let mut list = Vec::new();
        for (id, r) in st.goals.iter().filter(|(id, _)| mine(id)) {
            list.push(json!({
                "id": id.as_str(),
                "status": sched::status_name(r.status),
                "leaf": r.goal.leaf,
                "tier": r.goal.tier.map(Tier::model),
                "parent": r.goal.parent.as_ref().map(GoalId::as_str),
                "by": r.owner.name(),
                "text": r.goal.text,
                "retired_reason": r.retired.as_ref().map(|x| x.reason.clone()),
                "blocked_reason": r.blocked_reason,
            }));
        }
        let active: Vec<&str> = st
            .goals
            .iter()
            .filter(|(id, r)| mine(id) && r.status == GoalStatus::Active)
            .map(|(id, _)| id.as_str())
            .collect();
        let retired: Vec<Value> = st
            .goals
            .iter()
            .filter(|(id, _)| mine(id))
            .filter_map(|(id, r)| {
                r.retired
                    .as_ref()
                    .map(|x| json!({"goal": id.as_str(), "reason": x.reason, "by": x.by}))
            })
            .collect();
        let pending: Vec<Value> = st
            .pending
            .iter()
            .filter(|(id, _)| mine(id))
            .map(|(id, p)| json!({"goal": id.as_str(), "fellow": p.fellow.name(), "attempts": p.attempts}))
            .collect();
        let leaps: Vec<&str> = st
            .leap
            .iter()
            .filter(|id| mine(id))
            .map(GoalId::as_str)
            .collect();
        let quarantined: Vec<&str> = st
            .tests
            .iter()
            .filter(|(_, t)| t.quarantined && mine(&t.goal))
            .map(|(n, _)| n.as_str())
            .collect();
        let escalated: Vec<&str> = need
            .into_iter()
            .flat_map(|n| n.rejections.iter())
            .filter(|(_, n)| **n >= 3)
            .map(|(f, _)| f.name())
            .collect();
        let alt: Vec<Value> = st
            .alt_groups
            .iter()
            .filter(|(id, _)| mine(id))
            .map(|(t, g)| {
                json!({
                    "target": t.as_str(),
                    "members": g.members.iter().map(GoalId::as_str).collect::<Vec<_>>(),
                    "winner": g.winner.as_ref().map(GoalId::as_str),
                })
            })
            .collect();
        Ok(json!({
            "need": need.map(|n| json!({
                "id": n.id.as_str(), "project": n.rec.project, "goal": n.rec.goal,
                "acceptance": n.rec.acceptance,
            })),
            "merged": need.is_some_and(|n| n.merged),
            "merge_stale": need.is_some_and(|n| n.merge_stale),
            "complete": st.goals.get(&root).is_some_and(|r| r.status == GoalStatus::Reached),
            "goals": goal_counts(st, ns),
            "current": st.target(None).ok().map(|i| st.needs[i].id.as_str()),
            "needs": st.needs.iter().enumerate().map(|(i, n)| json!({
                "id": n.id.as_str(), "project": n.rec.project, "goal": n.rec.goal,
                "current": st.target(None).ok() == Some(i),
                "complete": st.is_complete(i),
                "merged": n.merged,
                "goals": goal_counts(st, i + 1),
            })).collect::<Vec<_>>(),
            "goal_list": list,
            "active": active,
            "retired": retired,
            "pending_rechart": pending,
            "leap_candidate": !leaps.is_empty(),
            "leap_candidates": leaps,
            "quarantined": quarantined,
            "escalated": escalated,
            "alt_groups": alt,
            "facts": st.facts.iter().map(|f| json!({
                "id": f.id, "by": f.by, "refutes": f.refutes.as_ref().map(GoalId::as_str),
                "source": f.source, "text": f.text,
            })).collect::<Vec<_>>(),
        }))
    }

    /// The one Dr House leap for a dead subtree. `Ok(Some)` only when `goal` is a leap
    /// candidate (blocked after its re-charts were spent); `Ok(None)` for any other known goal.
    /// A goal gets one leap: a second call returns [`HouseError::LeapAlreadyUsed`].
    /// The dispatch names the `dr_house` model slot and the `dr_house_fallback` slot to use if
    /// that dispatch fails, so changing the Dr House model is changing one string.
    pub fn leap(
        &self,
        goal: &GoalId,
        cause_of_death: &str,
    ) -> Result<Option<LeapDispatch>, HouseError> {
        self.leap_in(None, goal, cause_of_death)
    }

    #[allow(missing_docs)]
    pub fn leap_in(
        &self,
        need: Option<&GoalId>,
        goal: &GoalId,
        cause_of_death: &str,
    ) -> Result<Option<LeapDispatch>, HouseError> {
        const TIMEOUT_SECS: u64 = 300;
        let mut g = self.lock();
        let inner = &mut *g;
        inner.state.owns(need, goal)?;
        let rec = inner
            .state
            .goals
            .get(goal)
            .ok_or_else(|| HouseError::UnknownGoal(goal.clone()))?;
        if !inner.state.leap.contains(goal) {
            return Ok(None);
        }
        if inner.state.leaps_used.contains(goal) {
            return Err(HouseError::LeapAlreadyUsed(goal.clone()));
        }
        let mut subtree: Vec<GoalId> = vec![goal.clone()];
        subtree.extend(
            inner
                .state
                .goals
                .keys()
                .filter(|id| inner.state.ancestors(id).contains(goal))
                .cloned(),
        );
        let mut input = format!("DEAD THEORY {goal}: {}\n", rec.goal.text);
        let kids: Vec<String> = subtree
            .iter()
            .skip(1)
            .filter_map(|id| {
                inner
                    .state
                    .goals
                    .get(id)
                    .map(|r| format!("- {id}: {}\n", r.goal.text))
            })
            .collect();
        if !kids.is_empty() {
            input.push_str("SUBTREE:\n");
            kids.iter().for_each(|k| input.push_str(k));
        }
        input.push_str(&format!("CAUSE OF DEATH: {cause_of_death}\n"));
        let facts: Vec<String> = inner
            .state
            .facts
            .iter()
            .filter(|f| f.refutes.as_ref().is_some_and(|r| subtree.contains(r)))
            .map(|f| format!("- {} ({}, {}): {}\n", f.id, f.by, f.source, f.text))
            .collect();
        if !facts.is_empty() {
            input.push_str("FACTS:\n");
            facts.iter().for_each(|f| input.push_str(f));
        }
        let leap_id = format!("L{}", inner.state.leaps_used.len() + 1);
        let text = rec.goal.text.clone();
        let dispatch = LeapDispatch {
            goal: goal.clone(),
            model: self.cfg.models.dr_house.clone(),
            fallback_model: self.cfg.models.dr_house_fallback.clone(),
            effort: "max".to_string(),
            timeout_secs: TIMEOUT_SECS,
            nurses: 0,
            input,
        };
        inner.state.leaps_used.insert(goal.clone());
        inner.state.dirty.meta = true;
        self.flush(inner)?;
        self.sink.emit(json!({
            "t": "dispatch", "agent": "DR HOUSE", "model": dispatch.model, "task": "leap",
        }));
        self.sink.emit(json!({
            "t": "leap", "id": leap_id, "goal": goal.as_str(), "text": text,
            "minutes": TIMEOUT_SECS / 60, "model": dispatch.model,
        }));
        Ok(Some(dispatch))
    }
    /// Tests to run after `touched_files` changed, in order: L0 then L1 then (only if
    /// `allow_full_suite`) L2; within a level recently failed first, then cheapest.
    pub fn select_tests(&self, touched_files: &[String]) -> Result<Vec<TestPick>, HouseError> {
        self.select_tests_in(None, touched_files)
    }

    #[allow(missing_docs)]
    pub fn select_tests_in(
        &self,
        need: Option<&GoalId>,
        touched_files: &[String],
    ) -> Result<Vec<TestPick>, HouseError> {
        let g = self.lock();
        let scope = match g.state.target(need) {
            Ok(idx) => Some(idx + 1),
            Err(HouseError::NoNeed) => None,
            Err(e) => return Err(e),
        };
        Ok(smart::select(&g.state, touched_files, &self.cfg, scope))
    }
}
