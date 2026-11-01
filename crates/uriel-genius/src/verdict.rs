use std::collections::BTreeSet;

use serde_json::{json, Value as Json};
use sha2::{Digest, Sha256};

use crate::book::State;
use crate::claim::{cut_chars, Claim, Cost, Kind, Outcome, Stance};
use crate::hex::{Vertex, AXES, VERTICES};

const SHOWN_TEXT: usize = 120;
const RANKED_CAP: usize = 16;
const QUEUE_CAP: usize = 10;
const REFUTED_CAP: usize = 8;
const EVIDENCE_SHOWN: usize = 2;
const TESTED_CAP: usize = 24;
const QUESTIONS_CAP: usize = 8;
const UNKNOWNS_CAP: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Status {
    Corroborated,
    Grounded,
    Contested,
    Open,
    Quarantined,
    Question,
    Refuted,
}

impl Status {
    fn name(self) -> &'static str {
        match self {
            Status::Corroborated => "corroborated",
            Status::Grounded => "grounded",
            Status::Open => "open",
            Status::Contested => "contested",
            Status::Quarantined => "quarantined",
            Status::Question => "question",
            Status::Refuted => "refuted",
        }
    }
}

struct Row<'a> {
    claim: &'a Claim,
    supporters: BTreeSet<Vertex>,
    attackers: BTreeSet<Vertex>,
    hard: BTreeSet<Vertex>,
    outcome: Option<Outcome>,
}

impl Row<'_> {
    fn status(&self) -> Status {
        match (self.claim.kind, self.outcome) {
            (_, Some(Outcome::Killed)) => Status::Refuted,
            (Kind::Question, _) => Status::Question,
            (Kind::Observation, _) => Status::Grounded,
            (_, Some(Outcome::Survived)) => Status::Corroborated,
            (Kind::Wild, None) => Status::Quarantined,
            (_, None) if !self.attackers.is_empty() => Status::Contested,
            _ => Status::Open,
        }
    }

    fn score(&self) -> i64 {
        let tested = i64::from(self.outcome == Some(Outcome::Survived));
        2 * tested + self.supporters.len() as i64 - self.attackers.len() as i64 - self.hard.len() as i64
    }
}

fn short(text: &str) -> String {
    cut_chars(text, SHOWN_TEXT)
}

fn rows(st: &State) -> Vec<Row<'_>> {
    let r1: Vec<&Claim> = st.r1.iter().flatten().flat_map(|s| s.claims.iter()).collect();
    let r2: Vec<&Claim> = st.r2.iter().flatten().flat_map(|s| s.claims.iter()).collect();
    let mut out: Vec<Row<'_>> = r1
        .iter()
        .chain(r2.iter())
        .map(|c| Row {
            claim: c,
            supporters: BTreeSet::new(),
            attackers: BTreeSet::new(),
            hard: BTreeSet::new(),
            outcome: st.tests.get(&c.id).map(|t| t.outcome),
        })
        .collect();
    for c in &r2 {
        for target in &c.targets {
            if let Some(row) = out.iter_mut().find(|r| &r.claim.id == target) {
                if row.claim.owner == c.owner {
                    continue;
                }
                match c.stance {
                    Some(Stance::Support) => {
                        row.supporters.insert(c.owner);
                    }
                    Some(Stance::Attack) => {
                        row.attackers.insert(c.owner);
                        if !c.evidence.is_empty() {
                            row.hard.insert(c.owner);
                        }
                    }
                    None => {}
                }
            }
        }
    }
    out
}

