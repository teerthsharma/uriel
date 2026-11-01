#![allow(dead_code)]

use uriel_dispatch::*;

pub struct H {
    pub r: Router,
    pub t: Ms,
}

impl H {
    pub fn new(policy: Policy) -> H {
        H::with(RouterConfig::default(), policy)
    }

    pub fn with(cfg: RouterConfig, policy: Policy) -> H {
        H {
            r: Router::new(cfg, policy),
            t: 1_000,
        }
    }

    pub fn at(&mut self, dt: Ms, input: Input) -> Vec<Effect> {
        self.t += dt;
        self.r.apply(self.t, input)
    }

    pub fn go(&mut self, input: Input) -> Vec<Effect> {
        self.at(1, input)
    }

    pub fn ingest(&mut self, t: Trigger) -> Vec<Effect> {
        self.go(Input::Ingest(t))
    }

    pub fn open(&mut self, trigger: &str, thread: &str) -> TicketId {
        let fx = self.ingest(person(trigger, "alice", true, thread));
        let eval = started(&fx).expect("eval started");
        self.go(Input::EvalDelivered { eval });
        let fx = self.go(Input::EvalResult {
            eval,
            verdicts: vec![(TriggerId::new(trigger), accept(&[]))],
        });
        opened(&fx).expect("ticket opened")
    }

    pub fn run_to_turn(&mut self, id: TicketId) {
        self.go(Input::PrepareDone { ticket: id });
    }
}

pub fn policy() -> Policy {
    let mut scopes = std::collections::BTreeMap::new();
    scopes.insert("issues".to_string(), ScopePolicy::default());
    scopes.insert(
        "all".to_string(),
        ScopePolicy {
            receive_all: true,
            admission: Admission::default(),
        },
    );
    let src = SourcePolicy {
        listening: true,
        allowed: ["alice".to_string()].into(),
        bots: ["dependabot[bot]".to_string()].into(),
        scopes,
    };
    let mut rules = std::collections::BTreeMap::new();
    rules.insert(
        "deps".to_string(),
        Rule {
            enabled: true,
            text: "bump dependencies".into(),
        },
    );
    Policy {
        mode: AcceptMode::Observe,
        rules,
        sources: [("gh".to_string(), src)].into(),
    }
}

pub fn with_mode(mode: AcceptMode) -> Policy {
    let mut p = policy();
    p.mode = mode;
    p
}

pub fn person(id: &str, who: &str, addressed: bool, thread: &str) -> Trigger {
    trigger(id, Some((who, false)), addressed, thread, "issues")
}

pub fn bot(id: &str, who: &str, addressed: bool, thread: &str) -> Trigger {
    trigger(id, Some((who, true)), addressed, thread, "issues")
}

pub fn trigger(
    id: &str,
    who: Option<(&str, bool)>,
    addressed: bool,
    thread: &str,
    scope: &str,
) -> Trigger {
    Trigger {
        id: TriggerId::new(id),
        source: "gh".into(),
        scope: scope.into(),
        thread: ThreadId::new(thread),
        actor: who.map(|(id, bot)| Actor { id: id.into(), bot }),
        addressed,
    }
}

pub fn started(fx: &[Effect]) -> Option<EvalId> {
    fx.iter().find_map(|e| match e {
        Effect::StartEval { eval, .. } => Some(*eval),
        _ => None,
    })
}

pub fn batch(fx: &[Effect]) -> Vec<Candidate> {
    fx.iter()
        .find_map(|e| match e {
            Effect::StartEval { batch, .. } => Some(batch.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

pub fn opened(fx: &[Effect]) -> Option<TicketId> {
    fx.iter().find_map(|e| match e {
        Effect::TicketOpened { ticket, .. } => Some(*ticket),
        _ => None,
    })
}

pub fn accept(rules: &[&str]) -> Verdict {
    Verdict::Accept {
        scope: "work".into(),
        rules: rules.iter().map(|s| s.to_string()).collect(),
        assessed: true,
    }
}

pub fn last_decision(h: &H) -> Decision {
    h.r.decisions().last().expect("a decision").decision.clone()
}

pub fn has(fx: &[Effect], f: impl Fn(&Effect) -> bool) -> bool {
    fx.iter().any(f)
}
