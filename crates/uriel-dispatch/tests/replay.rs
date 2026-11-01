mod common;

use common::*;
use uriel_dispatch::*;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

struct Sim {
    h: H,
    rng: Rng,
    n: u64,
    evals: Vec<(EvalId, Vec<TriggerId>)>,
}

impl Sim {
    fn new(seed: u64, cfg: RouterConfig) -> Sim {
        Sim {
            h: H::with(cfg, with_mode(AcceptMode::Auto)),
            rng: Rng(seed),
            n: 0,
            evals: Vec::new(),
        }
    }

    fn absorb(&mut self, fx: &[Effect]) {
        for e in fx {
            if let Effect::StartEval { eval, batch, .. } = e {
                self.evals
                    .push((*eval, batch.iter().map(|c| c.trigger.clone()).collect()));
            }
        }
    }

    fn step(&mut self) {
        let dt = [1, 10, 1_000, 60_000, 700_000][self.rng.below(5) as usize];
        let t = self.rng.below(8);
        let input = match self.rng.below(24) {
            0..=5 => {
                self.n += 1;
                let thread = format!("th{}", self.rng.below(6));
                let (who, bot, addr, scope) = match self.rng.below(5) {
                    0 => (Some("alice"), false, true, "issues"),
                    1 => (Some("mallory"), false, self.rng.below(2) == 0, "issues"),
                    2 => (Some("dependabot[bot]"), true, true, "all"),
                    3 => (None, false, false, "all"),
                    _ => (Some("carol"), false, false, "issues"),
                };
                Input::Ingest(trigger(
                    &format!("t{}", self.n),
                    who.map(|w| (w, bot)),
                    addr,
                    &thread,
                    scope,
                ))
            }
            6 | 7 => match self.evals.pop() {
                Some((eval, batch)) => Input::EvalResult {
                    eval,
                    verdicts: batch
                        .into_iter()
                        .map(|t| {
                            let v = match self.rng.below(5) {
                                0..=2 => accept(&["deps"]),
                                3 => Verdict::Ignore { from_card: true },
                                _ => Verdict::Attach {
                                    ticket: TicketId(1 + self.rng.below(8)),
                                },
                            };
                            (t, v)
                        })
                        .collect(),
                },
                None => Input::Tick,
            },
            8 => match self.evals.last() {
                Some((eval, _)) => Input::EvalDelivered { eval: *eval },
                None => Input::Tick,
            },
            9 => Input::PrepareDone {
                ticket: TicketId(1 + t),
            },
            10 => Input::TurnEnded {
                ticket: TicketId(1 + t),
                background: self.rng.below(3) == 0,
            },
            11 => Input::BackgroundDone {
                ticket: TicketId(1 + t),
            },
            12 => Input::Report {
                ticket: TicketId(1 + t),
                report: [
                    Report::Progress,
                    Report::Waiting,
                    Report::Ready,
                    Report::Blocked,
                    Report::Failed,
                    Report::Dropped,
                ][self.rng.below(6) as usize],
                attention: (self.rng.below(4) == 0).then(|| "q".into()),
            },
            13 => Input::Pressure {
                free_mb: self.rng.below(20_000),
                total_mb: 16_000,
            },
            14 => Input::ThreadClosed {
                thread: ThreadId::new(format!("th{}", self.rng.below(6))),
            },
            15 => Input::Follow {
                ticket: TicketId(1 + t),
                thread: ThreadId::new(format!("th{}", self.rng.below(6))),
                kind: [SubKind::Origin, SubKind::Attached, SubKind::Context]
                    [self.rng.below(3) as usize],
            },
            16 => Input::Unfollow {
                ticket: TicketId(1 + t),
                thread: ThreadId::new(format!("th{}", self.rng.below(6))),
            },
            17 => Input::HandBack {
                ticket: TicketId(1 + t),
                trigger: TriggerId::new(format!("t{}", 1 + self.rng.below(self.n.max(1)))),
                reason: "r".into(),
            },
            18 => Input::DeliveryFailed {
                ticket: TicketId(1 + t),
            },
            19 => Input::DeliveryOk {
                ticket: TicketId(1 + t),
            },
            20 => Input::ManualStop {
                ticket: TicketId(1 + t),
            },
            21 => Input::PrepareFailed {
                ticket: TicketId(1 + t),
            },
            22 => {
                let mut p = with_mode(
                    [AcceptMode::Off, AcceptMode::Observe, AcceptMode::Auto]
                        [self.rng.below(3) as usize],
                );
                if self.rng.below(2) == 0 {
                    p.sources
                        .get_mut("gh")
                        .unwrap()
                        .allowed
                        .insert("carol".into());
                }
                Input::SetPolicy(p)
            }
            _ => Input::Tick,
        };
        let fx = self.h.at(dt, input);
        self.absorb(&fx);
        self.check();
    }

