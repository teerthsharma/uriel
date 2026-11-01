use std::collections::BTreeMap;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::config::RouterConfig;
use crate::input::{Effect, Input};
use crate::policy::{AcceptMode, Policy, ScopePolicy, SourcePolicy};
use crate::router::Router;
use crate::ticket::{Phase, Status};
use crate::types::*;

pub const SOURCE: &str = "afk";
pub const SCOPE: &str = "session";
const MIN_MS: Ms = 60_000;
const MAX_MS: Ms = 24 * 3_600_000;
const FORMAT: u32 = 1;

#[derive(Debug)]
pub enum AfkError {
    Io(String),
    Corrupt { line: usize, why: &'static str },
    BadDuration(String),
    NoSession,
    Active,
    Refused(Refusal),
    UnknownTicket(TicketId),
    WindowClosed,
    Empty,
}

impl fmt::Display for AfkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AfkError::Io(e) => write!(f, "io: {e}"),
            AfkError::Corrupt { line, why } => write!(f, "log corrupt at line {line}: {why}"),
            AfkError::BadDuration(s) => write!(
                f,
                "bad duration {s:?}: use 90m, 2h or 1h30m, between 1m and 24h"
            ),
            AfkError::NoSession => f.write_str("no afk session here; run `uriel afk start` first"),
            AfkError::Active => {
                f.write_str("an afk session is already open; run `uriel afk end` first")
            }
            AfkError::Refused(r) => write!(f, "the gate refused: {r:?}"),
            AfkError::UnknownTicket(t) => write!(f, "no ticket {}", t.0),
            AfkError::WindowClosed => f.write_str("the afk window has closed; nothing new starts"),
            AfkError::Empty => f.write_str("empty goal"),
        }
    }
}

impl std::error::Error for AfkError {}

impl From<std::io::Error> for AfkError {
    fn from(e: std::io::Error) -> AfkError {
        AfkError::Io(e.to_string())
    }
}

pub fn parse_duration(s: &str) -> Result<Ms, AfkError> {
    let bad = || AfkError::BadDuration(s.to_string());
    let s = s.trim().to_ascii_lowercase();
    if s.is_empty() {
        return Err(bad());
    }
    let mut total: Ms = 0;
    let mut num = String::new();
    let mut seen_unit = false;
    for c in s.chars() {
        if c.is_ascii_digit() {
            num.push(c);
            continue;
        }
        let n: Ms = num.parse().map_err(|_| bad())?;
        num.clear();
        let unit = match c {
            'h' => 3_600_000,
            'm' => 60_000,
            's' => 1_000,
            _ => return Err(bad()),
        };
        total = total
            .checked_add(n.checked_mul(unit).ok_or_else(bad)?)
            .ok_or_else(bad)?;
        seen_unit = true;
    }
    if !num.is_empty() {
        if seen_unit {
            return Err(bad());
        }
        let n: Ms = num.parse().map_err(|_| bad())?;
        total = n.checked_mul(60_000).ok_or_else(bad)?;
    }
    if !(MIN_MS..=MAX_MS).contains(&total) {
        return Err(bad());
    }
    Ok(total)
}

