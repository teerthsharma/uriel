mod common;

use common::*;
use uriel_dispatch::*;

fn with_pr(h: &mut H, id: TicketId) {
    h.go(Input::Follow {
        ticket: id,
        thread: ThreadId::new("pr-5"),
        kind: SubKind::Origin,
    });
    h.run_to_turn(id);
}

#[test]
fn a_persons_message_on_a_ticket_thread_goes_to_its_executor_with_no_eval() {
    let mut h = H::new(policy());
    let id = h.open("o", "issue-1");
    let fx = h.ingest(person("m", "carol", false, "issue-1"));
    assert!(started(&fx).is_none());
    assert!(has(
        &fx,
        |e| matches!(e, Effect::Deliver { ticket, .. } if *ticket == id)
    ));
    assert!(has(&fx, |e| matches!(e, Effect::FollowUpNotice { .. })));
    assert!(h.r.ticket(id).unwrap().owned.contains(&TriggerId::new("m")));
}

#[test]
fn bot_and_ci_messages_reach_the_ticket_and_still_take_the_eval_path_independently() {
    let mut h = H::new(with_mode(AcceptMode::Auto));
    let id = h.open("o", "issue-1");
    let fx = h.ingest(trigger(
        "b",
        Some(("dependabot[bot]", true)),
        true,
        "issue-1",
        "issues",
    ));
    assert!(has(
        &fx,
        |e| matches!(e, Effect::Deliver { ticket, .. } if *ticket == id)
    ));
    assert_eq!(batch(&fx)[0].strength, Strength::Inferred);
    let ci = h.ingest(trigger("ci", None, false, "issue-1", "issues"));
    assert!(has(&ci, |e| matches!(e, Effect::Deliver { .. })));
    assert!(
        started(&ci).is_none(),
        "CI is ignored by the default last cell and the scope is mention-only"
    );
}

#[test]
fn a_context_subscription_delivers_but_grants_no_ownership_and_never_resumes() {
    let mut h = H::new(policy());
    let id = h.open("o", "issue-1");
    h.go(Input::Follow {
        ticket: id,
        thread: ThreadId::new("issue-9"),
        kind: SubKind::Context,
    });
    let fx = h.ingest(person("m", "alice", true, "issue-9"));
    assert!(has(&fx, |e| matches!(e, Effect::Deliver { .. })));
    assert!(
        started(&fx).is_some(),
        "an addressed request there is judged by the gate and goes to eval"
    );
    assert!(h.r.ticket(id).unwrap().owned.is_empty());
    assert!(!h.r.ticket(id).unwrap().person_since_report);
}

#[test]
fn a_context_follow_cannot_widen_an_authority_subscription() {
    let mut h = H::new(policy());
    let id = h.open("o", "issue-1");
    h.go(Input::Follow {
        ticket: id,
        thread: ThreadId::new("issue-1"),
        kind: SubKind::Context,
    });
    assert_eq!(
        h.r.ticket(id).unwrap().subs[&ThreadId::new("issue-1")],
        SubKind::Origin
    );
}

#[test]
fn an_ended_ticket_resumes_on_a_persons_message_without_eval_or_a_new_decision() {
    let mut h = H::new(policy());
    let id = h.open("o", "issue-1");
    with_pr(&mut h, id);
    h.go(Input::ThreadClosed {
        thread: ThreadId::new("pr-5"),
    });
    assert_eq!(h.r.ticket(id).unwrap().status, Status::Ended(End::Closed));
    assert_eq!(h.r.slots_held(), (0, 0));
    let decisions = h.r.decisions().len();
    let fx = h.ingest(person("late", "carol", false, "pr-5"));
    assert!(has(&fx, |e| matches!(e, Effect::Resumed { .. })));
    assert!(started(&fx).is_none());
    assert_eq!(h.r.decisions().len(), decisions);
    assert_eq!(h.r.ticket(id).unwrap().status, Status::Open);
    assert_eq!(h.r.ticket(id).unwrap().assignments.len(), 2);
    assert!(h.r.ticket(id).unwrap().holds_slot());
}