    fn check(&self) {
        let r = &self.h.r;
        let (regular, reserve) = r.slots_held();
        assert!(
            regular <= r.config().max_workers,
            "regular slots over the worker limit"
        );
        assert!(reserve <= 1, "more than one reserve slot");
        for t in r.tickets() {
            if t.status != Status::Open {
                assert!(!t.holds_slot(), "ended ticket holds a slot");
                assert!(t.owned.is_empty(), "an ended ticket still owns messages");
            }
            if let Some(Phase::Preparing { .. } | Phase::Turn | Phase::Background { .. }) =
                Some(t.phase)
            {
                assert!(t.holds_slot(), "a running ticket must hold a slot");
            }
            if matches!(t.phase, Phase::Parked | Phase::Queued { .. }) {
                assert!(!t.holds_slot(), "a parked ticket holds a slot");
            }
        }
    }

    fn drain(&mut self) {
        while let Some((eval, batch)) = self.evals.pop() {
            let fx = self.h.go(Input::EvalResult {
                eval,
                verdicts: batch
                    .into_iter()
                    .map(|t| (t, Verdict::Ignore { from_card: false }))
                    .collect(),
            });
            self.absorb(&fx);
        }
    }
}

fn run(seed: u64, steps: usize) -> Sim {
    let mut s = Sim::new(seed, RouterConfig::default());
    for _ in 0..steps {
        s.step();
    }
    s
}

#[test]
fn slot_and_phase_invariants_hold_over_random_event_streams() {
    for seed in 1..=40 {
        run(seed * 7919, 600);
    }
}

#[test]
fn no_admitted_trigger_is_lost_once_the_evals_drain() {
    for seed in 1..=20 {
        let mut s = run(seed * 104_729, 500);
        s.drain();
        s.drain();
        let r = &s.h.r;
        for id in r.trigger_ids() {
            let d = r.dispositions(id);
            let queued_only = d.is_empty();
            assert!(
                !queued_only,
                "trigger {id:?} has no disposition (seed {seed})"
            );
        }
    }
}

#[test]
fn the_random_streams_reach_every_ticket_ending_and_the_reserve_slot() {
    let mut ended = std::collections::BTreeSet::new();
    let mut reserve_seen = false;
    let mut tickets = 0;
    let mut resumed = 0;
    for seed in 1..=40u64 {
        let mut s = Sim::new(seed * 7919, RouterConfig::default());
        for _ in 0..600 {
            s.step();
            reserve_seen |= s.h.r.slots_held().1 == 1;
        }
        tickets += s.h.r.tickets().count();
        for t in s.h.r.tickets() {
            resumed += t.assignments.len() - 1;
            if let Status::Ended(e) = t.status {
                ended.insert(match e {
                    End::Done => "done",
                    End::Closed => "closed",
                    End::Stopped => "stopped",
                    End::Revoked(_) => "revoked",
                });
            }
        }
    }
    assert!(tickets >= 200, "only {tickets} tickets");
    assert!(resumed >= 5, "only {resumed} resumes");
    assert!(reserve_seen, "the reserve slot was never used");
    assert_eq!(ended.len(), 4, "{ended:?}");
}

#[test]
fn replaying_the_recorded_inputs_reproduces_the_digest_exactly() {
    for seed in [3, 17, 99, 12_345] {
        let s = run(seed, 800);
        let again = replay(
            RouterConfig::default(),
            with_mode(AcceptMode::Auto),
            s.h.r.inputs(),
        );
        assert_eq!(again.digest(), s.h.r.digest(), "seed {seed}");
        assert_eq!(again.decisions(), s.h.r.decisions());
        assert_eq!(again.tickets().count(), s.h.r.tickets().count());
    }
}

#[test]
fn the_digest_survives_a_json_round_trip_of_the_log() {
    let s = run(2024, 400);
    let json = serde_json::to_string(s.h.r.inputs()).unwrap();
    let log: Vec<(Ms, Input)> = serde_json::from_str(&json).unwrap();
    let again = replay(RouterConfig::default(), with_mode(AcceptMode::Auto), &log);
    assert_eq!(again.digest(), s.h.r.digest());
}

