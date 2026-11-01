use std::collections::{BTreeMap, VecDeque};

use serde_json::{json, Value as Json};

use crate::error::Error;
use crate::sanitize::{sanitize, Policy, Report};
use crate::trace::{Event, Kind, Trace};

const ATTR_CAP: usize = 8;
const ATTR_BYTES: usize = 120;
const LABEL_BYTES: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Limits {
    pub max_concurrent: usize,
    pub max_depth: usize,
    pub max_total: usize,
    pub max_tokens: u64,
    pub max_events: usize,
    pub max_runs: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_concurrent: 8,
            max_depth: 2,
            max_total: 64,
            max_tokens: 2_000_000,
            max_events: 4096,
            max_runs: 16,
        }
    }
}

impl Limits {
    pub fn clamped(self) -> Limits {
        Limits {
            max_concurrent: self.max_concurrent.max(1),
            max_depth: self.max_depth.max(1),
            max_total: self.max_total.max(1),
            max_tokens: self.max_tokens.max(1),
            max_events: self.max_events.max(16),
            max_runs: self.max_runs.max(1),
        }
    }

    pub fn to_json(&self) -> Json {
        json!({
            "max_concurrent": self.max_concurrent,
            "max_depth": self.max_depth,
            "max_total": self.max_total,
            "max_tokens": self.max_tokens,
            "max_events": self.max_events,
            "max_runs": self.max_runs
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Want {
    Count(usize),
    Max,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Deny {
    Concurrency,
    Depth,
    Total,
    TokenBudget,
}

impl Deny {
    pub fn name(self) -> &'static str {
        match self {
            Deny::Concurrency => "concurrency",
            Deny::Depth => "depth",
            Deny::Total => "total",
            Deny::TokenBudget => "token_budget",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Admission {
    pub run: String,
    pub granted: Vec<String>,
    pub requested: Option<usize>,
    pub deferred: usize,
    pub deny: Option<Deny>,
    pub free_after: usize,
}

impl Admission {
    pub fn to_json(&self) -> Json {
        json!({
            "run": self.run,
            "granted": self.granted,
            "requested": self.requested,
            "deferred": self.deferred,
            "denied_by": self.deny.map(Deny::name),
            "free_after": self.free_after
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Running,
    Done,
    Failed,
    Cancelled,
}

impl Status {
    pub fn name(self) -> &'static str {
        match self {
            Status::Running => "running",
            Status::Done => "done",
            Status::Failed => "failed",
            Status::Cancelled => "cancelled",
        }
    }

    pub fn parse_final(name: &str) -> Option<Status> {
        match name {
            "done" => Some(Status::Done),
            "failed" => Some(Status::Failed),
            "cancelled" => Some(Status::Cancelled),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
struct Agent {
    depth: usize,
    status: Status,
    tokens: u64,
}

#[derive(Debug)]
struct Run {
    agents: BTreeMap<String, Agent>,
    spawned: usize,
    tokens: u64,
    trace: Trace,
}

impl Run {
    fn running(&self) -> usize {
        self.agents
            .values()
            .filter(|a| a.status == Status::Running)
            .count()
    }
}

#[derive(Debug, Clone, Default)]
pub struct Obs {
    pub kind: Option<Kind>,
    pub name: String,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub cached_in: u64,
    pub t_ms: Option<u64>,
    pub attrs: BTreeMap<String, String>,
}

#[derive(Debug)]
pub struct Fleet {
    limits: Limits,
    runs: BTreeMap<String, Run>,
    order: VecDeque<String>,
}

pub fn valid_id(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b':' | b'-'))
}

fn label(text: &str) -> String {
    sanitize(text, &Policy::new(LABEL_BYTES)).text
}

fn attrs_of(raw: &BTreeMap<String, String>, report: &mut Report) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for (k, v) in raw.iter().take(ATTR_CAP) {
        let key = sanitize(k, &Policy::new(LABEL_BYTES));
        let val = sanitize(v, &Policy::new(ATTR_BYTES));
        for (class, n) in val.report.secrets.iter().chain(key.report.secrets.iter()) {
            *report.secrets.entry(class).or_insert(0) += n;
        }
        report.injection += val.report.injection;
        report.hidden += val.report.hidden + key.report.hidden;
        report.tags += val.report.tags + key.report.tags;
        report.controls += val.report.controls + key.report.controls;
        report.ansi += val.report.ansi + key.report.ansi;
        report.truncated |= val.report.truncated;
        out.insert(key.text, val.text);
    }
    report.truncated |= raw.len() > ATTR_CAP;
    out
}

impl Fleet {
    pub fn new(limits: Limits) -> Fleet {
        Fleet {
            limits: limits.clamped(),
            runs: BTreeMap::new(),
            order: VecDeque::new(),
        }
    }

    pub fn limits(&self) -> &Limits {
        &self.limits
    }

    fn open(&mut self, run: &str) -> Result<(), Error> {
        if !valid_id(run) {
            return Err(Error::BadId(run.to_string()));
        }
        if self.runs.contains_key(run) {
            return Ok(());
        }
        if self.runs.len() >= self.limits.max_runs {
            let victim = self
                .order
                .iter()
                .find(|id| self.runs.get(*id).is_some_and(|r| r.running() == 0))
                .cloned();
            let Some(victim) = victim else {
                return Err(Error::TooManyRuns(self.runs.len()));
            };
            self.runs.remove(&victim);
            self.order.retain(|id| id != &victim);
        }
        self.runs.insert(
            run.to_string(),
            Run {
                agents: BTreeMap::new(),
                spawned: 0,
                tokens: 0,
                trace: Trace::new(self.limits.max_events),
            },
        );
        self.order.push_back(run.to_string());
        Ok(())
    }

    fn run_mut(&mut self, run: &str) -> Result<&mut Run, Error> {
        self.runs
            .get_mut(run)
            .ok_or_else(|| Error::UnknownRun(run.to_string()))
    }

    pub fn admit(
        &mut self,
        run: &str,
        want: Want,
        parent: Option<&str>,
        role: &str,
        tier: &str,
    ) -> Result<Admission, Error> {
        self.open(run)?;
        let limits = self.limits.clone();
        let role = label(role);
        let tier = label(tier);
        let r = self.run_mut(run)?;
        let depth = match parent {
            None => 1,
            Some(p) => match r.agents.get(p) {
                Some(a) if a.status == Status::Running => a.depth + 1,
                Some(_) => return Err(Error::NotRunning(p.to_string())),
                None => return Err(Error::UnknownAgent(p.to_string())),
            },
        };
        let requested = match want {
            Want::Count(n) => Some(n),
            Want::Max => None,
        };
        let free = limits.max_concurrent.saturating_sub(r.running());
        let left = limits.max_total.saturating_sub(r.spawned);
        let deny_all = if depth > limits.max_depth {
            Some(Deny::Depth)
        } else if r.tokens >= limits.max_tokens {
            Some(Deny::TokenBudget)
        } else {
            None
        };
        let room = if deny_all.is_some() { 0 } else { free.min(left) };
        let granted_n = requested.map_or(room, |n| n.min(room));
        let binding = if granted_n < requested.unwrap_or(0) || (requested.is_none() && room == 0) {
            deny_all.or(if free <= left {
                Some(Deny::Concurrency)
            } else {
                Some(Deny::Total)
            })
        } else {
            None
        };
        let mut granted = Vec::with_capacity(granted_n);
        for _ in 0..granted_n {
            r.spawned += 1;
            let id = format!("a{}", r.spawned);
            r.agents.insert(
                id.clone(),
                Agent {
                    depth,
                    status: Status::Running,
                    tokens: 0,
                },
            );
            let mut ev = Event::new(run, &id, Kind::Start, "agent");
            ev.parent = parent.map(str::to_string);
            ev.role = Some(role.clone());
            ev.tier = Some(tier.clone());
            ev.status = Some(Status::Running.name().to_string());
            r.trace.push(ev);
            granted.push(id);
        }
        Ok(Admission {
            run: run.to_string(),
            requested,
            deferred: requested.map_or(0, |n| n - granted_n),
            deny: binding,
            free_after: limits.max_concurrent.saturating_sub(r.running()),
            granted,
        })
    }

    pub fn observe(&mut self, run: &str, agent: &str, obs: Obs) -> Result<(Event, Report), Error> {
        let max_tokens = self.limits.max_tokens;
        let r = self.run_mut(run)?;
        let status = r
            .agents
            .get(agent)
            .ok_or_else(|| Error::UnknownAgent(agent.to_string()))?
            .status;
        if status != Status::Running {
            return Err(Error::NotRunning(agent.to_string()));
        }
        let mut report = Report::default();
        let kind = obs.kind.unwrap_or(Kind::Mark);
        let kind = if matches!(kind, Kind::Start | Kind::End | Kind::Reap) {
            Kind::Mark
        } else {
            kind
        };
        let mut ev = Event::new(run, agent, kind, &label(&obs.name));
        ev.tokens_in = obs.tokens_in;
        ev.tokens_out = obs.tokens_out;
        ev.cached_in = obs.cached_in.min(obs.tokens_in);
        ev.t_ms = obs.t_ms;
        ev.attrs = attrs_of(&obs.attrs, &mut report);
        let spent = obs.tokens_in.saturating_add(obs.tokens_out);
        if let Some(a) = r.agents.get_mut(agent) {
            a.tokens = a.tokens.saturating_add(spent);
        }
        r.tokens = r.tokens.saturating_add(spent);
        let over = r.tokens > max_tokens;
        let out = r.trace.push(ev).clone();
        if over {
            let mut warn = Event::new(run, agent, Kind::Mark, "token_budget_exceeded");
            warn.status = Some("over_budget".to_string());
            r.trace.push(warn);
        }
        Ok((out, report))
    }

    pub fn end(&mut self, run: &str, agent: &str, status: Status, note: &str) -> Result<Event, Error> {
        let r = self.run_mut(run)?;
        let a = r
            .agents
            .get_mut(agent)
            .ok_or_else(|| Error::UnknownAgent(agent.to_string()))?;
        if a.status != Status::Running {
            return Err(Error::NotRunning(agent.to_string()));
        }
        a.status = if status == Status::Running { Status::Done } else { status };
        let final_status = a.status;
        let mut ev = Event::new(run, agent, Kind::End, "agent");
        ev.status = Some(final_status.name().to_string());
        if !note.is_empty() {
            ev.attrs.insert("note".to_string(), sanitize(note, &Policy::new(ATTR_BYTES)).text);
        }
        Ok(r.trace.push(ev).clone())
    }

    pub fn reap(&mut self, run: &str) -> Result<usize, Error> {
        let r = self.run_mut(run)?;
        let stuck: Vec<String> = r
            .agents
            .iter()
            .filter(|(_, a)| a.status == Status::Running)
            .map(|(id, _)| id.clone())
            .collect();
        for id in &stuck {
            if let Some(a) = r.agents.get_mut(id) {
                a.status = Status::Cancelled;
            }
            let mut ev = Event::new(run, id, Kind::Reap, "agent");
            ev.status = Some(Status::Cancelled.name().to_string());
            r.trace.push(ev);
        }
        Ok(stuck.len())
    }

    pub fn trace_jsonl(&self, run: &str) -> Result<String, Error> {
        self.runs
            .get(run)
            .map(|r| r.trace.jsonl())
            .ok_or_else(|| Error::UnknownRun(run.to_string()))
    }

    pub fn trace(&self, run: &str) -> Option<&Trace> {
        self.runs.get(run).map(|r| &r.trace)
    }

    pub fn summary(&self, run: &str) -> Result<Json, Error> {
        let r = self
            .runs
            .get(run)
            .ok_or_else(|| Error::UnknownRun(run.to_string()))?;
        let count = |s: Status| r.agents.values().filter(|a| a.status == s).count();
        Ok(json!({
            "run": run,
            "spawned": r.spawned,
            "running": count(Status::Running),
            "done": count(Status::Done),
            "failed": count(Status::Failed),
            "cancelled": count(Status::Cancelled),
            "tokens": r.tokens,
            "events": r.trace.len(),
            "dropped": r.trace.dropped(),
            "chain_ok": r.trace.verify().is_ok(),
            "head": r.trace.head(),
            "limits": self.limits.to_json()
        }))
    }
}
