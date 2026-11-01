mod common;

use common::*;
use uriel_dispatch::*;

#[test]
fn an_explicit_request_becomes_one_eval_and_an_accept_becomes_one_ticket() {
    let mut h = H::new(policy());
    let fx = h.ingest(person("t1", "alice", true, "issue-1"));
    let cands = batch(&fx);
    assert_eq!(cands.len(), 1);
    assert_eq!(cands[0].strength, Strength::Explicit);
    let id = h.open("t2", "issue-2");
    assert_eq!(h.r.ticket(id).unwrap().mandate.basis, Basis::Listed);
    assert_eq!(h.r.ticket(id).unwrap().mandate.policy_version, 1);
}

#[test]
fn accepting_the_same_trigger_twice_creates_one_ticket() {
    let mut h = H::new(policy());
    let fx = h.ingest(person("t1", "alice", true, "issue-1"));
    let eval = started(&fx).unwrap();
    h.go(Input::EvalDelivered { eval });
    let fx = h.go(Input::EvalResult {
        eval,
        verdicts: vec![
            (TriggerId::new("t1"), accept(&[])),
            (TriggerId::new("t1"), accept(&[])),
        ],
    });
    assert_eq!(h.r.tickets().count(), 1);
    assert!(opened(&fx).is_some());
    let again = h.ingest(person("t1", "alice", true, "issue-1"));
    assert!(again.is_empty());
    assert_eq!(
        h.r.dispositions(&TriggerId::new("t1")).last(),
        Some(&Disposition::Duplicate)
    );
    assert_eq!(h.r.tickets().count(), 1);
}

#[test]
fn triggers_arriving_during_an_eval_merge_into_the_next_batch() {
    let mut h = H::new(policy());
    let first = h.ingest(person("a", "alice", true, "th"));
    let e1 = started(&first).unwrap();
    assert!(started(&h.ingest(person("b", "alice", true, "th"))).is_none());
    assert!(started(&h.ingest(person("c", "alice", true, "th"))).is_none());
    let fx = h.go(Input::EvalResult {
        eval: e1,
        verdicts: vec![],
    });
    let next = batch(&fx);
    assert_eq!(
        next.iter()
            .map(|c| c.trigger.0.as_str())
            .collect::<Vec<_>>(),
        ["b", "c"]
    );
}

#[test]
fn at_most_one_eval_runs_per_thread_but_threads_run_in_parallel() {
    let mut h = H::new(policy());
    assert!(started(&h.ingest(person("a", "alice", true, "x"))).is_some());
    assert!(started(&h.ingest(person("b", "alice", true, "y"))).is_some());
    assert!(started(&h.ingest(person("c", "alice", true, "x"))).is_none());
}

#[test]
fn an_inferred_candidate_is_only_recorded_in_observe_mode_and_accepted_in_auto() {
    for (mode, expect_ticket) in [(AcceptMode::Observe, false), (AcceptMode::Auto, true)] {
        let mut h = H::new(with_mode(mode));
        let fx = h.ingest(trigger(
            "b",
            Some(("dependabot[bot]", true)),
            false,
            "pr-9",
            "all",
        ));
        let eval = started(&fx).unwrap();
        assert_eq!(batch(&fx)[0].strength, Strength::Inferred);
        h.go(Input::EvalResult {
            eval,
            verdicts: vec![(TriggerId::new("b"), accept(&["deps"]))],
        });
        assert_eq!(h.r.tickets().count(), usize::from(expect_ticket));
        if !expect_ticket {
            assert_eq!(
                last_decision(&h),
                Decision::WouldAccept {
                    rules: vec!["deps".into()]
                }
            );
        }
    }
}

#[test]
fn off_mode_never_starts_an_eval_for_inferred_triggers_and_records_why() {
    let mut h = H::new(with_mode(AcceptMode::Off));
    let fx = h.ingest(trigger(
        "b",
        Some(("dependabot[bot]", true)),
        false,
        "pr-9",
        "all",
    ));
    assert!(started(&fx).is_none());
    assert_eq!(
        h.r.dispositions(&TriggerId::new("b")),
        [Disposition::Filtered(Refusal::ModeOff)]
    );
}