fn axes(st: &State, rows: &[Row<'_>]) -> Vec<Json> {
    let count = |v: Vertex| {
        st.r1[v.index()].as_ref().map_or(0, |s| s.claims.len())
            + st.r2[v.index()].as_ref().map_or(0, |s| s.claims.len())
    };
    let owner_of = |id: &str| rows.iter().find(|r| r.claim.id == id).map(|r| r.claim.owner);
    AXES.iter()
        .map(|(a, b, label)| {
            let (mut attacks, mut supports) = (0, 0);
            for sub in st.r2.iter().flatten() {
                for c in &sub.claims {
                    let across = (c.owner == *a || c.owner == *b)
                        && c.targets
                            .iter()
                            .any(|t| owner_of(t).is_some_and(|o| (o == *a || o == *b) && o != c.owner));
                    if across {
                        match c.stance {
                            Some(Stance::Attack) => attacks += 1,
                            Some(Stance::Support) => supports += 1,
                            None => {}
                        }
                    }
                }
            }
            json!({
                "pair": [a.name(), b.name()],
                "label": label,
                "claims": [count(*a), count(*b)],
                "cross_attacks": attacks,
                "cross_supports": supports
            })
        })
        .collect()
}

pub(crate) fn compute(st: &State) -> Json {
    let rows = rows(st);
    let mut ranked: Vec<&Row<'_>> = rows.iter().collect();
    ranked.sort_by(|a, b| {
        a.status()
            .cmp(&b.status())
            .then(b.score().cmp(&a.score()))
            .then(a.claim.id.cmp(&b.claim.id))
    });
    let entry = |r: &Row<'_>| {
        json!({
            "id": r.claim.id,
            "owner": r.claim.owner.name(),
            "kind": r.claim.kind.name(),
            "status": r.status().name(),
            "score": r.score(),
            "text": short(&r.claim.text)
        })
    };
    let listed: Vec<Json> = ranked
        .iter()
        .filter(|r| !matches!(r.status(), Status::Question | Status::Refuted))
        .take(RANKED_CAP)
        .map(|r| entry(r))
        .collect();
    let refuted: Vec<Json> = ranked
        .iter()
        .filter(|r| r.status() == Status::Refuted)
        .take(REFUTED_CAP)
        .map(|r| {
            let why: Vec<String> = st
                .tests
                .get(&r.claim.id)
                .map(|t| t.evidence.iter().take(EVIDENCE_SHOWN).map(|e| short(e)).collect())
                .unwrap_or_default();
            json!({"id": r.claim.id, "owner": r.claim.owner.name(), "text": short(&r.claim.text), "killed_by": why})
        })
        .collect();
    let mut queue: Vec<&Row<'_>> = rows
        .iter()
        .filter(|r| r.claim.kind.testable() && r.outcome.is_none())
        .collect();
    queue.sort_by(|a, b| {
        (b.claim.kind == Kind::Wild)
            .cmp(&(a.claim.kind == Kind::Wild))
            .then(a.claim.cost.unwrap_or(Cost::L).cmp(&b.claim.cost.unwrap_or(Cost::L)))
            .then(b.claim.confidence.cmp(&a.claim.confidence))
            .then(a.claim.id.cmp(&b.claim.id))
    });
    let queue_total = queue.len();
    let test_queue: Vec<Json> = queue
        .iter()
        .take(QUEUE_CAP)
        .map(|r| {
            json!({
                "id": r.claim.id,
                "kind": r.claim.kind.name(),
                "cost": r.claim.cost.map(Cost::name),
                "status": r.status().name(),
                "test": short(&r.claim.test),
                "falsifier": short(&r.claim.falsifier)
            })
        })
        .collect();
    let questions: Vec<Json> = rows
        .iter()
        .filter(|r| r.claim.kind == Kind::Question)
        .take(QUESTIONS_CAP)
        .map(|r| json!({"id": r.claim.id, "text": short(&r.claim.text)}))
        .collect();
    let mut unknowns: Vec<&String> = st
        .r1
        .iter()
        .chain(st.r2.iter())
        .flatten()
        .flat_map(|s| s.unknowns.iter())
        .collect();
    unknowns.sort();
    unknowns.dedup();
    let silent: Vec<&str> = VERTICES
        .iter()
        .filter(|v| st.r1[v.index()].as_ref().is_none_or(|s| s.claims.is_empty()))
        .map(|v| v.name())
        .collect();
    let mut counts = serde_json::Map::new();
    for r in &rows {
        let n = counts.entry(r.status().name().to_string()).or_insert(json!(0));
        *n = json!(n.as_u64().unwrap_or(0) + 1);
    }
    let mut out = json!({
        "plan": st.plan.id,
        "ranked": listed,
        "refuted": refuted,
        "test_queue": test_queue,
        "test_queue_total": queue_total,
        "questions": questions,
        "unknowns": unknowns.into_iter().take(UNKNOWNS_CAP).map(|u| short(u)).collect::<Vec<_>>(),
        "silent_vertices": silent,
        "axes": axes(st, &rows),
        "counts": counts,
        "tested": st
            .tests
            .iter()
            .take(TESTED_CAP)
            .map(|(id, t)| json!({"id": id, "outcome": t.outcome.name()}))
            .collect::<Vec<_>>()
    });
    let mut hasher = Sha256::new();
    hasher.update(out.to_string().as_bytes());
    let digest: String = hasher.finalize().iter().take(8).map(|b| format!("{b:02x}")).collect();
    out["digest"] = Json::String(digest);
    out
}
