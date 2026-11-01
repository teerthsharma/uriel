use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value as Json};
use uriel_fleet::{sanitize, Policy, Report};

use crate::Error;

pub const SCHEMA: &str = "uriel.ledger.v1";
pub const MAX_GOALS: usize = 256;
pub const MAX_BATCH: usize = 64;
pub const MAX_NEEDS: usize = 16;
pub const MAX_TITLE_BYTES: usize = 200;
pub const MAX_DETAIL_BYTES: usize = 600;
pub const MAX_RESULT_BYTES: usize = 400;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Pending,
    Issued,
    Done,
    Failed,
}

impl State {
    pub fn name(self) -> &'static str {
        match self {
            State::Pending => "pending",
            State::Issued => "issued",
            State::Done => "done",
            State::Failed => "failed",
        }
    }

    fn parse(s: &str) -> Option<State> {
        match s {
            "pending" => Some(State::Pending),
            "issued" => Some(State::Issued),
            "done" => Some(State::Done),
            "failed" => Some(State::Failed),
            _ => None,
        }
    }
}

/// A goal as a caller proposes it. Text is sanitised by [`Ledger::add`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewGoal {
    pub id: String,
    pub title: String,
    pub detail: String,
    pub needs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Goal {
    title: String,
    detail: String,
    result: String,
    needs: Vec<String>,
    state: State,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddReport {
    pub added: usize,
    pub unchanged: usize,
    pub sanitised: Report,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ledger {
    name: String,
    goals: BTreeMap<String, Goal>,
}

pub fn valid_id(s: &str) -> bool {
    (1..=64).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b':' | b'-'))
}

fn absorb(into: &mut Report, from: Report) {
    into.truncated |= from.truncated;
    into.ansi += from.ansi;
    into.controls += from.controls;
    into.hidden += from.hidden;
    into.tags += from.tags;
    into.fences += from.fences;
    into.injection += from.injection;
    for (family, n) in from.secrets {
        *into.secrets.entry(family).or_insert(0) += n;
    }
}

fn clean(text: &str, cap: usize, report: &mut Report) -> String {
    let out = sanitize(text, &Policy::new(cap));
    absorb(report, out.report);
    out.text
}

/// The characters `scripts/ledger_next.py` refuses in stored text.
fn text_is_clean(s: &str) -> bool {
    s.chars().all(|c| {
        let n = c as u32;
        !((n < 32 && c != '\n' && c != '\t')
            || (0x7F..0xA0).contains(&n)
            || matches!(n, 0x202A..=0x202E | 0x200B | 0xFEFF))
    })
}

fn sorted_set(needs: &[String]) -> Vec<String> {
    needs
        .iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .cloned()
        .collect()
}

/// Kahn layers; ids sort lexicographically inside a layer.
fn layers_of(goals: &BTreeMap<String, Goal>) -> Result<Vec<Vec<String>>, Error> {
    let mut indegree: BTreeMap<&str, usize> = goals
        .iter()
        .map(|(id, g)| (id.as_str(), g.needs.len()))
        .collect();
    let mut dependents: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (id, g) in goals {
        for n in &g.needs {
            dependents.entry(n.as_str()).or_default().push(id.as_str());
        }
    }
    let mut layer: BTreeSet<&str> = indegree
        .iter()
        .filter(|(_, d)| **d == 0)
        .map(|(id, _)| *id)
        .collect();
    let mut out = Vec::new();
    let mut placed = 0;
    while !layer.is_empty() {
        let mut next = BTreeSet::new();
        for id in &layer {
            for dep in dependents.get(id).into_iter().flatten() {
                if let Some(d) = indegree.get_mut(dep) {
                    *d = d.saturating_sub(1);
                    if *d == 0 {
                        next.insert(*dep);
                    }
                }
            }
        }
        placed += layer.len();
        out.push(layer.iter().map(|s| (*s).to_string()).collect());
        layer = next;
    }
    if placed < goals.len() {
        return Err(Error::Cycle);
    }
    Ok(out)
}

/// Every need exists and is not the goal itself; then no cycle.
fn check_graph(goals: &BTreeMap<String, Goal>) -> Result<Vec<Vec<String>>, Error> {
    for (id, g) in goals {
        for n in &g.needs {
            if n == id {
                return Err(Error::SelfNeed(id.clone()));
            }
            if !goals.contains_key(n) {
                return Err(Error::UnknownNeed {
                    id: id.clone(),
                    need: n.clone(),
                });
            }
        }
    }
    layers_of(goals)
}