#[test]
fn a_proactive_accept_needs_an_assessment_and_at_least_one_enabled_known_rule() {
    let mut h = H::new(with_mode(AcceptMode::Auto));
    let cases: Vec<(Verdict, Refusal)> = vec![
        (
            Verdict::Accept {
                scope: "w".into(),
                rules: vec!["deps".into()],
                assessed: false,
            },
            Refusal::NotExplicit,
        ),
        (accept(&[]), Refusal::NoMatchingRule),
        (accept(&["ghost"]), Refusal::UnknownRule),
    ];
    for (i, (verdict, refusal)) in cases.into_iter().enumerate() {
        let id = format!("b{i}");
        let fx = h.ingest(trigger(
            &id,
            Some(("dependabot[bot]", true)),
            false,
            &format!("pr-{i}"),
            "all",
        ));
        h.go(Input::EvalResult {
            eval: started(&fx).unwrap(),
            verdicts: vec![(TriggerId::new(&id), verdict)],
        });
        assert_eq!(last_decision(&h), Decision::Refused(refusal));
    }
    let mut p = with_mode(AcceptMode::Auto);
    p.rules.get_mut("deps").unwrap().enabled = false;
    let mut h = H::new(p);
    let fx = h.ingest(trigger(
        "d",
        Some(("dependabot[bot]", true)),
        false,
        "pr-d",
        "all",
    ));
    h.go(Input::EvalResult {
        eval: started(&fx).unwrap(),
        verdicts: vec![(TriggerId::new("d"), accept(&["deps"]))],
    });
    assert_eq!(last_decision(&h), Decision::Refused(Refusal::DisabledRule));
    assert_eq!(h.r.tickets().count(), 0);
}

#[test]
fn a_result_for_a_discarded_eval_is_rejected_and_creates_no_ticket() {
    let mut h = H::new(policy());
    let eval = started(&h.ingest(person("t", "alice", true, "th"))).unwrap();
    let mut gone = policy();
    gone.sources.get_mut("gh").unwrap().allowed.clear();
    let fx = h.go(Input::SetPolicy(gone));
    assert!(started(&fx).is_none());
    assert!(h
        .r
        .decisions()
        .iter()
        .any(|d| d.decision == Decision::Unavailable(Why::StalePolicy)));
    assert_eq!(
        h.r.dispositions(&TriggerId::new("t")).last(),
        Some(&Disposition::Filtered(Refusal::Ignored))
    );
    let stale = h.go(Input::EvalResult {
        eval,
        verdicts: vec![(TriggerId::new("t"), accept(&[]))],
    });
    assert!(has(&stale, |e| matches!(
        e,
        Effect::Rejected {
            why: Refusal::StaleEval,
            ..
        }
    )));
    assert_eq!(h.r.tickets().count(), 0);
}

#[test]
fn an_eval_that_never_reaches_the_main_session_never_expires() {
    let mut h = H::new(policy());
    let fx = h.ingest(person("t", "alice", true, "th"));
    assert!(started(&fx).is_some());
    let fx = h.at(10 * 86_400_000, Input::Tick);
    assert!(!has(&fx, |e| matches!(e, Effect::EvalExpired { .. })));
    assert!(h.r.decisions().is_empty());
}

#[test]
fn two_expiries_retry_and_the_third_records_unavailable() {
    let mut h = H::new(policy());
    let fx = h.ingest(person("t", "alice", true, "th"));
    let mut eval = started(&fx).unwrap();
    for round in 1..=3u32 {
        h.go(Input::EvalDelivered { eval });
        let fx = h.at(120_000, Input::Tick);
        if round < 3 {
            assert!(has(&fx, |e| matches!(
                e,
                Effect::EvalExpired { retry: true, .. }
            )));
            eval = started(&fx).expect("a fresh eval for the same batch");
            assert_eq!(batch(&fx)[0].trigger, TriggerId::new("t"));
        } else {
            assert!(!has(&fx, |e| matches!(e, Effect::StartEval { .. })));
        }
    }
    assert_eq!(last_decision(&h), Decision::Unavailable(Why::Timeout));
    assert_eq!(h.r.eval_health(), 1);
    assert!(matches!(
        h.r.attention()[0],
        AttentionItem::EvalUnavailable { .. }
    ));
}