pub fn afk_policy(owner: &str) -> Policy {
    let source = SourcePolicy {
        listening: true,
        allowed: [owner.to_string()].into(),
        bots: Default::default(),
        scopes: [(SCOPE.to_string(), ScopePolicy::default())].into(),
    };
    Policy {
        mode: AcceptMode::Off,
        rules: BTreeMap::new(),
        sources: [(SOURCE.to_string(), source)].into(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Header {
    v: u32,
    owner: String,
    workers: u32,
    started: Ms,
    closes: Ms,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "k")]
enum Line {
    Header(Header),
    Step {
        at: Ms,
        input: Input,
        digest: String,
    },
    Note {
        ticket: TicketId,
        goal: Option<String>,
        text: String,
    },
    Result {
        ticket: TicketId,
        text: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Ready,
    Blocked,
    Failed,
    Done,
    Waiting,
    Ask,
}

impl Kind {
    pub fn parse(s: &str) -> Option<Kind> {
        Some(match s {
            "ready" => Kind::Ready,
            "blocked" => Kind::Blocked,
            "failed" => Kind::Failed,
            "done" => Kind::Done,
            "waiting" => Kind::Waiting,
            "ask" => Kind::Ask,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "do", rename_all = "snake_case")]
pub enum Action {
    Start {
        ticket: u64,
        goal: Option<String>,
        scope: String,
    },
    WindDown {
        ticket: u64,
    },
    WindowClosed,
}

pub struct Afk {
    path: PathBuf,
    file: File,
    header: Header,
    router: Router,
    notes: BTreeMap<TicketId, (Option<String>, String)>,
    results: BTreeMap<TicketId, String>,
    ingested: u64,
}

fn hex(d: [u8; 32]) -> String {
    d.iter().map(|b| format!("{b:02x}")).collect()
}

fn router_for(h: &Header) -> Router {
    let cfg = RouterConfig {
        max_workers: h.workers,
        ..RouterConfig::default()
    };
    Router::new(cfg, afk_policy(&h.owner))
}

impl Afk {
    pub fn start(
        path: &Path,
        owner: &str,
        window_ms: Ms,
        workers: u32,
        now: Ms,
    ) -> Result<Afk, AfkError> {
        if !(MIN_MS..=MAX_MS).contains(&window_ms) {
            return Err(AfkError::BadDuration(format!("{window_ms}ms")));
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let file = OpenOptions::new()
            .read(true)
            .append(true)
            .create_new(true)
            .open(path)
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::AlreadyExists => AfkError::Active,
                _ => AfkError::from(e),
            })?;
        file.lock()?;
        let header = Header {
            v: FORMAT,
            owner: owner.to_string(),
            workers: workers.clamp(1, 16),
            started: now,
            closes: now.saturating_add(window_ms),
        };
        let mut afk = Afk {
            path: path.to_path_buf(),
            file,
            router: router_for(&header),
            header: header.clone(),
            notes: BTreeMap::new(),
            results: BTreeMap::new(),
            ingested: 0,
        };
        afk.write(&Line::Header(header))?;
        afk.step(
            now,
            Input::Window {
                closes: Some(afk.header.closes),
            },
        )?;
        Ok(afk)
    }

    pub fn open(path: &Path) -> Result<Afk, AfkError> {
        let mut file = OpenOptions::new()
            .read(true)
            .append(true)
            .open(path)
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => AfkError::NoSession,
                _ => AfkError::from(e),
            })?;
        file.lock()?;
        let mut text = String::new();
        file.read_to_string(&mut text)?;
        if !text.is_empty() && !text.ends_with('\n') {
            let keep = text.rfind('\n').map_or(0, |i| i + 1);
            file.set_len(keep as u64)?;
            text.truncate(keep);
        }
        let mut lines = text.lines().enumerate();
        let header = match lines.next() {
            Some((_, l)) => match serde_json::from_str::<Line>(l) {
                Ok(Line::Header(h)) if h.v == FORMAT => h,
                _ => {
                    return Err(AfkError::Corrupt {
                        line: 1,
                        why: "bad header",
                    })
                }
            },
            None => {
                return Err(AfkError::Corrupt {
                    line: 1,
                    why: "empty log",
                })
            }
        };
        let mut afk = Afk {
            path: path.to_path_buf(),
            file,
            router: router_for(&header),
            header,
            notes: BTreeMap::new(),
            results: BTreeMap::new(),
            ingested: 0,
        };
        for (i, l) in lines {
            let n = i + 1;
            match serde_json::from_str::<Line>(l) {
                Ok(Line::Step { at, input, digest }) => {
                    if matches!(input, Input::Ingest(_)) {
                        afk.ingested += 1;
                    }
                    afk.router.apply(at, input);
                    if hex(afk.router.digest()) != digest {
                        return Err(AfkError::Corrupt {
                            line: n,
                            why: "digest mismatch",
                        });
                    }
                }
                Ok(Line::Note { ticket, goal, text }) => {
                    afk.notes.insert(ticket, (goal, text));
                }
                Ok(Line::Result { ticket, text }) => {
                    afk.results.insert(ticket, text);
                }
                _ => {
                    return Err(AfkError::Corrupt {
                        line: n,
                        why: "unreadable line",
                    })
                }
            }
        }
        Ok(afk)
    }

    pub fn router(&self) -> &Router {
        &self.router
    }

    pub fn closes(&self) -> Ms {
        self.header.closes
    }

    fn write(&mut self, line: &Line) -> Result<(), AfkError> {
        let mut s = serde_json::to_string(line).map_err(|e| AfkError::Io(e.to_string()))?;
        s.push('\n');
        self.file.write_all(s.as_bytes())?;
        self.file.flush()?;
        Ok(())
    }

    fn step(&mut self, now: Ms, input: Input) -> Result<Vec<Effect>, AfkError> {
        let fx = self.router.apply(now, input.clone());
        let at = self.router.now();
        let digest = hex(self.router.digest());
        self.write(&Line::Step { at, input, digest })?;
        Ok(fx)
    }

    fn drive(&mut self, now: Ms, mut fx: Vec<Effect>) -> Result<Vec<Action>, AfkError> {
        let mut all = Vec::new();
        while !fx.is_empty() {
            let mut next = Vec::new();
            for e in &fx {
                if let Effect::Prepare { ticket } = e {
                    next.extend(self.step(now, Input::PrepareDone { ticket: *ticket })?);
                }
            }
            all.append(&mut fx);
            fx = next;
        }
        Ok(self.actions(&all))
    }

    fn actions(&self, fx: &[Effect]) -> Vec<Action> {
        let mut out = Vec::new();
        for e in fx {
            match e {
                Effect::StartTurn { ticket } => {
                    let (goal, scope) = self.notes.get(ticket).cloned().unwrap_or_default();
                    out.push(Action::Start {
                        ticket: ticket.0,
                        goal,
                        scope,
                    });
                }
                Effect::WindDown { ticket } => out.push(Action::WindDown { ticket: ticket.0 }),
                Effect::WindowClosed => out.push(Action::WindowClosed),
                _ => {}
            }
        }
        out
    }

    pub fn pressure(
        &mut self,
        now: Ms,
        free_mb: u64,
        total_mb: u64,
    ) -> Result<Vec<Action>, AfkError> {
        let ram = self.router.config().ram_slots(free_mb, total_mb);
        let cap = self.router.config().max_workers.min(ram);
        if cap == self.router.capacity() {
            return Ok(Vec::new());
        }
        let fx = self.step(now, Input::Pressure { free_mb, total_mb })?;
        self.drive(now, fx)
    }

    pub fn tick(&mut self, now: Ms) -> Result<Vec<Action>, AfkError> {
        let fx = self.step(now, Input::Tick)?;
        self.drive(now, fx)
    }

    pub fn add(
        &mut self,
        now: Ms,
        goal: Option<&str>,
        text: &str,
    ) -> Result<(TicketId, Vec<Action>), AfkError> {
        let text = text.trim();
        if text.is_empty() {
            return Err(AfkError::Empty);
        }
        let fx = self.step(now, Input::Tick)?;
        let mut actions = self.drive(now, fx)?;
        if !self.router.window_open() {
            return Err(AfkError::WindowClosed);
        }
        let n = self.ingested + 1;
        let trigger = Trigger {
            id: TriggerId::new(format!("afk-{n}")),
            source: SOURCE.into(),
            scope: SCOPE.into(),
            thread: ThreadId::new(format!(
                "afk/{}",
                goal.map_or(n.to_string(), str::to_string)
            )),
            actor: Some(Actor {
                id: self.header.owner.clone(),
                bot: false,
            }),
            addressed: true,
        };
        let tid = trigger.id.clone();
        self.ingested = n;
        let mut fx = self.step(now, Input::Ingest(trigger))?;
        let Some(eval) = fx.iter().find_map(|e| match e {
            Effect::StartEval { eval, .. } => Some(*eval),
            _ => None,
        }) else {
            let why = match self.router.dispositions(&tid).last() {
                Some(Disposition::Filtered(r)) => *r,
                _ => Refusal::Ignored,
            };
            return Err(AfkError::Refused(why));
        };
        fx.extend(self.step(now, Input::EvalDelivered { eval })?);
        fx.extend(self.step(
            now,
            Input::EvalResult {
                eval,
                verdicts: vec![(
                    tid,
                    Verdict::Accept {
                        scope: text.to_string(),
                        rules: Vec::new(),
                        assessed: true,
                    },
                )],
            },
        )?);
        let ticket = fx
            .iter()
            .find_map(|e| match e {
                Effect::TicketOpened { ticket, .. } => Some(*ticket),
                _ => None,
            })
            .ok_or(AfkError::Refused(Refusal::Ignored))?;
        self.notes
            .insert(ticket, (goal.map(str::to_string), text.to_string()));
        self.write(&Line::Note {
            ticket,
            goal: goal.map(str::to_string),
            text: text.to_string(),
        })?;
        actions.extend(self.drive(now, fx)?);
        Ok((ticket, actions))
    }

    pub fn report(
        &mut self,
        now: Ms,
        ticket: TicketId,
        kind: Kind,
        note: Option<String>,
    ) -> Result<Vec<Action>, AfkError> {
        if self.router.ticket(ticket).is_none() {
            return Err(AfkError::UnknownTicket(ticket));
        }
        let (report, attention) = match kind {
            Kind::Ready => (Report::Ready, None),
            Kind::Blocked => (Report::Blocked, None),
            Kind::Failed => (Report::Failed, None),
            Kind::Done => (Report::Dropped, None),
            Kind::Waiting => (Report::Waiting, None),
            Kind::Ask => (
                Report::Progress,
                Some(note.clone().unwrap_or_else(|| "needs a decision".into())),
            ),
        };
        if let Some(n) = note
            .as_ref()
            .filter(|n| kind != Kind::Ask && !n.trim().is_empty())
        {
            self.results.insert(ticket, n.clone());
            self.write(&Line::Result {
                ticket,
                text: n.clone(),
            })?;
        }
        let mut fx = self.step(
            now,
            Input::Report {
                ticket,
                report,
                attention,
            },
        )?;
        if let Some(t) = self.router.ticket(ticket) {
            if t.status == Status::Open
                && t.slot.is_some()
                && matches!(t.phase, Phase::Turn)
                && kind == Kind::Waiting
            {
                fx.extend(self.step(
                    now,
                    Input::TurnEnded {
                        ticket,
                        background: false,
                    },
                )?);
            }
        }
        self.drive(now, fx)
    }

    pub fn status(&mut self, now: Ms) -> Result<Value, AfkError> {
        let fx = self.step(now, Input::Tick)?;
        let actions = self.drive(now, fx)?;
        let r = &self.router;
        let tickets: Vec<Value> = r
            .tickets()
            .map(|t| {
                let (goal, text) = self.notes.get(&t.id).cloned().unwrap_or_default();
                json!({
                    "id": t.id.0,
                    "goal": goal,
                    "scope": text,
                    "stage": t.stage().map(|s| format!("{s:?}")),
                    "status": match t.status { Status::Open => "open".to_string(), Status::Ended(e) => format!("{e:?}") },
                    "slot": t.slot.map(|s| format!("{s:?}")),
                    "ask": t.attention,
                    "result": self.results.get(&t.id),
                })
            })
            .collect();
        let (regular, reserve) = r.slots_held();
        Ok(json!({
            "window": {
                "open": r.window_open(),
                "remaining_s": self.header.closes.saturating_sub(r.now()) / 1000,
            },
            "capacity": r.capacity(),
            "slots": { "regular": regular, "reserve": reserve },
            "tickets": tickets,
            "actions": actions,
        }))
    }

    pub fn summary(&self) -> Value {
        let r = &self.router;
        let mut ready = Vec::new();
        let mut needs_you = Vec::new();
        let mut unfinished = Vec::new();
        let mut never_started = Vec::new();
        let mut done = Vec::new();
        let mut idle = Vec::new();
        for t in r.tickets() {
            let (goal, text) = self.notes.get(&t.id).cloned().unwrap_or_default();
            let item = json!({ "id": t.id.0, "goal": goal, "scope": text, "ask": t.attention, "result": self.results.get(&t.id) });
            match (t.status, t.stage()) {
                (Status::Ended(End::Done), _) => done.push(item),
                (Status::Ended(_), _) => needs_you.push(item),
                (_, Some(Stage::PersonReview)) => ready.push(item),
                (_, Some(s)) if s.needs_person() => needs_you.push(item),
                (_, Some(Stage::Queued)) if !t.prepared => never_started.push(item),
                (_, Some(Stage::Idle)) => idle.push(item),
                _ => unfinished.push(item),
            }
        }
        let headline = format!(
            "{} ready, {} need you, {} unfinished, {} never started, {} done",
            ready.len(),
            needs_you.len(),
            unfinished.len(),
            never_started.len(),
            done.len() + idle.len()
        );
        json!({
            "headline": headline,
            "ready": ready,
            "needs_you": needs_you,
            "unfinished": unfinished,
            "never_started": never_started,
            "done": done,
            "idle": idle,
            "digest": hex(r.digest()),
        })
    }

    pub fn end(mut self, now: Ms) -> Result<(Value, PathBuf), AfkError> {
        self.step(now, Input::Window { closes: Some(now) })?;
        let summary = self.summary();
        let started = self.header.started;
        let path = self.path.clone();
        drop(self.file);
        let archive = path.with_file_name(format!(
            "{}.{started}.done",
            path.file_name()
                .map_or_else(|| "afk".into(), |n| n.to_string_lossy().to_string())
        ));
        std::fs::rename(&path, &archive)?;
        Ok((summary, archive))
    }
}
