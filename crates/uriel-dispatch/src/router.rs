use std::collections::{BTreeMap, BTreeSet};

use sha2::{Digest, Sha256};

use crate::config::RouterConfig;
use crate::input::{Effect, Input};
use crate::policy::{changes_for, gate, revalidate, AcceptMode, Policy};
use crate::ticket::{Assignment, InputFail, Phase, Status, Ticket};
use crate::types::*;

const DIGEST_DOMAIN: &[u8] = b"uriel-dispatch/log/v1\0";

#[derive(Debug, Clone)]
struct Run {
    id: EvalId,
    batch: Vec<TriggerId>,
    delivered_at: Option<Ms>,
}

#[derive(Debug, Clone)]
struct Unavail {
    at: Ms,
    batch: Vec<TriggerId>,
    dismissed: bool,
}

#[derive(Debug, Clone, Default)]
struct ThreadState {
    queued: Vec<TriggerId>,
    run: Option<Run>,
    expiries: u8,
    subs: BTreeSet<TicketId>,
    last_activity: Ms,
    unpolled: bool,
    unavail: Option<Unavail>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttentionItem {
    Ticket {
        ticket: TicketId,
        stage: Stage,
        since: Ms,
    },
    EvalUnavailable {
        thread: ThreadId,
        since: Ms,
    },
}

impl AttentionItem {
    fn key(&self) -> (u8, Ms, u64) {
        match self {
            AttentionItem::Ticket {
                ticket,
                stage,
                since,
            } => {
                let rank = match stage {
                    Stage::PersonAnswer => 0,
                    Stage::PersonReview => 1,
                    Stage::PersonBlocked => 2,
                    _ => 3,
                };
                (rank, *since, ticket.0)
            }
            AttentionItem::EvalUnavailable { since, .. } => (4, *since, 0),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ObserveReport {
    pub would_accept: u32,
    pub by_rule: BTreeMap<String, u32>,
    pub accepted: u32,
    pub ignored_from_card: u32,
    pub unavailable: u32,
}

pub struct Router {
    cfg: RouterConfig,
    policy: Policy,
    version: u64,
    now: Ms,
    tickets: BTreeMap<TicketId, Ticket>,
    next_ticket: u64,
    next_eval: u64,
    seq: u64,
    threads: BTreeMap<ThreadId, ThreadState>,
    triggers: BTreeMap<TriggerId, Trigger>,
    accepted: BTreeMap<TriggerId, TicketId>,
    dispositions: BTreeMap<TriggerId, Vec<Disposition>>,
    decisions: Vec<DecisionRecord>,
    reasons: BTreeMap<TriggerId, Vec<String>>,
    ram_slots: Option<u32>,
    window: Option<Ms>,
    window_announced: bool,
    inputs: Vec<(Ms, Input)>,
    digest: [u8; 32],
}

pub fn replay(cfg: RouterConfig, policy: Policy, log: &[(Ms, Input)]) -> Router {
    let mut r = Router::new(cfg, policy);
    for (at, input) in log {
        r.apply(*at, input.clone());
    }
    r
}

impl Router {
    pub fn new(cfg: RouterConfig, policy: Policy) -> Router {
        let mut h = Sha256::new();
        h.update(DIGEST_DOMAIN);
        Router {
            cfg: cfg.clamped(),
            policy,
            version: 1,
            now: 0,
            tickets: BTreeMap::new(),
            next_ticket: 1,
            next_eval: 1,
            seq: 0,
            threads: BTreeMap::new(),
            triggers: BTreeMap::new(),
            accepted: BTreeMap::new(),
            dispositions: BTreeMap::new(),
            decisions: Vec::new(),
            reasons: BTreeMap::new(),
            ram_slots: None,
            window: None,
            window_announced: false,
            inputs: Vec::new(),
            digest: h.finalize().into(),
        }
    }

    pub fn config(&self) -> &RouterConfig {
        &self.cfg
    }

    pub fn policy(&self) -> &Policy {
        &self.policy
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn now(&self) -> Ms {
        self.now
    }

    pub fn ticket(&self, id: TicketId) -> Option<&Ticket> {
        self.tickets.get(&id)
    }

    pub fn tickets(&self) -> impl Iterator<Item = &Ticket> {
        self.tickets.values()
    }

    pub fn stage(&self, id: TicketId) -> Option<Stage> {
        self.tickets.get(&id)?.stage()
    }

    pub fn decisions(&self) -> &[DecisionRecord] {
        &self.decisions
    }

    pub fn dispositions(&self, trigger: &TriggerId) -> &[Disposition] {
        self.dispositions.get(trigger).map_or(&[], Vec::as_slice)
    }

    pub fn trigger_ids(&self) -> impl Iterator<Item = &TriggerId> {
        self.triggers.keys()
    }

    pub fn inputs(&self) -> &[(Ms, Input)] {
        &self.inputs
    }

    pub fn digest(&self) -> [u8; 32] {
        self.digest
    }

    pub fn window_closes(&self) -> Option<Ms> {
        self.window
    }

    pub fn window_open(&self) -> bool {
        self.window.is_none_or(|c| self.now < c)
    }

    pub fn capacity(&self) -> u32 {
        self.cfg
            .max_workers
            .min(self.ram_slots.unwrap_or(self.cfg.max_workers))
    }

    pub fn slots_held(&self) -> (u32, u32) {
        let mut regular = 0;
        let mut reserve = 0;
        for t in self.tickets.values() {
            match t.slot {
                Some(SlotKind::Regular) => regular += 1,
                Some(SlotKind::Reserve) => reserve += 1,
                None => {}
            }
        }
        (regular, reserve)
    }

    pub fn attention(&self) -> Vec<AttentionItem> {
        let mut out: Vec<AttentionItem> = self
            .tickets
            .values()
            .filter_map(|t| {
                let stage = t.stage()?;
                stage.needs_person().then_some(AttentionItem::Ticket {
                    ticket: t.id,
                    stage,
                    since: t.changed_at,
                })
            })
            .collect();
        for (thread, th) in &self.threads {
            let Some(u) = &th.unavail else { continue };
            let fresh = self.now.saturating_sub(u.at) < self.cfg.unavailable_attention_ms;
            if fresh && !u.dismissed && th.run.is_none() && th.queued.is_empty() {
                out.push(AttentionItem::EvalUnavailable {
                    thread: thread.clone(),
                    since: u.at,
                });
            }
        }
        out.sort_by_key(AttentionItem::key);
        out
    }

    pub fn eval_health(&self) -> u32 {
        let n = self
            .threads
            .values()
            .filter(|th| {
                th.unavail.as_ref().is_some_and(|u| {
                    !u.dismissed && self.now.saturating_sub(u.at) < self.cfg.unavailable_health_ms
                })
            })
            .count();
        u32::try_from(n).unwrap_or(u32::MAX)
    }

    pub fn observe_report(&self) -> ObserveReport {
        let mut rep = ObserveReport::default();
        for d in &self.decisions {
            match &d.decision {
                Decision::WouldAccept { rules } => {
                    rep.would_accept += 1;
                    for r in rules {
                        *rep.by_rule.entry(r.clone()).or_default() += 1;
                    }
                }
                Decision::Accept(_) => rep.accepted += 1,
                Decision::Ignore { from_card: true } => rep.ignored_from_card += 1,
                Decision::Unavailable(_) => rep.unavailable += 1,
                _ => {}
            }
        }
        rep
    }

    pub fn apply(&mut self, now: Ms, input: Input) -> Vec<Effect> {
        self.now = self.now.max(now);
        let now = self.now;
        let mut fx = Vec::new();
        self.handle(now, &input, &mut fx);
        self.timers(now, &mut fx);
        self.settle(now, &mut fx);
        let mut h = Sha256::new();
        h.update(self.digest);
        h.update(now.to_le_bytes());
        h.update(serde_json::to_vec(&input).unwrap_or_default());
        h.update(serde_json::to_vec(&fx).unwrap_or_default());
        self.digest = h.finalize().into();
        self.inputs.push((now, input));
        fx
    }

    fn handle(&mut self, now: Ms, input: &Input, fx: &mut Vec<Effect>) {
        match input {
            Input::Tick => {}
            Input::Window { closes } => {
                self.window = *closes;
                self.window_announced = false;
            }
            Input::SetPolicy(p) => self.set_policy(now, p, fx),
            Input::Pressure { free_mb, total_mb } => {
                self.ram_slots = Some(self.cfg.ram_slots(*free_mb, *total_mb));
            }
            Input::Ingest(t) => self.ingest(now, t, fx),
            Input::EvalDelivered { eval } => {
                for th in self.threads.values_mut() {
                    if let Some(run) = th.run.as_mut().filter(|r| r.id == *eval) {
                        run.delivered_at.get_or_insert(now);
                    }
                }
            }
            Input::EvalResult { eval, verdicts } => self.eval_result(now, *eval, verdicts, fx),
            Input::EvalFailed { eval } => {
                if let Some(thread) = self.thread_of_eval(*eval) {
                    self.fail_run(now, &thread, Why::AssessmentFailed, fx);
                } else {
                    reject(fx, "eval_failed", Refusal::StaleEval);
                }
            }
            Input::StopOwned { trigger, ticket } => self.stop_owned(now, trigger, *ticket, fx),
            Input::ManualStop { ticket } => {
                let Some(t) = self.tickets.get(ticket) else {
                    return reject(fx, "manual_stop", Refusal::NoSuchTicket);
                };
                if t.status == Status::Ended(End::Stopped) {
                    return reject(fx, "manual_stop", Refusal::TicketStopped);
                }
                let thread = t.mandate.thread.clone();
                self.end_ticket(now, *ticket, End::Stopped, fx);
                self.record(now, thread, None, Decision::ManualStop(*ticket), fx);
            }
            Input::Reeval { thread } => {
                let Some(th) = self.threads.get_mut(thread) else {
                    return reject(fx, "reeval", Refusal::StaleEval);
                };
                if let Some(u) = th.unavail.take() {
                    for t in u.batch.into_iter().rev() {
                        if !th.queued.contains(&t) {
                            th.queued.insert(0, t);
                        }
                    }
                    self.start_eval(now, thread, fx);
                }
            }
            Input::Dismiss { thread } => {
                if let Some(u) = self
                    .threads
                    .get_mut(thread)
                    .and_then(|th| th.unavail.as_mut())
                {
                    u.dismissed = true;
                }
            }
            Input::PrepareDone { ticket } => {
                let Some(t) = self.tickets.get_mut(ticket) else {
                    return reject(fx, "prepare_done", Refusal::NoSuchTicket);
                };
                if !matches!(t.phase, Phase::Preparing { .. }) {
                    return reject(fx, "prepare_done", Refusal::NoSlot);
                }
                t.prepared = true;
                t.phase = Phase::Turn;
                if let Some(a) = t.assignments.last_mut() {
                    *a = Assignment::Running;
                }
                fx.push(Effect::StartTurn { ticket: *ticket });
            }
            Input::PrepareFailed { ticket } => self.prepare_failed(now, *ticket, fx),
            Input::TurnEnded { ticket, background } => {
                self.turn_ended(now, *ticket, *background, fx)
            }
            Input::BackgroundDone { ticket } => {
                let Some(t) = self.tickets.get_mut(ticket) else {
                    return reject(fx, "background_done", Refusal::NoSuchTicket);
                };
                if matches!(t.phase, Phase::Background { .. }) {
                    t.phase = Phase::Turn;
                    fx.push(Effect::StartTurn { ticket: *ticket });
                }
            }
            Input::Report {
                ticket,
                report,
                attention,
            } => self.report(now, *ticket, *report, attention.clone(), fx),
            Input::Ack { ticket, trigger } => {
                if let Some(t) = self.tickets.get_mut(ticket) {
                    t.owned.remove(trigger);
                }
            }
            Input::HandBack {
                ticket,
                trigger,
                reason,
            } => self.hand_back(now, *ticket, trigger, reason, fx),
            Input::Follow {
                ticket,
                thread,
                kind,
            } => {
                let open = self
                    .tickets
                    .get(ticket)
                    .is_some_and(|t| t.status == Status::Open);
                if !open {
                    return reject(fx, "follow", Refusal::TicketEnded);
                }
                if let Some(t) = self.tickets.get_mut(ticket) {
                    let slot = t.subs.entry(thread.clone()).or_insert(*kind);
                    *slot = (*slot).min(*kind);
                }
                self.threads
                    .entry(thread.clone())
                    .or_default()
                    .subs
                    .insert(*ticket);
            }
            Input::Unfollow { ticket, thread } => self.unfollow(now, *ticket, thread, fx),
            Input::DropThread { thread } => {
                if let Some(th) = self.threads.get_mut(thread) {
                    let subs = std::mem::take(&mut th.subs);
                    for id in subs {
                        if let Some(t) = self.tickets.get_mut(&id) {
                            t.subs.remove(thread);
                        }
                    }
                }
            }
            Input::ThreadClosed { thread } => {
                let ids: Vec<TicketId> = self
                    .threads
                    .get(thread)
                    .map(|th| th.subs.iter().copied().collect())
                    .unwrap_or_default();
                for id in ids {
                    let origin = self.tickets.get(&id).is_some_and(|t| {
                        t.status == Status::Open && t.subs.get(thread) == Some(&SubKind::Origin)
                    });
                    if origin {
                        self.end_ticket(now, id, End::Closed, fx);
                    }
                }
            }
            Input::DeliveryFailed { ticket } => {
                let delay_for = |attempts: u32, cfg: &RouterConfig| cfg.redelivery_delay(attempts);
                let Some(t) = self.tickets.get_mut(ticket) else {
                    return reject(fx, "delivery_failed", Refusal::NoSuchTicket);
                };
                if t.status != Status::Open {
                    return reject(fx, "delivery_failed", Refusal::TicketEnded);
                }
                let attempts = t.input_fail.map_or(1, |f| f.attempts + 1);
                t.input_fail = Some(InputFail {
                    attempts,
                    next_at: now.saturating_add(delay_for(attempts, &self.cfg)),
                });
                t.changed_at = now;
            }
            Input::DeliveryOk { ticket } => {
                if let Some(t) = self.tickets.get_mut(ticket) {
                    t.input_fail = None;
                    t.changed_at = now;
                }
            }
        }
    }

    fn set_policy(&mut self, now: Ms, new: &Policy, fx: &mut Vec<Effect>) {
        if *new == self.policy {
            return;
        }
        let old = std::mem::replace(&mut self.policy, new.clone());
        self.version += 1;
        let threads: Vec<ThreadId> = self.threads.keys().cloned().collect();
        for thread in &threads {
            let Some(run) = self.threads.get_mut(thread).and_then(|th| th.run.take()) else {
                continue;
            };
            for t in &run.batch {
                self.record(
                    now,
                    thread.clone(),
                    Some(t.clone()),
                    Decision::Unavailable(Why::StalePolicy),
                    fx,
                );
            }
            if let Some(th) = self.threads.get_mut(thread) {
                let mut merged = run.batch;
                merged.extend(std::mem::take(&mut th.queued));
                th.queued = merged;
            }
        }
        let live: Vec<TicketId> = self
            .tickets
            .values()
            .filter(|t| t.status == Status::Open)
            .map(|t| t.id)
            .collect();
        for id in live {
            let changes = changes_for(&old, &self.policy, &self.tickets[&id].mandate);
            if !changes.is_empty() {
                fx.push(Effect::PolicyChanged {
                    ticket: id,
                    changes,
                });
            }
        }
        for thread in threads {
            self.start_eval(now, &thread, fx);
        }
    }

    fn ingest(&mut self, now: Ms, t: &Trigger, fx: &mut Vec<Effect>) {
        if self.triggers.contains_key(&t.id) {
            self.dispositions
                .entry(t.id.clone())
                .or_default()
                .push(Disposition::Duplicate);
            return;
        }
        self.triggers.insert(t.id.clone(), t.clone());
        let th = self.threads.entry(t.thread.clone()).or_default();
        th.last_activity = now;
        th.unpolled = false;
        let subs: Vec<TicketId> = th.subs.iter().copied().collect();
        let mut delivered = Vec::new();
        let mut owners = Vec::new();
        for id in subs {
            let Some(tk) = self.tickets.get(&id) else {
                continue;
            };
            let Some(kind) = tk.subs.get(&t.thread).copied() else {
                continue;
            };
            let person_auth = t.is_person() && kind.authority();
            match tk.status {
                Status::Ended(End::Stopped | End::Revoked(_)) => continue,
                Status::Ended(_) => {
                    if !person_auth {
                        continue;
                    }
                    match revalidate(&self.policy, &tk.mandate) {
                        Ok(()) => self.resume(now, id, fx),
                        Err(why) => {
                            self.revoke(now, id, why, fx);
                            continue;
                        }
                    }
                }
                Status::Open => {}
            }
            delivered.push(id);
            self.deliver_input(now, id, t, person_auth, fx);
            if person_auth {
                owners.push(id);
            }
        }
        if !owners.is_empty() {
            self.dispositions
                .entry(t.id.clone())
                .or_default()
                .push(Disposition::FannedOut(delivered));
            fx.push(Effect::FollowUpNotice {
                trigger: t.id.clone(),
                tickets: owners,
            });
            return;
        }
        if !delivered.is_empty() {
            self.dispositions
                .entry(t.id.clone())
                .or_default()
                .push(Disposition::FannedOut(delivered));
        }
        self.route_to_eval(now, &t.id, fx);
    }

    fn deliver_input(
        &mut self,
        now: Ms,
        id: TicketId,
        t: &Trigger,
        person_auth: bool,
        fx: &mut Vec<Effect>,
    ) {
        let seq = self.seq;
        let mut queued = false;
        let Some(tk) = self.tickets.get_mut(&id) else {
            return;
        };
        if person_auth {
            tk.person_since_report = true;
            tk.attention = None;
            tk.owned.insert(t.id.clone());
            tk.changed_at = now;
        }
        fx.push(Effect::Deliver {
            ticket: id,
            trigger: t.id.clone(),
        });
        match tk.phase {
            Phase::Parked => {
                if person_auth || tk.stage() == Some(Stage::Idle) {
                    tk.phase = Phase::Queued {
                        seq,
                        person: person_auth,
                    };
                    queued = true;
                }
            }
            Phase::Queued { seq, person } => {
                tk.phase = Phase::Queued {
                    seq,
                    person: person || person_auth,
                };
            }
            _ => tk.fresh_inputs += 1,
        }
        if queued {
            self.seq += 1;
        }
    }

    fn resume(&mut self, now: Ms, id: TicketId, fx: &mut Vec<Effect>) {
        if let Some(t) = self.tickets.get_mut(&id) {
            t.status = Status::Open;
            t.assignments.push(Assignment::Pending);
            t.phase = Phase::Parked;
            t.changed_at = now;
            fx.push(Effect::Resumed { ticket: id });
        }
    }

    fn revoke(&mut self, now: Ms, id: TicketId, why: Revoke, fx: &mut Vec<Effect>) {
        if let Some(t) = self.tickets.get_mut(&id) {
            t.status = Status::Ended(End::Revoked(why));
            t.changed_at = now;
            fx.push(Effect::Ended {
                ticket: id,
                end: End::Revoked(why),
            });
        }
    }

    fn owned_elsewhere(&self, trigger: &TriggerId, except: TicketId) -> bool {
        self.tickets
            .values()
            .any(|t| t.id != except && t.owned.contains(trigger))
    }

    fn end_ticket(&mut self, now: Ms, id: TicketId, end: End, fx: &mut Vec<Effect>) {
        let Some(t) = self.tickets.get_mut(&id) else {
            return;
        };
        t.status = Status::Ended(end);
        t.slot = None;
        t.phase = Phase::Parked;
        if let Some(a) = t.assignments.last_mut() {
            *a = Assignment::Ended;
        }
        t.attention = None;
        t.input_fail = None;
        t.fresh_inputs = 0;
        t.changed_at = now;
        let owned = std::mem::take(&mut t.owned);
        fx.push(Effect::Ended { ticket: id, end });
        for trig in owned {
            if !self.owned_elsewhere(&trig, id) {
                self.reasons
                    .entry(trig.clone())
                    .or_default()
                    .push("released: ticket ended".into());
                self.route_to_eval(now, &trig, fx);
            }
        }
    }

    fn hand_back(
        &mut self,
        now: Ms,
        id: TicketId,
        trigger: &TriggerId,
        reason: &str,
        fx: &mut Vec<Effect>,
    ) {
        let Some(t) = self.tickets.get_mut(&id) else {
            return reject(fx, "hand_back", Refusal::NoSuchTicket);
        };
        if !t.owned.remove(trigger) {
            return reject(fx, "hand_back", Refusal::NotOwnedByTicket);
        }
        self.reasons
            .entry(trigger.clone())
            .or_default()
            .push(reason.to_string());
        if !self.owned_elsewhere(trigger, id) {
            self.route_to_eval(now, trigger, fx);
        }
    }

    fn unfollow(&mut self, now: Ms, id: TicketId, thread: &ThreadId, fx: &mut Vec<Effect>) {
        let Some(t) = self.tickets.get_mut(&id) else {
            return reject(fx, "unfollow", Refusal::NoSuchTicket);
        };
        match t.subs.get(thread) {
            None => return,
            Some(SubKind::Origin) => return reject(fx, "unfollow", Refusal::OriginPinned),
            Some(_) => {}
        }
        t.subs.remove(thread);
        let mine: Vec<TriggerId> = t
            .owned
            .iter()
            .filter(|tr| self.triggers.get(*tr).is_some_and(|x| x.thread == *thread))
            .cloned()
            .collect();
        if let Some(th) = self.threads.get_mut(thread) {
            th.subs.remove(&id);
        }
        for tr in mine {
            self.hand_back(now, id, &tr, "unrelated thread", fx);
        }
    }

    fn route_to_eval(&mut self, now: Ms, id: &TriggerId, fx: &mut Vec<Effect>) {
        let Some(t) = self.triggers.get(id).cloned() else {
            return;
        };
        match gate(&self.policy, &t, false) {
            Err(r) => self
                .dispositions
                .entry(id.clone())
                .or_default()
                .push(Disposition::Filtered(r)),
            Ok(_) => {
                let th = self.threads.entry(t.thread.clone()).or_default();
                let inflight =
                    th.queued.contains(id) || th.run.as_ref().is_some_and(|r| r.batch.contains(id));
                if !inflight {
                    th.queued.push(id.clone());
                }
                self.start_eval(now, &t.thread, fx);
            }
        }
    }

    fn start_eval(&mut self, _now: Ms, thread: &ThreadId, fx: &mut Vec<Effect>) {
        let Some(th) = self.threads.get_mut(thread) else {
            return;
        };
        if th.run.is_some() || th.queued.is_empty() {
            return;
        }
        let batch = std::mem::take(&mut th.queued);
        let eval = EvalId(self.next_eval);
        let mut kept = Vec::new();
        let mut cands = Vec::new();
        for id in batch {
            let Some(t) = self.triggers.get(&id) else {
                continue;
            };
            match gate(&self.policy, t, false) {
                Ok(adm) => {
                    kept.push(id.clone());
                    cands.push(Candidate {
                        trigger: id.clone(),
                        strength: adm.strength,
                        reasons: self.reasons.get(&id).cloned().unwrap_or_default(),
                    });
                }
                Err(r) => self
                    .dispositions
                    .entry(id)
                    .or_default()
                    .push(Disposition::Filtered(r)),
            }
        }
        if kept.is_empty() {
            return;
        }
        self.next_eval += 1;
        for id in &kept {
            self.dispositions
                .entry(id.clone())
                .or_default()
                .push(Disposition::Evaluated(eval));
        }
        if let Some(th) = self.threads.get_mut(thread) {
            th.run = Some(Run {
                id: eval,
                batch: kept,
                delivered_at: None,
            });
        }
        fx.push(Effect::StartEval {
            eval,
            thread: thread.clone(),
            version: self.version,
            batch: cands,
        });
    }

    fn thread_of_eval(&self, eval: EvalId) -> Option<ThreadId> {
        self.threads
            .iter()
            .find(|(_, th)| th.run.as_ref().is_some_and(|r| r.id == eval))
            .map(|(k, _)| k.clone())
    }

    fn fail_run(&mut self, now: Ms, thread: &ThreadId, why: Why, fx: &mut Vec<Effect>) {
        let Some(run) = self.threads.get_mut(thread).and_then(|th| th.run.take()) else {
            return;
        };
        for t in &run.batch {
            self.record(
                now,
                thread.clone(),
                Some(t.clone()),
                Decision::Unavailable(why.clone()),
                fx,
            );
        }
        if let Some(th) = self.threads.get_mut(thread) {
            th.expiries = 0;
            th.unavail = Some(Unavail {
                at: now,
                batch: run.batch,
                dismissed: false,
            });
        }
        self.start_eval(now, thread, fx);
    }

    fn record(
        &mut self,
        now: Ms,
        thread: ThreadId,
        trigger: Option<TriggerId>,
        decision: Decision,
        fx: &mut Vec<Effect>,
    ) {
        let rec = DecisionRecord {
            at: now,
            thread: thread.clone(),
            trigger,
            version: self.version,
            decision,
        };
        if !matches!(rec.decision, Decision::Unavailable(_)) {
            if let Some(th) = self.threads.get_mut(&thread) {
                th.unavail = None;
            }
        }
        self.decisions.push(rec.clone());
        fx.push(Effect::Decided(rec));
    }

    fn eval_result(
        &mut self,
        now: Ms,
        eval: EvalId,
        verdicts: &[(TriggerId, Verdict)],
        fx: &mut Vec<Effect>,
    ) {
        let Some(thread) = self.thread_of_eval(eval) else {
            return reject(fx, "eval_result", Refusal::StaleEval);
        };
        let Some(run) = self.threads.get_mut(&thread).and_then(|th| th.run.take()) else {
            return;
        };
        if let Some(th) = self.threads.get_mut(&thread) {
            th.expiries = 0;
        }
        for trigger in &run.batch {
            let verdict = verdicts.iter().find(|(t, _)| t == trigger).map(|(_, v)| v);
            let decision = match verdict {
                None => Decision::Ignore { from_card: false },
                Some(v) => self.judge(now, trigger, v, fx),
            };
            self.record(now, thread.clone(), Some(trigger.clone()), decision, fx);
        }
        self.start_eval(now, &thread, fx);
    }

    fn judge(
        &mut self,
        now: Ms,
        trigger: &TriggerId,
        v: &Verdict,
        fx: &mut Vec<Effect>,
    ) -> Decision {
        let Some(t) = self.triggers.get(trigger).cloned() else {
            return Decision::Refused(Refusal::StaleEval);
        };
        match v {
            Verdict::Ignore { from_card } => Decision::Ignore {
                from_card: *from_card,
            },
            Verdict::Stop { ticket } => self.stop_decision(now, &t, *ticket, fx),
            Verdict::Attach { ticket } => self.attach(now, &t, *ticket, fx),
            Verdict::Accept {
                scope,
                rules,
                assessed,
            } => {
                if let Some(id) = self.accepted.get(trigger) {
                    return Decision::Accept(*id);
                }
                if self.tickets.values().any(|k| k.owned.contains(trigger)) {
                    return Decision::Refused(Refusal::TicketOwned);
                }
                let adm = match gate(&self.policy, &t, false) {
                    Ok(a) => a,
                    Err(r) => return Decision::Refused(r),
                };
                if let Some(r) = rules.iter().find_map(|r| match self.policy.rules.get(r) {
                    None => Some(Refusal::UnknownRule),
                    Some(x) if !x.enabled => Some(Refusal::DisabledRule),
                    Some(_) => None,
                }) {
                    return Decision::Refused(r);
                }
                if adm.strength == Strength::Inferred {
                    if !assessed {
                        return Decision::Refused(Refusal::NotExplicit);
                    }
                    if rules.is_empty() {
                        return Decision::Refused(Refusal::NoMatchingRule);
                    }
                    match self.policy.mode {
                        AcceptMode::Off => return Decision::Refused(Refusal::ModeOff),
                        AcceptMode::Observe => {
                            return Decision::WouldAccept {
                                rules: rules.clone(),
                            };
                        }
                        AcceptMode::Auto => {}
                    }
                }
                let mandate = Mandate {
                    policy_version: self.version,
                    scope: scope.clone(),
                    strength: adm.strength,
                    rules: rules.clone(),
                    sender: t.actor.as_ref().map(|a| a.id.clone()),
                    source: t.source.clone(),
                    receiving_scope: t.scope.clone(),
                    thread: t.thread.clone(),
                    basis: adm.basis,
                };
                Decision::Accept(self.open_ticket(now, &t, mandate, fx))
            }
        }
    }

    fn open_ticket(
        &mut self,
        now: Ms,
        t: &Trigger,
        mandate: Mandate,
        fx: &mut Vec<Effect>,
    ) -> TicketId {
        let id = TicketId(self.next_ticket);
        self.next_ticket += 1;
        let mut subs = BTreeMap::new();
        subs.insert(t.thread.clone(), SubKind::Origin);
        let tk = Ticket {
            id,
            mandate,
            status: Status::Open,
            subs,
            phase: Phase::Queued {
                seq: self.seq,
                person: false,
            },
            slot: None,
            assignments: vec![Assignment::Pending],
            latest: None,
            attention: None,
            person_since_report: false,
            fresh_inputs: 0,
            owned: BTreeSet::new(),
            input_fail: None,
            prepared: false,
            opened_at: now,
            changed_at: now,
        };
        self.seq += 1;
        self.tickets.insert(id, tk);
        self.threads
            .entry(t.thread.clone())
            .or_default()
            .subs
            .insert(id);
        self.accepted.insert(t.id.clone(), id);
        fx.push(Effect::TicketOpened {
            ticket: id,
            trigger: t.id.clone(),
        });
        id
    }

    fn attach(&mut self, now: Ms, t: &Trigger, id: TicketId, fx: &mut Vec<Effect>) -> Decision {
        let Some(tk) = self.tickets.get(&id) else {
            return Decision::Refused(Refusal::NoSuchTicket);
        };
        match tk.status {
            Status::Ended(End::Stopped) => return Decision::Refused(Refusal::TicketStopped),
            Status::Ended(End::Revoked(_)) => return Decision::Refused(Refusal::TicketRevoked),
            Status::Ended(_) => {
                if !t.is_person() {
                    return Decision::Refused(Refusal::TicketEnded);
                }
                if let Err(why) = revalidate(&self.policy, &tk.mandate) {
                    self.revoke(now, id, why, fx);
                    return Decision::Refused(Refusal::TicketRevoked);
                }
                self.resume(now, id, fx);
            }
            Status::Open => {}
        }
        if let Some(tk) = self.tickets.get_mut(&id) {
            tk.subs.entry(t.thread.clone()).or_insert(SubKind::Attached);
        }
        self.threads
            .entry(t.thread.clone())
            .or_default()
            .subs
            .insert(id);
        self.deliver_input(now, id, t, t.is_person(), fx);
        Decision::Attach(id)
    }

    fn stop_decision(
        &mut self,
        now: Ms,
        t: &Trigger,
        id: TicketId,
        fx: &mut Vec<Effect>,
    ) -> Decision {
        if t.is_bot() {
            return Decision::Refused(Refusal::BotCannotStop);
        }
        if !t.is_person() {
            return Decision::Refused(Refusal::NotPerson);
        }
        if let Err(r) = gate(&self.policy, t, true) {
            return Decision::Refused(r);
        }
        let Some(tk) = self.tickets.get(&id) else {
            return Decision::Refused(Refusal::NoSuchTicket);
        };
        if tk.status == Status::Ended(End::Stopped) {
            return Decision::Refused(Refusal::TicketStopped);
        }
        self.end_ticket(now, id, End::Stopped, fx);
        Decision::Stop(id)
    }

    fn stop_owned(&mut self, now: Ms, trigger: &TriggerId, id: TicketId, fx: &mut Vec<Effect>) {
        let Some(t) = self.triggers.get(trigger).cloned() else {
            return reject(fx, "stop_owned", Refusal::StaleEval);
        };
        let Some(tk) = self.tickets.get_mut(&id) else {
            return reject(fx, "stop_owned", Refusal::NoSuchTicket);
        };
        let thread = t.thread.clone();
        let decision = if !tk.owned.contains(trigger) {
            Decision::Refused(Refusal::NotOwnedByTicket)
        } else {
            tk.owned.remove(trigger);
            let d = self.stop_decision(now, &t, id, fx);
            if matches!(d, Decision::Refused(_)) {
                if let Some(tk) = self.tickets.get_mut(&id) {
                    tk.owned.insert(trigger.clone());
                }
            }
            d
        };
        self.record(now, thread, Some(trigger.clone()), decision, fx);
    }

    fn report(
        &mut self,
        now: Ms,
        id: TicketId,
        report: Report,
        attention: Option<String>,
        fx: &mut Vec<Effect>,
    ) {
        let Some(t) = self.tickets.get_mut(&id) else {
            return reject(fx, "report", Refusal::NoSuchTicket);
        };
        if t.status != Status::Open {
            return reject(fx, "report", Refusal::TicketEnded);
        }
        t.changed_at = now;
        t.person_since_report = false;
        if report == Report::Dropped {
            return self.end_ticket(now, id, End::Done, fx);
        }
        let forced = matches!(report, Report::Blocked | Report::Failed);
        let handoff = forced || matches!(report, Report::Ready) || attention.is_some();
        if handoff && !forced && t.fresh_inputs > 0 {
            t.latest = Some(Report::Progress);
            t.attention = None;
            t.fresh_inputs = 0;
            return;
        }
        t.latest = Some(report);
        t.attention = attention;
        if handoff {
            t.fresh_inputs = 0;
            t.slot = None;
            t.phase = Phase::Parked;
            fx.push(Effect::HandOff { ticket: id });
        } else if report == Report::Waiting && !matches!(t.phase, Phase::Background { .. }) {
            t.slot = None;
            t.phase = Phase::Parked;
        }
    }

    fn prepare_failed(&mut self, now: Ms, id: TicketId, fx: &mut Vec<Effect>) {
        let Some(t) = self.tickets.get_mut(&id) else {
            return reject(fx, "prepare_failed", Refusal::NoSuchTicket);
        };
        if !matches!(t.phase, Phase::Preparing { .. }) {
            return reject(fx, "prepare_failed", Refusal::NoSlot);
        }
        t.latest = Some(Report::Failed);
        t.person_since_report = false;
        t.slot = None;
        t.phase = Phase::Parked;
        t.changed_at = now;
        fx.push(Effect::HandOff { ticket: id });
    }

    fn turn_ended(&mut self, now: Ms, id: TicketId, background: bool, fx: &mut Vec<Effect>) {
        let Some(t) = self.tickets.get_mut(&id) else {
            return reject(fx, "turn_ended", Refusal::NoSuchTicket);
        };
        if t.slot.is_none() || !matches!(t.phase, Phase::Turn | Phase::Background { .. }) {
            return reject(fx, "turn_ended", Refusal::NoSlot);
        }
        if background {
            t.phase = Phase::Background { since: now };
            return;
        }
        if t.fresh_inputs > 0 {
            t.fresh_inputs = 0;
            t.phase = Phase::Turn;
            fx.push(Effect::StartTurn { ticket: id });
            return;
        }
        if matches!(t.latest, None | Some(Report::Progress)) {
            t.latest = Some(Report::Waiting);
        }
        t.changed_at = now;
        t.slot = None;
        t.phase = Phase::Parked;
    }

    fn timers(&mut self, now: Ms, fx: &mut Vec<Effect>) {
        let expired: Vec<ThreadId> = self
            .threads
            .iter()
            .filter(|(_, th)| {
                th.run
                    .as_ref()
                    .and_then(|r| r.delivered_at)
                    .is_some_and(|d| d.saturating_add(self.cfg.eval_timeout_ms) <= now)
            })
            .map(|(k, _)| k.clone())
            .collect();
        for thread in expired {
            let Some(th) = self.threads.get_mut(&thread) else {
                continue;
            };
            th.expiries += 1;
            if th.expiries > self.cfg.eval_retries {
                self.fail_run(now, &thread, Why::Timeout, fx);
                continue;
            }
            if let Some(run) = th.run.take() {
                fx.push(Effect::EvalExpired {
                    eval: run.id,
                    retry: true,
                });
                let mut merged = run.batch;
                merged.extend(std::mem::take(&mut th.queued));
                th.queued = merged;
            }
            self.start_eval(now, &thread, fx);
        }
        let ids: Vec<TicketId> = self.tickets.keys().copied().collect();
        for id in ids {
            let Some(t) = self.tickets.get_mut(&id) else {
                continue;
            };
            match t.phase {
                Phase::Preparing { since }
                    if since.saturating_add(self.cfg.prepare_max_ms) <= now =>
                {
                    t.latest = Some(Report::Failed);
                    t.person_since_report = false;
                    t.slot = None;
                    t.phase = Phase::Parked;
                    t.changed_at = now;
                    fx.push(Effect::Abort { ticket: id });
                    fx.push(Effect::HandOff { ticket: id });
                }
                Phase::Background { since }
                    if since.saturating_add(self.cfg.background_max_ms) <= now =>
                {
                    t.latest = Some(Report::Waiting);
                    t.slot = None;
                    t.phase = Phase::Parked;
                    t.changed_at = now;
                    fx.push(Effect::KillJobs { ticket: id });
                }
                _ => {}
            }
            if let Some(f) = t.input_fail.filter(|f| f.next_at <= now) {
                t.input_fail = Some(InputFail {
                    attempts: f.attempts,
                    next_at: Ms::MAX,
                });
                fx.push(Effect::Redeliver { ticket: id });
            }
        }
        if let Some(c) = self.window {
            if now >= c && !self.window_announced {
                self.window_announced = true;
                for t in self.tickets.values().filter(|t| t.slot.is_some()) {
                    fx.push(Effect::WindDown { ticket: t.id });
                }
                fx.push(Effect::WindowClosed);
            }
        }
        let horizon = self.cfg.poll_horizon_ms;
        for (thread, th) in &mut self.threads {
            if !th.subs.is_empty()
                && !th.unpolled
                && th.last_activity.saturating_add(horizon) <= now
            {
                th.unpolled = true;
                fx.push(Effect::Unpoll {
                    thread: thread.clone(),
                });
            }
        }
    }

    fn settle(&mut self, now: Ms, fx: &mut Vec<Effect>) {
        loop {
            if !self.window_open() {
                break;
            }
            let cap = self.capacity();
            let (regular, reserve) = self.slots_held();
            let mut cands: Vec<(bool, u64, TicketId)> = self
                .tickets
                .values()
                .filter(|t| t.status == Status::Open)
                .filter_map(|t| match t.phase {
                    Phase::Queued { seq, person } => Some((!person, seq, t.id)),
                    _ => None,
                })
                .collect();
            cands.sort();
            let Some(&(not_person, _, id)) = cands.first() else {
                break;
            };
            let kind = if regular < cap {
                SlotKind::Regular
            } else if !not_person && reserve < 1 {
                SlotKind::Reserve
            } else {
                break;
            };
            let Some(t) = self.tickets.get_mut(&id) else {
                break;
            };
            t.slot = Some(kind);
            t.changed_at = now;
            if t.latest == Some(Report::Waiting) {
                t.latest = Some(Report::Progress);
            }
            if t.prepared {
                t.phase = Phase::Turn;
                if let Some(a) = t.assignments.last_mut() {
                    if *a == Assignment::Pending {
                        *a = Assignment::Running;
                    }
                }
                fx.push(Effect::StartTurn { ticket: id });
            } else {
                t.phase = Phase::Preparing { since: now };
                fx.push(Effect::Prepare { ticket: id });
            }
        }
    }
}

fn reject(fx: &mut Vec<Effect>, what: &'static str, why: Refusal) {
    fx.push(Effect::Rejected { what, why });
}