impl Ledger {
    pub fn new(name: &str) -> Result<Ledger, Error> {
        if !valid_id(name) {
            return Err(Error::BadId(name.to_string()));
        }
        Ok(Ledger {
            name: name.to_string(),
            goals: BTreeMap::new(),
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn len(&self) -> usize {
        self.goals.len()
    }

    pub fn is_empty(&self) -> bool {
        self.goals.is_empty()
    }

    pub fn state_of(&self, id: &str) -> Option<State> {
        self.goals.get(id).map(|g| g.state)
    }

    pub fn needs_of(&self, id: &str) -> Option<&[String]> {
        self.goals.get(id).map(|g| g.needs.as_slice())
    }

    /// Topological layers, ids sorted inside each layer. A stored ledger is always acyclic.
    pub fn layers(&self) -> Vec<Vec<String>> {
        layers_of(&self.goals).unwrap_or_default()
    }

    /// Add a batch. The whole batch is checked on a copy and committed only if every rule holds:
    /// ids, at most 64 goals per batch and 256 in total, at most 16 needs, no self-dependency, no
    /// unknown dependency, no cycle, no changed redefinition. Adding an identical goal is a no-op.
    pub fn add(&mut self, batch: Vec<NewGoal>) -> Result<AddReport, Error> {
        if batch.len() > MAX_BATCH {
            return Err(Error::BatchTooLarge {
                max: MAX_BATCH,
                got: batch.len(),
            });
        }
        let mut report = Report::default();
        let mut next = self.goals.clone();
        let (mut added, mut unchanged) = (0, 0);
        for g in batch {
            if !valid_id(&g.id) {
                return Err(Error::BadId(g.id));
            }
            if g.needs.iter().any(|n| !valid_id(n)) {
                return Err(Error::BadId(
                    g.needs
                        .iter()
                        .find(|n| !valid_id(n))
                        .cloned()
                        .unwrap_or_default(),
                ));
            }
            let needs = sorted_set(&g.needs);
            if needs.len() > MAX_NEEDS {
                return Err(Error::TooManyNeeds {
                    id: g.id,
                    max: MAX_NEEDS,
                    got: needs.len(),
                });
            }
            let title = clean(&g.title, MAX_TITLE_BYTES, &mut report);
            if title.trim().is_empty() {
                return Err(Error::EmptyTitle(g.id));
            }
            let detail = clean(&g.detail, MAX_DETAIL_BYTES, &mut report);
            match next.get(&g.id) {
                Some(old) if old.title == title && old.detail == detail && old.needs == needs => {
                    unchanged += 1;
                }
                Some(_) => return Err(Error::Changed(g.id)),
                None => {
                    next.insert(
                        g.id,
                        Goal {
                            title,
                            detail,
                            result: String::new(),
                            needs,
                            state: State::Pending,
                        },
                    );
                    added += 1;
                }
            }
        }
        if next.len() > MAX_GOALS {
            return Err(Error::TooManyGoals {
                max: MAX_GOALS,
                got: next.len(),
            });
        }
        check_graph(&next)?;
        self.goals = next;
        Ok(AddReport {
            added,
            unchanged,
            sanitised: report,
        })
    }

    /// Pending goals whose dependencies are all done, earliest layer first, ids sorted inside a
    /// layer, at most `limit`. Does not change anything.
    pub fn ready(&self, limit: usize) -> Vec<String> {
        let mut out = Vec::new();
        for layer in self.layers() {
            for id in layer {
                if out.len() >= limit {
                    return out;
                }
                if self.is_ready(&id) {
                    out.push(id);
                }
            }
        }
        out
    }

    fn is_ready(&self, id: &str) -> bool {
        self.goals.get(id).is_some_and(|g| {
            g.state == State::Pending
                && g.needs
                    .iter()
                    .all(|n| self.goals.get(n).is_some_and(|d| d.state == State::Done))
        })
    }

    /// Issue the ready frontier: mark up to `limit` ready goals `issued` and return their ids.
    pub fn issue(&mut self, limit: usize) -> Vec<String> {
        let ids = self.ready(limit);
        for id in &ids {
            if let Some(g) = self.goals.get_mut(id) {
                g.state = State::Issued;
            }
        }
        ids
    }

    /// Record the outcome of an issued goal. The result text is sanitised and cut to 400 bytes.
    pub fn finish(&mut self, id: &str, ok: bool, result: &str) -> Result<Report, Error> {
        let mut report = Report::default();
        let text = clean(result, MAX_RESULT_BYTES, &mut report);
        let g = self
            .goals
            .get_mut(id)
            .ok_or_else(|| Error::UnknownGoal(id.to_string()))?;
        if g.state != State::Issued {
            return Err(Error::NotIssued(id.to_string()));
        }
        g.result = text;
        g.state = if ok { State::Done } else { State::Failed };
        Ok(report)
    }

    /// Pending goals with a failed goal among their transitive dependencies.
    pub fn blocked(&self) -> BTreeSet<String> {
        let mut tainted: BTreeSet<&str> = BTreeSet::new();
        for layer in self.layers() {
            for id in layer {
                let Some((key, g)) = self.goals.get_key_value(id.as_str()) else {
                    continue;
                };
                let bad = g.needs.iter().any(|n| {
                    tainted.contains(n.as_str())
                        || self.goals.get(n).is_some_and(|d| d.state == State::Failed)
                });
                if bad {
                    tainted.insert(key.as_str());
                }
            }
        }
        tainted
            .into_iter()
            .filter(|id| {
                self.goals
                    .get(*id)
                    .is_some_and(|g| g.state == State::Pending)
            })
            .map(str::to_string)
            .collect()
    }

    pub fn summary(&self) -> Json {
        let count = |s: State| self.goals.values().filter(|g| g.state == s).count();
        json!({
            "goals": self.goals.len(),
            "layers": self.layers().len(),
            "pending": count(State::Pending),
            "issued": count(State::Issued),
            "done": count(State::Done),
            "failed": count(State::Failed),
            "blocked": self.blocked().len(),
            "ready": self.ready(MAX_GOALS).len(),
        })
    }

    /// The `uriel.ledger.v1` form: goals sorted by id, needs sorted.
    pub fn to_json(&self) -> Json {
        let goals: Vec<Json> = self
            .goals
            .iter()
            .map(|(id, g)| {
                json!({
                    "id": id,
                    "title": g.title,
                    "detail": g.detail,
                    "result": g.result,
                    "needs": g.needs,
                    "state": g.state.name(),
                })
            })
            .collect();
        json!({"schema": SCHEMA, "name": self.name, "goals": goals})
    }

    /// Re-validate a stored ledger. The rules match `scripts/ledger_next.py`: schema, name, at most
    /// 256 goals, unique valid ids, at most 16 needs each, none self-referencing, a known state,
    /// text within its byte cap and free of control or hidden characters, every need present, no
    /// cycle.
    pub fn from_json(raw: &Json) -> Result<Ledger, Error> {
        let bad = |why: &str| Error::Malformed(why.to_string());
        let obj = raw.as_object().ok_or_else(|| bad("not an object"))?;
        if obj.get("schema").and_then(Json::as_str) != Some(SCHEMA) {
            return Err(bad("unknown schema"));
        }
        let name = obj
            .get("name")
            .and_then(Json::as_str)
            .ok_or_else(|| bad("bad ledger name"))?;
        let mut ledger = Ledger::new(name).map_err(|_| bad("bad ledger name"))?;
        let items = obj
            .get("goals")
            .and_then(Json::as_array)
            .ok_or_else(|| bad("bad goals list"))?;
        if items.len() > MAX_GOALS {
            return Err(bad("bad goals list"));
        }
        for item in items {
            let o = item
                .as_object()
                .ok_or_else(|| bad("a goal is not an object"))?;
            let id = o.get("id").and_then(Json::as_str).filter(|s| valid_id(s));
            let id = id.ok_or_else(|| bad("bad or duplicate goal id"))?;
            if ledger.goals.contains_key(id) {
                return Err(bad("bad or duplicate goal id"));
            }
            let raw_needs = o
                .get("needs")
                .and_then(Json::as_array)
                .ok_or_else(|| bad("bad needs"))?;
            if raw_needs.len() > MAX_NEEDS {
                return Err(bad("bad needs"));
            }
            let mut needs = Vec::with_capacity(raw_needs.len());
            for n in raw_needs {
                let n = n
                    .as_str()
                    .filter(|s| valid_id(s) && *s != id)
                    .ok_or_else(|| bad("bad need"))?;
                needs.push(n.to_string());
            }
            let state = o
                .get("state")
                .and_then(Json::as_str)
                .and_then(State::parse)
                .ok_or_else(|| bad("bad state"))?;
            let text = |key: &str, cap: usize| -> Result<String, Error> {
                let s = o
                    .get(key)
                    .and_then(Json::as_str)
                    .ok_or_else(|| bad(&format!("{key} is not a string")))?;
                if s.len() > cap {
                    return Err(bad(&format!("{key} exceeds {cap} bytes")));
                }
                if !text_is_clean(s) {
                    return Err(bad(&format!(
                        "{key} contains a control or hidden character"
                    )));
                }
                Ok(s.to_string())
            };
            ledger.goals.insert(
                id.to_string(),
                Goal {
                    title: text("title", MAX_TITLE_BYTES)?,
                    detail: text("detail", MAX_DETAIL_BYTES)?,
                    result: text("result", MAX_RESULT_BYTES)?,
                    needs: sorted_set(&needs),
                    state,
                },
            );
        }
        for (id, g) in &ledger.goals {
            for n in &g.needs {
                if !ledger.goals.contains_key(n) {
                    return Err(bad(&format!("goal '{id}' names an unknown need")));
                }
            }
        }
        layers_of(&ledger.goals)?;
        Ok(ledger)
    }
}