#[test]
fn bots_and_ci_do_not_wake_an_ended_ticket() {
    let mut h = H::new(policy());
    let id = h.open("o", "issue-1");
    with_pr(&mut h, id);
    h.go(Input::ThreadClosed {
        thread: ThreadId::new("pr-5"),
    });
    let fx = h.ingest(bot("ci", "ci[bot]", false, "pr-5"));
    assert!(!has(&fx, |e| matches!(
        e,
        Effect::Resumed { .. } | Effect::Deliver { .. }
    )));
    assert_eq!(h.r.ticket(id).unwrap().status, Status::Ended(End::Closed));
}

#[test]
fn a_ticket_stopped_by_a_person_receives_nothing_and_its_threads_take_the_eval_path() {
    let mut h = H::new(policy());
    let id = h.open("o", "issue-1");
    h.go(Input::ManualStop { ticket: id });
    let fx = h.ingest(person("m", "alice", true, "issue-1"));
    assert!(!has(&fx, |e| matches!(
        e,
        Effect::Deliver { .. } | Effect::Resumed { .. }
    )));
    assert!(started(&fx).is_some());
    assert_eq!(h.r.ticket(id).unwrap().status, Status::Ended(End::Stopped));
}

#[test]
fn resume_is_revalidated_and_a_failure_revokes_the_ticket_and_sends_the_message_to_eval() {
    let mut h = H::new(policy());
    let id = h.open("o", "issue-1");
    with_pr(&mut h, id);
    h.go(Input::ThreadClosed {
        thread: ThreadId::new("pr-5"),
    });
    let mut p = policy();
    p.sources.get_mut("gh").unwrap().allowed.clear();
    p.sources
        .get_mut("gh")
        .unwrap()
        .allowed
        .insert("bob".into());
    h.go(Input::SetPolicy(p));
    let fx = h.ingest(trigger(
        "late",
        Some(("bob", false)),
        true,
        "pr-5",
        "issues",
    ));
    assert_eq!(
        h.r.ticket(id).unwrap().status,
        Status::Ended(End::Revoked(Revoke::SenderRemoved))
    );
    assert!(
        started(&fx).is_some(),
        "the message reached no ticket, so eval judges it"
    );
    assert!(!has(&fx, |e| matches!(e, Effect::Resumed { .. })));
    let eval = started(&fx).unwrap();
    let fx = h.go(Input::EvalResult {
        eval,
        verdicts: vec![(TriggerId::new("late"), Verdict::Attach { ticket: id })],
    });
    assert!(has(
        &fx,
        |e| matches!(e, Effect::Decided(r) if r.decision == Decision::Refused(Refusal::TicketRevoked))
    ));
}

#[test]
fn a_policy_change_notifies_only_the_tickets_it_affects_and_revokes_none() {
    let mut h = H::new(with_mode(AcceptMode::Auto));
    let fx = h.ingest(trigger(
        "b",
        Some(("dependabot[bot]", true)),
        true,
        "pr-1",
        "issues",
    ));
    h.go(Input::EvalResult {
        eval: started(&fx).unwrap(),
        verdicts: vec![(TriggerId::new("b"), accept(&["deps"]))],
    });
    let bot_ticket = TicketId(1);
    let listed = h.open("o", "issue-2");
    let mut p = with_mode(AcceptMode::Auto);
    p.rules.get_mut("deps").unwrap().text = "bump deps weekly".into();
    let fx = h.go(Input::SetPolicy(p));
    let notified: Vec<TicketId> = fx
        .iter()
        .filter_map(|e| match e {
            Effect::PolicyChanged { ticket, .. } => Some(*ticket),
            _ => None,
        })
        .collect();
    assert_eq!(notified, [bot_ticket]);
    assert_eq!(h.r.ticket(bot_ticket).unwrap().status, Status::Open);
    assert_eq!(h.r.ticket(listed).unwrap().status, Status::Open);
    let mut p = with_mode(AcceptMode::Observe);
    p.rules.get_mut("deps").unwrap().text = "bump deps weekly".into();
    let fx = h.go(Input::SetPolicy(p));
    assert!(has(
        &fx,
        |e| matches!(e, Effect::PolicyChanged { changes, .. } if changes.contains(&Change::Revoked(Revoke::ProactiveOff)))
    ));
    assert_eq!(h.r.ticket(bot_ticket).unwrap().status, Status::Open);
}

