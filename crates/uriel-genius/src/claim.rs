use serde_json::{json, Value as Json};
use uriel_fleet::{sanitize, Policy, Report};

use crate::error::Error;
use crate::hex::Vertex;

pub const MAX_CLAIMS: usize = 8;
pub const MAX_EVIDENCE: usize = 6;
pub const TEXT_BYTES: usize = 400;
pub const FIELD_BYTES: usize = 240;
pub const WILD_MAX_CONFIDENCE: u16 = 800;
pub const SHOWN_TEXT: usize = 240;
pub const SHOWN_FALSIFIER: usize = 160;

pub fn cut_chars(text: &str, limit: usize) -> String {
    match text.char_indices().nth(limit) {
        Some((i, _)) => format!("{}...", &text[..i]),
        None => text.to_string(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Observation,
    Conjecture,
    Wild,
    Question,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Observation => "observation",
            Kind::Conjecture => "conjecture",
            Kind::Wild => "wild",
            Kind::Question => "question",
        }
    }

    fn parse(s: &str) -> Option<Kind> {
        [Kind::Observation, Kind::Conjecture, Kind::Wild, Kind::Question]
            .into_iter()
            .find(|k| k.name() == s)
    }

    pub fn testable(self) -> bool {
        matches!(self, Kind::Conjecture | Kind::Wild)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Cost {
    S,
    M,
    L,
}

impl Cost {
    pub fn name(self) -> &'static str {
        match self {
            Cost::S => "S",
            Cost::M => "M",
            Cost::L => "L",
        }
    }

    fn parse(s: &str) -> Option<Cost> {
        [Cost::S, Cost::M, Cost::L].into_iter().find(|c| c.name() == s)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stance {
    Support,
    Attack,
}

impl Stance {
    pub fn name(self) -> &'static str {
        match self {
            Stance::Support => "support",
            Stance::Attack => "attack",
        }
    }

    fn parse(s: &str) -> Option<Stance> {
        [Stance::Support, Stance::Attack].into_iter().find(|c| c.name() == s)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Survived,
    Killed,
}

impl Outcome {
    pub fn name(self) -> &'static str {
        match self {
            Outcome::Survived => "survived",
            Outcome::Killed => "killed",
        }
    }

    pub fn parse(s: &str) -> Option<Outcome> {
        [Outcome::Survived, Outcome::Killed].into_iter().find(|c| c.name() == s)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claim {
    pub id: String,
    pub owner: Vertex,
    pub round: u8,
    pub kind: Kind,
    pub text: String,
    pub falsifier: String,
    pub test: String,
    pub cost: Option<Cost>,
    pub confidence: u16,
    pub evidence: Vec<String>,
    pub targets: Vec<String>,
    pub stance: Option<Stance>,
}

impl Claim {
    pub fn to_json(&self) -> Json {
        json!({
            "id": self.id,
            "kind": self.kind.name(),
            "text": self.text,
            "falsifier": self.falsifier,
            "test": self.test,
            "cost": self.cost.map(Cost::name),
            "confidence": self.confidence,
            "evidence": self.evidence,
            "targets": self.targets,
            "stance": self.stance.map(Stance::name)
        })
    }

    pub fn shown_json(&self) -> Json {
        json!({
            "id": self.id,
            "kind": self.kind.name(),
            "text": cut_chars(&self.text, SHOWN_TEXT),
            "falsifier": cut_chars(&self.falsifier, SHOWN_FALSIFIER),
            "cost": self.cost.map(Cost::name)
        })
    }
}

pub fn merge(into: &mut Report, from: &Report) {
    into.truncated |= from.truncated;
    into.ansi += from.ansi;
    into.controls += from.controls;
    into.hidden += from.hidden;
    into.tags += from.tags;
    into.fences += from.fences;
    into.injection += from.injection;
    for (class, n) in &from.secrets {
        *into.secrets.entry(class).or_insert(0) += n;
    }
}

fn text_field(
    obj: &serde_json::Map<String, Json>,
    key: &'static str,
    cap: usize,
    id: &str,
    report: &mut Report,
) -> Result<String, Error> {
    match obj.get(key) {
        None | Some(Json::Null) => Ok(String::new()),
        Some(Json::String(s)) => {
            let clean = sanitize(s, &Policy::new(cap));
            merge(report, &clean.report);
            Ok(clean.text.trim().to_string())
        }
        Some(_) => Err(Error::Field { claim: id.to_string(), field: key, problem: "not a string" }),
    }
}

fn list_field(
    obj: &serde_json::Map<String, Json>,
    key: &'static str,
    id: &str,
    report: &mut Report,
) -> Result<Vec<String>, Error> {
    match obj.get(key) {
        None | Some(Json::Null) => Ok(Vec::new()),
        Some(Json::Array(items)) => {
            if items.len() > MAX_EVIDENCE {
                return Err(Error::Field { claim: id.to_string(), field: key, problem: "too long" });
            }
            items
                .iter()
                .map(|v| match v {
                    Json::String(s) => {
                        let clean = sanitize(s, &Policy::new(FIELD_BYTES));
                        merge(report, &clean.report);
                        Ok(clean.text.trim().to_string())
                    }
                    _ => Err(Error::Field { claim: id.to_string(), field: key, problem: "not an array of strings" }),
                })
                .filter(|r| r.as_ref().map_or(true, |s| !s.is_empty()))
                .collect()
        }
        Some(_) => Err(Error::Field { claim: id.to_string(), field: key, problem: "not an array" }),
    }
}

pub fn id_shape(owner: Vertex, round: u8) -> String {
    format!("{}.{}<N>", owner.name(), if round == 1 { "c" } else { "d" })
}

pub fn id_ok(id: &str, owner: Vertex, round: u8) -> bool {
    let want = format!("{}.{}", owner.name(), if round == 1 { "c" } else { "d" });
    id.strip_prefix(&want)
        .is_some_and(|n| (1..=2).contains(&n.len()) && n.bytes().all(|b| b.is_ascii_digit()) && !n.starts_with('0'))
}

pub fn parse(raw: &Json, owner: Vertex, round: u8, report: &mut Report) -> Result<Claim, Error> {
    let obj = raw
        .as_object()
        .ok_or_else(|| Error::Malformed("each claim must be an object".into()))?;
    let id = obj
        .get("id")
        .and_then(Json::as_str)
        .ok_or_else(|| Error::Malformed("each claim needs a string id".into()))?
        .to_string();
    if !id_ok(&id, owner, round) {
        return Err(Error::BadClaimId(id, id_shape(owner, round)));
    }
    let kind_name = obj.get("kind").and_then(Json::as_str).unwrap_or("");
    let kind = Kind::parse(kind_name).ok_or(Error::Field {
        claim: id.clone(),
        field: "kind",
        problem: "not one of observation, conjecture, wild, question",
    })?;
    let text = text_field(obj, "text", TEXT_BYTES, &id, report)?;
    if text.is_empty() {
        return Err(Error::Field { claim: id, field: "text", problem: "missing" });
    }
    let falsifier = text_field(obj, "falsifier", FIELD_BYTES, &id, report)?;
    let test = text_field(obj, "test", FIELD_BYTES, &id, report)?;
    let evidence = list_field(obj, "evidence", &id, report)?;
    let targets = list_field(obj, "targets", &id, report)?;
    let cost = match obj.get("cost") {
        None | Some(Json::Null) => None,
        Some(Json::String(s)) => Some(Cost::parse(s).ok_or(Error::Field {
            claim: id.clone(),
            field: "cost",
            problem: "not S, M or L",
        })?),
        Some(_) => return Err(Error::Field { claim: id, field: "cost", problem: "not a string" }),
    };
    let confidence = match obj.get("confidence") {
        None | Some(Json::Null) => 500,
        Some(v) => {
            let x = v.as_f64().filter(|x| (0.0..=1.0).contains(x));
            match x {
                Some(x) => (x * 1000.0).round() as u16,
                None => {
                    return Err(Error::Field { claim: id, field: "confidence", problem: "not a number between 0 and 1" })
                }
            }
        }
    };
    let stance = match obj.get("stance") {
        None | Some(Json::Null) => None,
        Some(Json::String(s)) => Some(Stance::parse(s).ok_or(Error::Field {
            claim: id.clone(),
            field: "stance",
            problem: "not support or attack",
        })?),
        Some(_) => return Err(Error::Field { claim: id, field: "stance", problem: "not a string" }),
    };
    if !targets.is_empty() && stance.is_none() {
        return Err(Error::Field { claim: id, field: "stance", problem: "required when targets are given" });
    }
    if round == 1 && (!targets.is_empty() || stance.is_some()) {
        return Err(Error::Field { claim: id, field: "targets", problem: "not allowed in round 1" });
    }
    match kind {
        Kind::Observation if evidence.is_empty() => return Err(Error::ObservationNeedsEvidence(id)),
        Kind::Conjecture | Kind::Wild => {
            if falsifier.is_empty() {
                return Err(Error::Field { claim: id, field: "falsifier", problem: "missing" });
            }
            if test.is_empty() {
                return Err(Error::Field { claim: id, field: "test", problem: "missing" });
            }
            if cost.is_none() {
                return Err(Error::Field { claim: id, field: "cost", problem: "missing" });
            }
            if kind == Kind::Wild {
                if cost == Some(Cost::L) {
                    return Err(Error::WildNotCheap(id));
                }
                if confidence > WILD_MAX_CONFIDENCE {
                    return Err(Error::WildTooSure(id));
                }
            }
        }
        _ => {}
    }
    if stance == Some(Stance::Attack) && evidence.is_empty() && falsifier.is_empty() {
        return Err(Error::AttackNeedsGround(id));
    }
    Ok(Claim { id, owner, round, kind, text, falsifier, test, cost, confidence, evidence, targets, stance })
}