#[test]
fn the_timeout_is_clamped_to_its_documented_range() {
    for (asked, effective) in [(1, 30_000), (120_000, 120_000), (9_999_999, 600_000)] {
        let cfg = RouterConfig {
            eval_timeout_ms: asked,
            ..RouterConfig::default()
        };
        assert_eq!(
            Router::new(cfg, policy()).config().eval_timeout_ms,
            effective
        );
    }
    for (asked, effective) in [(0, 1), (3, 3), (99, 16)] {
        let cfg = RouterConfig {
            max_workers: asked,
            ..RouterConfig::default()
        };
        assert_eq!(Router::new(cfg, policy()).config().max_workers, effective);
    }
}

#[test]
fn a_successful_result_resets_the_expiry_count() {
    let mut h = H::new(policy());
    let mut eval = started(&h.ingest(person("a", "alice", true, "th"))).unwrap();
    for _ in 0..2 {
        h.go(Input::EvalDelivered { eval });
        eval = started(&h.at(120_000, Input::Tick)).unwrap();
    }
    h.go(Input::EvalResult {
        eval,
        verdicts: vec![],
    });
    let mut eval = started(&h.ingest(person("b", "alice", true, "th"))).unwrap();
    for _ in 0..2 {
        h.go(Input::EvalDelivered { eval });
        eval = started(&h.at(120_000, Input::Tick)).unwrap();
    }
    assert!(!h
        .r
        .decisions()
        .iter()
        .any(|d| matches!(d.decision, Decision::Unavailable(_))));
}

#[test]
fn an_assessment_failure_records_unavailable_and_a_later_trigger_starts_a_new_eval() {
    let mut h = H::new(policy());
    let eval = started(&h.ingest(person("a", "alice", true, "th"))).unwrap();
    h.go(Input::EvalFailed { eval });
    assert_eq!(
        last_decision(&h),
        Decision::Unavailable(Why::AssessmentFailed)
    );
    assert!(started(&h.ingest(person("b", "alice", true, "th"))).is_some());
}

#[test]
fn a_policy_change_discards_the_running_eval_and_queues_the_batch_under_the_new_version() {
    let mut h = H::new(policy());
    let eval = started(&h.ingest(person("a", "alice", true, "th"))).unwrap();
    let mut p = policy();
    p.rules.get_mut("deps").unwrap().text = "changed".into();
    let fx = h.go(Input::SetPolicy(p));
    let again = started(&fx).expect("requeued under the new policy");
    assert_ne!(again, eval);
    assert!(fx
        .iter()
        .any(|e| matches!(e, Effect::StartEval { version: 2, .. })));
    assert_eq!(h.r.version(), 2);
    assert_eq!(
        h.r.eval_health(),
        0,
        "a stale-policy record is not an outage"
    );
    assert!(h.r.attention().is_empty());
}

#[test]
fn an_identical_policy_does_not_bump_the_version() {
    let mut h = H::new(policy());
    h.go(Input::SetPolicy(policy()));
    assert_eq!(h.r.version(), 1);
}