#[test]
fn handing_back_sends_the_message_to_eval_with_the_reason_once() {
    let mut h = H::new(policy());
    let id = h.open("o", "issue-1");
    h.ingest(person("m", "alice", true, "issue-1"));
    let fx = h.go(Input::HandBack {
        ticket: id,
        trigger: TriggerId::new("m"),
        reason: "needs another repo".into(),
    });
    let cands = batch(&fx);
    assert_eq!(cands[0].reasons, ["needs another repo"]);
    assert_eq!(cands[0].strength, Strength::Explicit);
    let again = h.go(Input::HandBack {
        ticket: id,
        trigger: TriggerId::new("m"),
        reason: "x".into(),
    });
    assert!(has(&again, |e| matches!(
        e,
        Effect::Rejected {
            why: Refusal::NotOwnedByTicket,
            ..
        }
    )));
}

#[test]
fn a_message_goes_to_eval_only_after_no_ticket_owns_it() {
    let mut h = H::new(policy());
    let a = h.open("oa", "issue-1");
    let b = h.open("ob", "issue-2");
    h.go(Input::Follow {
        ticket: b,
        thread: ThreadId::new("issue-1"),
        kind: SubKind::Attached,
    });
    let fx = h.ingest(person("m", "alice", true, "issue-1"));
    assert!(started(&fx).is_none());
    let fx = h.go(Input::HandBack {
        ticket: a,
        trigger: TriggerId::new("m"),
        reason: "not mine".into(),
    });
    assert!(started(&fx).is_none(), "ticket b still owns it");
    let fx = h.go(Input::HandBack {
        ticket: b,
        trigger: TriggerId::new("m"),
        reason: "not mine either".into(),
    });
    assert!(started(&fx).is_some());
}

#[test]
fn unread_followups_are_released_to_eval_when_the_ticket_ends_and_acknowledged_ones_are_not() {
    let mut h = H::new(policy());
    let id = h.open("o", "issue-1");
    h.ingest(person("read", "alice", true, "issue-1"));
    h.ingest(person("unread", "alice", true, "issue-1"));
    h.go(Input::Ack {
        ticket: id,
        trigger: TriggerId::new("read"),
    });
    let fx = h.go(Input::Report {
        ticket: id,
        report: Report::Dropped,
        attention: None,
    });
    let ids: Vec<String> = batch(&fx).into_iter().map(|c| c.trigger.0).collect();
    assert_eq!(ids, ["unread"]);
    assert_eq!(batch(&fx)[0].reasons, ["released: ticket ended"]);
}

#[test]
fn an_executor_can_drop_a_context_thread_but_not_the_thread_it_started_on() {
    let mut h = H::new(policy());
    let id = h.open("o", "issue-1");
    h.go(Input::Follow {
        ticket: id,
        thread: ThreadId::new("issue-9"),
        kind: SubKind::Attached,
    });
    h.ingest(person("m", "alice", true, "issue-9"));
    let fx = h.go(Input::Unfollow {
        ticket: id,
        thread: ThreadId::new("issue-9"),
    });
    assert_eq!(batch(&fx)[0].reasons, ["unrelated thread"]);
    let fx = h.go(Input::Unfollow {
        ticket: id,
        thread: ThreadId::new("issue-1"),
    });
    assert!(has(&fx, |e| matches!(
        e,
        Effect::Rejected {
            why: Refusal::OriginPinned,
            ..
        }
    )));
    assert!(h
        .r
        .ticket(id)
        .unwrap()
        .subs
        .contains_key(&ThreadId::new("issue-1")));
}