#[test]
fn one_changed_input_changes_the_digest() {
    let s = run(5, 300);
    let mut log = s.h.r.inputs().to_vec();
    log.push((
        u64::MAX - 1,
        Input::Pressure {
            free_mb: 1,
            total_mb: 1,
        },
    ));
    let changed = replay(RouterConfig::default(), with_mode(AcceptMode::Auto), &log);
    assert_ne!(changed.digest(), s.h.r.digest());
}

#[test]
fn the_same_seed_gives_the_same_digest_and_different_seeds_differ() {
    assert_eq!(run(77, 300).h.r.digest(), run(77, 300).h.r.digest());
    assert_ne!(run(77, 300).h.r.digest(), run(78, 300).h.r.digest());
}

#[test]
fn a_clock_that_runs_backwards_is_clamped_and_never_reopens_a_timer() {
    let mut h = H::new(policy());
    let eval = started(&h.ingest(person("a", "alice", true, "th"))).unwrap();
    h.go(Input::EvalDelivered { eval });
    let before = h.r.now();
    h.r.apply(before.saturating_sub(500_000), Input::Tick);
    assert_eq!(h.r.now(), before);
    let fx = h.r.apply(before + 120_000, Input::Tick);
    assert!(has(&fx, |e| matches!(e, Effect::EvalExpired { .. })));
}

#[test]
fn garbage_ids_never_panic_and_never_mutate_state() {
    let mut h = H::new(policy());
    let ghost = TicketId(4040);
    let inputs = vec![
        Input::PrepareDone { ticket: ghost },
        Input::PrepareFailed { ticket: ghost },
        Input::TurnEnded {
            ticket: ghost,
            background: true,
        },
        Input::BackgroundDone { ticket: ghost },
        Input::Report {
            ticket: ghost,
            report: Report::Ready,
            attention: None,
        },
        Input::Ack {
            ticket: ghost,
            trigger: TriggerId::new("x"),
        },
        Input::HandBack {
            ticket: ghost,
            trigger: TriggerId::new("x"),
            reason: "r".into(),
        },
        Input::Follow {
            ticket: ghost,
            thread: ThreadId::new("t"),
            kind: SubKind::Context,
        },
        Input::Unfollow {
            ticket: ghost,
            thread: ThreadId::new("t"),
        },
        Input::DropThread {
            thread: ThreadId::new("t"),
        },
        Input::ThreadClosed {
            thread: ThreadId::new("t"),
        },
        Input::DeliveryFailed { ticket: ghost },
        Input::DeliveryOk { ticket: ghost },
        Input::ManualStop { ticket: ghost },
        Input::StopOwned {
            trigger: TriggerId::new("x"),
            ticket: ghost,
        },
        Input::EvalDelivered { eval: EvalId(9) },
        Input::EvalResult {
            eval: EvalId(9),
            verdicts: vec![],
        },
        Input::EvalFailed { eval: EvalId(9) },
        Input::Reeval {
            thread: ThreadId::new("t"),
        },
        Input::Dismiss {
            thread: ThreadId::new("t"),
        },
    ];
    for i in inputs {
        h.go(i);
    }
    assert_eq!(h.r.tickets().count(), 0);
    assert!(h.r.decisions().is_empty());
    assert_eq!(h.r.slots_held(), (0, 0));
}

#[test]
fn reeval_requeues_an_unavailable_batch_and_dismiss_silences_the_panel() {
    let mut h = H::new(policy());
    let eval = started(&h.ingest(person("a", "alice", true, "th"))).unwrap();
    h.go(Input::EvalFailed { eval });
    assert_eq!(h.r.attention().len(), 1);
    let fx = h.go(Input::Reeval {
        thread: ThreadId::new("th"),
    });
    assert_eq!(batch(&fx)[0].trigger, TriggerId::new("a"));
    assert!(
        h.r.attention().is_empty(),
        "evaluating again is not a problem to show"
    );
    let eval = started(&fx).unwrap();
    h.go(Input::EvalFailed { eval });
    h.go(Input::Dismiss {
        thread: ThreadId::new("th"),
    });
    assert!(h.r.attention().is_empty());
    assert_eq!(h.r.eval_health(), 0);
}

#[test]
fn an_unavailable_decision_stops_asking_for_attention_after_a_day() {
    let mut h = H::new(policy());
    let eval = started(&h.ingest(person("a", "alice", true, "th"))).unwrap();
    h.go(Input::EvalFailed { eval });
    h.at(23 * 3_600_000, Input::Tick);
    assert_eq!(h.r.attention().len(), 1);
    h.at(2 * 3_600_000, Input::Tick);
    assert!(h.r.attention().is_empty());
}