#[test]
fn a_stop_by_a_person_withdraws_the_mandate_and_a_bot_cannot_stop() {
    let mut h = H::new(with_mode(AcceptMode::Auto));
    let id = h.open("t1", "issue-1");
    let eval = started(&h.ingest(person("t2", "alice", true, "elsewhere"))).unwrap();
    h.go(Input::EvalResult {
        eval,
        verdicts: vec![(TriggerId::new("t2"), Verdict::Stop { ticket: id })],
    });
    assert_eq!(last_decision(&h), Decision::Stop(id));
    assert_eq!(h.r.ticket(id).unwrap().status, Status::Ended(End::Stopped));
    let id2 = h.open("t3", "issue-3");
    let eval = started(&h.ingest(trigger(
        "b",
        Some(("dependabot[bot]", true)),
        true,
        "x",
        "issues",
    )))
    .unwrap();
    h.go(Input::EvalResult {
        eval,
        verdicts: vec![(TriggerId::new("b"), Verdict::Stop { ticket: id2 })],
    });
    assert_eq!(last_decision(&h), Decision::Refused(Refusal::BotCannotStop));
    assert_eq!(h.r.ticket(id2).unwrap().status, Status::Open);
}

#[test]
fn an_unlisted_persons_stop_on_an_owned_followup_needs_a_mode_that_admits_inferred() {
    for (mode, stopped) in [
        (AcceptMode::Off, false),
        (AcceptMode::Observe, true),
        (AcceptMode::Auto, true),
    ] {
        let mut h = H::new(with_mode(mode));
        let id = h.open("t1", "issue-1");
        let fx = h.ingest(person("s", "mallory", false, "issue-1"));
        assert!(started(&fx).is_none(), "an owned follow-up takes no eval");
        assert!(has(&fx, |e| matches!(e, Effect::FollowUpNotice { .. })));
        h.go(Input::StopOwned {
            trigger: TriggerId::new("s"),
            ticket: id,
        });
        let want = if stopped {
            Decision::Stop(id)
        } else {
            Decision::Refused(Refusal::ModeOff)
        };
        assert_eq!(last_decision(&h), want);
        assert_eq!(h.r.ticket(id).unwrap().status == Status::Open, !stopped);
    }
}

#[test]
fn a_stopped_tickets_stop_message_is_not_sent_to_eval_again() {
    let mut h = H::new(policy());
    let id = h.open("t1", "issue-1");
    h.ingest(person("s", "alice", false, "issue-1"));
    h.go(Input::StopOwned {
        trigger: TriggerId::new("s"),
        ticket: id,
    });
    assert_eq!(h.r.ticket(id).unwrap().status, Status::Ended(End::Stopped));
    assert!(!h
        .r
        .dispositions(&TriggerId::new("s"))
        .iter()
        .any(|d| matches!(d, Disposition::Evaluated(_))));
}

#[test]
fn a_manual_stop_ends_the_ticket_and_is_recorded() {
    let mut h = H::new(policy());
    let id = h.open("t1", "issue-1");
    h.go(Input::ManualStop { ticket: id });
    assert_eq!(h.r.ticket(id).unwrap().status, Status::Ended(End::Stopped));
    assert_eq!(last_decision(&h), Decision::ManualStop(id));
    assert_eq!(h.r.slots_held(), (0, 0));
}

#[test]
fn observe_mode_builds_a_would_accept_ledger_per_rule() {
    let mut h = H::new(policy());
    for i in 0..3 {
        let id = format!("b{i}");
        let fx = h.ingest(trigger(
            &id,
            Some(("dependabot[bot]", true)),
            false,
            &format!("pr-{i}"),
            "all",
        ));
        h.go(Input::EvalResult {
            eval: started(&fx).unwrap(),
            verdicts: vec![(TriggerId::new(&id), accept(&["deps"]))],
        });
    }
    let rep = h.r.observe_report();
    assert_eq!(rep.would_accept, 3);
    assert_eq!(rep.by_rule["deps"], 3);
    assert_eq!(rep.accepted, 0);
}

#[test]
fn candidates_the_main_session_leaves_out_are_recorded_as_ignored() {
    let mut h = H::new(policy());
    let eval = started(&h.ingest(person("a", "alice", true, "th"))).unwrap();
    h.go(Input::EvalResult {
        eval,
        verdicts: vec![],
    });
    assert_eq!(last_decision(&h), Decision::Ignore { from_card: false });
}
