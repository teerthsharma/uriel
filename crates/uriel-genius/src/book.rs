use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde_json::{json, Value as Json};
use uriel_fleet::{sanitize, Policy, Report};

use crate::claim::{merge, parse, Claim, Outcome, FIELD_BYTES, MAX_CLAIMS};
use crate::error::Error;
use crate::hex::Vertex;
use crate::plan::{Mode, Plan};
use crate::verdict;

pub const MAX_UNKNOWNS: usize = 6;
pub const MAX_PLANS: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Round1,
    Round2,
    Review,
}

impl Phase {
    pub fn name(self) -> &'static str {
        match self {
            Phase::Round1 => "round 1",
            Phase::Round2 => "round 2",
            Phase::Review => "review",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Sub {
    pub claims: Vec<Claim>,
    pub unknowns: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct Test {
    pub outcome: Outcome,
    pub evidence: Vec<String>,
}

#[derive(Debug)]
pub(crate) struct State {
    pub plan: Plan,
    pub r1: [Option<Sub>; 6],
    pub r2: [Option<Sub>; 6],
    pub tests: BTreeMap<String, Test>,
}

impl State {
    fn phase(&self) -> Phase {
        if self.r1.iter().any(Option::is_none) {
            Phase::Round1
        } else if self.r2.iter().any(Option::is_none) {
            Phase::Round2
        } else {
            Phase::Review
        }
    }

    fn all_claims(&self) -> impl Iterator<Item = &Claim> {
        self.r1
            .iter()
            .chain(self.r2.iter())
            .flatten()
            .flat_map(|s| s.claims.iter())
    }

    fn round2_brief(&self, v: Vertex) -> String {
        let shown: Vec<(Vertex, Vec<&Claim>)> = v
            .seen_by()
            .iter()
            .map(|s| {
                let claims = self.r1[s.index()]
                    .as_ref()
                    .map(|sub| sub.claims.iter().collect())
                    .unwrap_or_default();
                (*s, claims)
            })
            .collect();
        self.plan.brief2(v, &shown)
    }
}

#[derive(Debug, Clone)]
pub struct Submitted {
    pub plan: String,
    pub accepted: usize,
    pub phase: Phase,
    pub report: Report,
    pub round2_ready: bool,
    pub verdict: Option<Json>,
}

impl Submitted {
    pub fn to_json(&self) -> Json {
        let mut out = json!({
            "plan": self.plan,
            "accepted": self.accepted,
            "phase": self.phase.name(),
            "sanitised": self.report.to_json()
        });
        if self.round2_ready {
            out["round2_ready"] = Json::Bool(true);
            out["next"] = Json::String(
                "round 2 is open: call genius_brief {plan, vertex, round: 2} once per vertex".to_string(),
            );
        }
        if let Some(v) = &self.verdict {
            out["verdict"] = v.clone();
        }
        out
    }
}

#[derive(Debug, Default)]
pub struct Book {
    plans: BTreeMap<String, State>,
    order: VecDeque<String>,
}

fn text_list(raw: Option<&Json>, report: &mut Report) -> Result<Vec<String>, Error> {
    match raw {
        None | Some(Json::Null) => Ok(Vec::new()),
        Some(Json::Array(items)) => {
            if items.len() > MAX_UNKNOWNS {
                return Err(Error::Malformed(format!("at most {MAX_UNKNOWNS} unknowns")));
            }
            let mut out = Vec::new();
            for v in items {
                let s = v
                    .as_str()
                    .ok_or_else(|| Error::Malformed("unknowns must be strings".into()))?;
                let clean = sanitize(s, &Policy::new(FIELD_BYTES));
                merge(report, &clean.report);
                let t = clean.text.trim().to_string();
                if !t.is_empty() {
                    out.push(t);
                }
            }
            Ok(out)
        }
        Some(_) => Err(Error::Malformed("unknowns must be an array".into())),
    }
}

impl Book {
    pub fn new() -> Book {
        Book::default()
    }

    pub fn open(&mut self, plan: Plan) -> &Plan {
        let id = plan.id.clone();
        if let Some(state) = self.plans.get_mut(&id) {
            state.plan = plan;
        } else {
            while self.plans.len() >= MAX_PLANS {
                match self.order.pop_front() {
                    Some(old) => {
                        self.plans.remove(&old);
                    }
                    None => break,
                }
            }
            self.order.push_back(id.clone());
            self.plans.insert(
                id.clone(),
                State {
                    plan,
                    r1: Default::default(),
                    r2: Default::default(),
                    tests: BTreeMap::new(),
                },
            );
        }
        &self.plans[&id].plan
    }

    pub fn plan(&self, id: &str) -> Option<&Plan> {
        self.plans.get(id).map(|s| &s.plan)
    }

    pub fn phase(&self, id: &str) -> Option<Phase> {
        self.plans.get(id).map(State::phase)
    }

    pub fn brief(&self, id: &str, vertex: Option<Vertex>, round: u8) -> Result<String, Error> {
        let st = self
            .plans
            .get(id)
            .ok_or_else(|| Error::UnknownPlan(id.to_string()))?;
        let need = || Error::Malformed("'vertex' is required for this brief".into());
        match round {
            1 => match (st.plan.opts.mode, vertex) {
                (Mode::Tier, Some(v)) => Ok(st.plan.brief1(v)),
                (Mode::Tier, None) => Err(need()),
                (Mode::Fork, None) => Ok(st.plan.shared().to_string()),
                (Mode::Fork, Some(v)) => Ok(st.plan.lens(v)),
            },
            2 => {
                if st.phase() == Phase::Round1 {
                    return Err(Error::WrongRound { got: "2".into(), expected: Phase::Round1.name().into() });
                }
                vertex.map(|v| st.round2_brief(v)).ok_or_else(need)
            }
            other => Err(Error::Malformed(format!("round must be 1 or 2, got {other}"))),
        }
    }

    pub fn bound_goal(&self, plan: &str, claim: &str) -> Option<(String, String)> {
        let st = self.plans.get(plan)?;
        st.all_claims()
            .find(|c| c.id == claim)
            .and_then(|c| c.goal.clone().map(|g| (g, c.text.clone())))
    }

    pub fn verdict(&self, id: &str) -> Result<Json, Error> {
        let st = self
            .plans
            .get(id)
            .ok_or_else(|| Error::UnknownPlan(id.to_string()))?;
        Ok(verdict::compute(st))
    }

    pub fn submit(&mut self, id: &str, input: &Json) -> Result<Submitted, Error> {
        let st = self
            .plans
            .get_mut(id)
            .ok_or_else(|| Error::UnknownPlan(id.to_string()))?;
        let obj = input
            .as_object()
            .ok_or_else(|| Error::Malformed("the submission must be an object".into()))?;
        if obj.contains_key("experiment") {
            return experiment(st, obj.get("experiment"));
        }
        claims(st, obj)
    }
}

fn claims(st: &mut State, obj: &serde_json::Map<String, Json>) -> Result<Submitted, Error> {
    let phase = st.phase();
    let round: u8 = match phase {
        Phase::Round1 => 1,
        Phase::Round2 => 2,
        Phase::Review => {
            return Err(Error::WrongRound { got: "claims".into(), expected: Phase::Review.name().into() })
        }
    };
    if let Some(given) = obj.get("round").filter(|r| !r.is_null()) {
        if given.as_u64() != Some(u64::from(round)) {
            return Err(Error::WrongRound { got: given.to_string(), expected: phase.name().into() });
        }
    }
    let vname = obj
        .get("vertex")
        .and_then(Json::as_str)
        .ok_or_else(|| Error::Malformed("'vertex' is required".into()))?;
    let owner = Vertex::parse(vname).ok_or_else(|| Error::BadVertex(vname.to_string()))?;
    let slot = if round == 1 { &st.r1 } else { &st.r2 };
    if slot[owner.index()].is_some() {
        return Err(Error::Duplicate { vertex: vname.to_string(), round });
    }
    let raw = match obj.get("claims") {
        Some(Json::Array(a)) => a,
        _ => return Err(Error::Malformed("'claims' must be an array".into())),
    };
    if raw.len() > MAX_CLAIMS {
        return Err(Error::TooManyClaims(MAX_CLAIMS));
    }
    let mut report = Report::default();
    let mut parsed = Vec::with_capacity(raw.len());
    let mut seen = BTreeSet::new();
    for r in raw {
        let c = parse(r, owner, round, &mut report)?;
        if !seen.insert(c.id.clone()) {
            return Err(Error::DuplicateClaimId(c.id));
        }
        parsed.push(c);
    }
    if round == 2 {
        let allowed: BTreeSet<&str> = owner
            .seen_by()
            .iter()
            .filter_map(|s| st.r1[s.index()].as_ref())
            .flat_map(|sub| sub.claims.iter().map(|c| c.id.as_str()))
            .collect();
        for c in &parsed {
            for t in &c.targets {
                if !allowed.contains(t.as_str()) {
                    return Err(Error::BadTarget { claim: c.id.clone(), target: t.clone() });
                }
            }
        }
        if !parsed.iter().any(|c| !c.targets.is_empty()) {
            return Err(Error::NoEngagement);
        }
    }
    let unknowns = text_list(obj.get("unknowns"), &mut report)?;
    let accepted = parsed.len();
    let sub = Sub { claims: parsed, unknowns };
    if round == 1 {
        st.r1[owner.index()] = Some(sub);
    } else {
        st.r2[owner.index()] = Some(sub);
    }
    let after = st.phase();
    let round2_ready = phase == Phase::Round1 && after == Phase::Round2;
    let verdict = (after == Phase::Review).then(|| verdict::compute(st));
    Ok(Submitted { plan: st.plan.id.clone(), accepted, phase: after, report, round2_ready, verdict })
}

fn experiment(st: &mut State, raw: Option<&Json>) -> Result<Submitted, Error> {
    let phase = st.phase();
    if phase != Phase::Review {
        return Err(Error::TooEarly(phase.name().to_string()));
    }
    let items = match raw {
        Some(Json::Array(a)) if !a.is_empty() && a.len() <= MAX_CLAIMS => a,
        _ => return Err(Error::Malformed("'experiment' must be a non-empty array of at most 8 results".into())),
    };
    let mut report = Report::default();
    let mut staged: Vec<(String, Test)> = Vec::new();
    for item in items {
        let o = item
            .as_object()
            .ok_or_else(|| Error::Malformed("each experiment must be an object".into()))?;
        let claim = o
            .get("claim")
            .and_then(Json::as_str)
            .ok_or_else(|| Error::Malformed("each experiment needs a string 'claim'".into()))?
            .to_string();
        let testable = st.all_claims().any(|c| c.id == claim && c.kind.testable());
        if !testable {
            return Err(Error::NotTestable(claim));
        }
        if st.tests.contains_key(&claim) || staged.iter().any(|(id, _)| id == &claim) {
            return Err(Error::AlreadyTested(claim));
        }
        let outcome = o
            .get("outcome")
            .and_then(Json::as_str)
            .and_then(Outcome::parse)
            .ok_or_else(|| Error::Malformed("'outcome' must be survived or killed".into()))?;
        let evidence = text_list(o.get("evidence"), &mut report)?;
        if evidence.is_empty() {
            return Err(Error::ExperimentNeedsEvidence(claim));
        }
        staged.push((claim, Test { outcome, evidence }));
    }
    let accepted = staged.len();
    st.tests.extend(staged);
    Ok(Submitted {
        plan: st.plan.id.clone(),
        accepted,
        phase: Phase::Review,
        report,
        round2_ready: false,
        verdict: Some(verdict::compute(st)),
    })
}