#[test]
fn a_person_can_drop_every_subscription_on_a_thread_and_ownership_continues() {
    let mut h = H::new(policy());
    let id = h.open("o", "issue-1");
    h.ingest(person("m", "alice", true, "issue-1"));
    h.go(Input::DropThread {
        thread: ThreadId::new("issue-1"),
    });
    assert!(h.r.ticket(id).unwrap().subs.is_empty());
    assert!(h.r.ticket(id).unwrap().owned.contains(&TriggerId::new("m")));
    let fx = h.ingest(person("n", "alice", true, "issue-1"));
    assert!(started(&fx).is_some());
}

#[test]
fn the_main_session_attach_delivers_and_a_stopped_ticket_refuses_attach() {
    let mut h = H::new(policy());
    let id = h.open("o", "issue-1");
    let eval = started(&h.ingest(person("m", "alice", true, "issue-7"))).unwrap();
    let fx = h.go(Input::EvalResult {
        eval,
        verdicts: vec![(TriggerId::new("m"), Verdict::Attach { ticket: id })],
    });
    assert!(has(&fx, |e| matches!(e, Effect::Deliver { .. })));
    assert_eq!(
        h.r.ticket(id).unwrap().subs[&ThreadId::new("issue-7")],
        SubKind::Attached
    );
    h.go(Input::ManualStop { ticket: id });
    let eval = started(&h.ingest(person("n", "alice", true, "issue-8"))).unwrap();
    h.go(Input::EvalResult {
        eval,
        verdicts: vec![(TriggerId::new("n"), Verdict::Attach { ticket: id })],
    });
    assert_eq!(last_decision(&h), Decision::Refused(Refusal::TicketStopped));
}

#[test]
fn a_stop_through_a_ticket_that_does_not_own_the_message_is_refused_and_changes_nothing() {
    let mut h = H::new(policy());
    let a = h.open("oa", "issue-1");
    let b = h.open("ob", "issue-2");
    h.ingest(person("m", "alice", true, "issue-1"));
    h.go(Input::StopOwned {
        trigger: TriggerId::new("m"),
        ticket: b,
    });
    assert_eq!(
        last_decision(&h),
        Decision::Refused(Refusal::NotOwnedByTicket)
    );
    assert_eq!(h.r.ticket(b).unwrap().status, Status::Open);
    assert!(h.r.ticket(a).unwrap().owned.contains(&TriggerId::new("m")));
}

#[test]
fn an_idle_subscribed_thread_is_unpolled_after_fourteen_days_once() {
    let mut h = H::new(policy());
    h.open("o", "issue-1");
    let day = 86_400_000;
    let fx = h.at(13 * day, Input::Tick);
    assert!(!has(&fx, |e| matches!(e, Effect::Unpoll { .. })));
    let fx = h.at(2 * day, Input::Tick);
    assert!(has(&fx, |e| matches!(e, Effect::Unpoll { .. })));
    let fx = h.at(day, Input::Tick);
    assert!(!has(&fx, |e| matches!(e, Effect::Unpoll { .. })));
}

#[test]
fn activity_resets_the_polling_horizon() {
    let mut h = H::new(policy());
    h.open("o", "issue-1");
    let day = 86_400_000;
    h.at(10 * day, Input::Tick);
    h.ingest(person("m", "carol", false, "issue-1"));
    let fx = h.at(10 * day, Input::Tick);
    assert!(!has(&fx, |e| matches!(e, Effect::Unpoll { .. })));
}

#[test]
fn every_ingested_trigger_gets_exactly_one_first_disposition() {
    let mut h = H::new(policy());
    h.open("o", "issue-1");
    for (i, t) in [
        person("a", "alice", true, "x"),
        person("b", "mallory", true, "x"),
        person("c", "carol", false, "issue-1"),
        bot("d", "evil[bot]", true, "x"),
        trigger("e", None, false, "x", "all"),
    ]
    .into_iter()
    .enumerate()
    {
        let _ = i;
        h.ingest(t);
    }
    for id in ["a", "b", "c", "d", "e"] {
        assert!(!h.r.dispositions(&TriggerId::new(id)).is_empty(), "{id}");
    }
}
