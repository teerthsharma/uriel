mod common;

use common::*;
use uriel_dispatch::*;

fn open_n(h: &mut H, n: usize) -> Vec<TicketId> {
    (0..n)
        .map(|i| h.open(&format!("o{i}"), &format!("issue-{i}")))
        .collect()
}

#[test]
fn three_workers_run_and_the_rest_queue_in_arrival_order() {
    let mut h = H::new(policy());
    let ids = open_n(&mut h, 5);
    assert_eq!(h.r.slots_held(), (3, 0));
    assert_eq!(h.r.stage(ids[0]), Some(Stage::Queued));
    assert!(ids[..3]
        .iter()
        .all(|i| h.r.ticket(*i).unwrap().holds_slot()));
    assert!(ids[3..]
        .iter()
        .all(|i| !h.r.ticket(*i).unwrap().holds_slot()));
    h.go(Input::PrepareDone { ticket: ids[0] });
    assert_eq!(h.r.stage(ids[0]), Some(Stage::Working));
    h.go(Input::TurnEnded {
        ticket: ids[0],
        background: false,
    });
    assert_eq!(h.r.stage(ids[0]), Some(Stage::Idle));
    assert!(h.r.ticket(ids[3]).unwrap().holds_slot());
    assert!(!h.r.ticket(ids[4]).unwrap().holds_slot());
}

#[test]
fn a_handoff_to_a_person_frees_the_slot_at_once() {
    let mut h = H::new(policy());
    let ids = open_n(&mut h, 4);
    h.run_to_turn(ids[0]);
    let fx = h.go(Input::Report {
        ticket: ids[0],
        report: Report::Ready,
        attention: None,
    });
    assert!(has(&fx, |e| matches!(e, Effect::HandOff { .. })));
    assert_eq!(h.r.stage(ids[0]), Some(Stage::PersonReview));
    assert!(h.r.ticket(ids[3]).unwrap().holds_slot());
    assert_eq!(h.r.slots_held(), (3, 0));
}

#[test]
fn a_ready_report_keeps_the_slot_when_another_input_arrived_since() {
    let mut h = H::new(policy());
    let id = h.open("o", "issue-1");
    h.run_to_turn(id);
    h.ingest(bot("ci", "ci[bot]", false, "issue-1"));
    h.go(Input::Report {
        ticket: id,
        report: Report::Ready,
        attention: None,
    });
    assert!(h.r.ticket(id).unwrap().holds_slot());
    assert_eq!(h.r.stage(id), Some(Stage::Working));
}

#[test]
fn blocked_and_failed_always_hand_off_even_with_fresh_input() {
    let mut h = H::new(policy());
    let id = h.open("o", "issue-1");
    h.run_to_turn(id);
    h.ingest(bot("ci", "ci[bot]", false, "issue-1"));
    h.go(Input::Report {
        ticket: id,
        report: Report::Blocked,
        attention: None,
    });
    assert!(!h.r.ticket(id).unwrap().holds_slot());
    assert_eq!(h.r.stage(id), Some(Stage::PersonBlocked));
}

#[test]
fn one_extra_slot_is_kept_for_a_persons_message_and_only_one() {
    let mut h = H::new(policy());
    let ids = open_n(&mut h, 3);
    for id in &ids {
        h.run_to_turn(*id);
        h.go(Input::Report {
            ticket: *id,
            report: Report::Waiting,
            attention: None,
        });
    }
    assert_eq!(h.r.slots_held(), (0, 0));
    let busy = open_n_from(&mut h, 3, 3);
    assert_eq!(h.r.slots_held(), (3, 0));
    h.ingest(person("p1", "carol", false, "issue-0"));
    h.ingest(person("p2", "carol", false, "issue-1"));
    assert_eq!(
        h.r.slots_held(),
        (3, 1),
        "a person's message reaches a ticket although every slot is busy"
    );
    assert!(h.r.ticket(ids[0]).unwrap().holds_slot());
    assert!(
        !h.r.ticket(ids[1]).unwrap().holds_slot(),
        "the reserve holds a single ticket"
    );
    let _ = busy;
}

fn open_n_from(h: &mut H, from: usize, n: usize) -> Vec<TicketId> {
    (from..from + n)
        .map(|i| h.open(&format!("o{i}"), &format!("issue-{i}")))
        .collect()
}

#[test]
fn a_bot_message_does_not_take_the_reserved_slot() {
    let mut h = H::new(policy());
    let ids = open_n(&mut h, 1);
    h.run_to_turn(ids[0]);
    h.go(Input::TurnEnded {
        ticket: ids[0],
        background: false,
    });
    open_n_from(&mut h, 1, 3);
    assert_eq!(h.r.slots_held(), (3, 0));
    h.ingest(bot("ci", "ci[bot]", false, "issue-0"));
    assert_eq!(h.r.slots_held(), (3, 0));
    assert!(!h.r.ticket(ids[0]).unwrap().holds_slot());
}

#[test]
fn person_messages_are_scheduled_before_older_queued_work() {
    let mut h = H::new(policy());
    let ids = open_n(&mut h, 3);
    for id in &ids {
        h.run_to_turn(*id);
        h.go(Input::TurnEnded {
            ticket: *id,
            background: false,
        });
    }
    let later = open_n_from(&mut h, 3, 3);
    let waiting = h.go(Input::Tick);
    let _ = waiting;
    let extra = h.open("zz", "issue-zz");
    assert!(!h.r.ticket(extra).unwrap().holds_slot());
    h.ingest(person("p", "carol", false, "issue-1"));
    assert!(h.r.ticket(ids[1]).unwrap().holds_slot());
    assert!(later.iter().all(|i| h.r.ticket(*i).unwrap().holds_slot()));
}

#[test]
fn memory_pressure_shrinks_capacity_without_preempting_and_recovery_restores_it() {
    let mut h = H::new(policy());
    let ids = open_n(&mut h, 3);
    assert_eq!(h.r.slots_held(), (3, 0));
    h.go(Input::Pressure {
        free_mb: 1_000,
        total_mb: 16_000,
    });
    assert_eq!(h.r.capacity(), 0);
    assert_eq!(h.r.slots_held(), (3, 0), "running work is not killed");
    let more = h.open("late", "issue-late");
    assert!(!h.r.ticket(more).unwrap().holds_slot());
    h.run_to_turn(ids[0]);
    h.go(Input::TurnEnded {
        ticket: ids[0],
        background: false,
    });
    assert!(
        !h.r.ticket(more).unwrap().holds_slot(),
        "no free capacity under pressure"
    );
    h.go(Input::Pressure {
        free_mb: 12_000,
        total_mb: 16_000,
    });
    assert!(h.r.ticket(more).unwrap().holds_slot());
}

#[test]
fn capacity_never_exceeds_the_worker_limit_however_much_ram_is_free() {
    let mut h = H::with(
        RouterConfig {
            max_workers: 2,
            ..RouterConfig::default()
        },
        policy(),
    );
    h.go(Input::Pressure {
        free_mb: 1_000_000,
        total_mb: 1_000_000,
    });
    assert_eq!(h.r.capacity(), 2);
}

#[test]
fn ram_slot_math_matches_the_house_governor_reserve() {
    use uriel_house::{governor, HouseConfig, JobClass};
    let mut hc = HouseConfig::at(".");
    hc.heavy_cap = 1_000_000;
    let cfg = RouterConfig {
        worker_cost_mb: 800,
        ..RouterConfig::default()
    };
    for total in [2_048, 8_192, 16_384, 65_536] {
        assert_eq!(cfg.reserve_mb(total), governor::reserve_mb(total, &hc));
        for free in [0, 1_000, 2_048, 3_000, 9_000, 40_000] {
            let want = governor::slots(JobClass::Build, free, total, 800, &hc);
            assert_eq!(
                cfg.ram_slots(free, total),
                want,
                "free {free} total {total}"
            );
        }
    }
}

#[test]
fn a_turn_that_ends_with_background_jobs_keeps_its_slot_until_they_finish() {
    let mut h = H::new(policy());
    let id = h.open("o", "issue-1");
    h.run_to_turn(id);
    h.go(Input::TurnEnded {
        ticket: id,
        background: true,
    });
    assert!(h.r.ticket(id).unwrap().holds_slot());
    assert_eq!(h.r.stage(id), Some(Stage::Working));
    let fx = h.go(Input::BackgroundDone { ticket: id });
    assert!(has(&fx, |e| matches!(e, Effect::StartTurn { .. })));
    h.go(Input::TurnEnded {
        ticket: id,
        background: false,
    });
    assert!(!h.r.ticket(id).unwrap().holds_slot());
    assert_eq!(h.r.stage(id), Some(Stage::Idle));
}

#[test]
fn background_work_is_killed_after_thirty_minutes_and_the_ticket_reports_waiting() {
    let mut h = H::new(policy());
    let id = h.open("o", "issue-1");
    h.run_to_turn(id);
    h.go(Input::TurnEnded {
        ticket: id,
        background: true,
    });
    let fx = h.at(1_799_000, Input::Tick);
    assert!(!has(&fx, |e| matches!(e, Effect::KillJobs { .. })));
    let fx = h.at(1_001, Input::Tick);
    assert!(has(&fx, |e| matches!(e, Effect::KillJobs { .. })));
    assert!(!h.r.ticket(id).unwrap().holds_slot());
    assert_eq!(h.r.stage(id), Some(Stage::Idle));
}

#[test]
fn a_prepare_command_that_runs_past_ten_minutes_is_aborted_and_handed_to_a_person() {
    let mut h = H::new(policy());
    let id = h.open("o", "issue-1");
    let fx = h.at(599_000, Input::Tick);
    assert!(!has(&fx, |e| matches!(e, Effect::Abort { .. })));
    let fx = h.at(2_000, Input::Tick);
    assert!(has(&fx, |e| matches!(e, Effect::Abort { .. })));
    assert_eq!(h.r.stage(id), Some(Stage::PersonBlocked));
    assert!(!h.r.ticket(id).unwrap().holds_slot());
}

#[test]
fn a_failed_prepare_hands_off_and_a_person_message_releases_it() {
    let mut h = H::new(policy());
    let id = h.open("o", "issue-1");
    h.go(Input::PrepareFailed { ticket: id });
    assert_eq!(h.r.stage(id), Some(Stage::PersonBlocked));
    h.ingest(person("p", "carol", false, "issue-1"));
    assert_ne!(h.r.stage(id), Some(Stage::PersonBlocked));
    assert!(h.r.ticket(id).unwrap().holds_slot());
}

#[test]
fn undelivered_input_backs_off_two_seconds_doubling_to_sixty() {
    let mut h = H::new(policy());
    let id = h.open("o", "issue-1");
    let mut secs = Vec::new();
    for _ in 0..8 {
        h.t += 1;
        let failed_at = h.t;
        h.r.apply(failed_at, Input::DeliveryFailed { ticket: id });
        assert_eq!(h.r.stage(id), Some(Stage::PersonDelivery));
        let next = h.r.ticket(id).unwrap().input_fail.unwrap().next_at;
        secs.push((next - failed_at) / 1000);
        let early = h.r.apply(next - 1, Input::Tick);
        assert!(!has(&early, |e| matches!(e, Effect::Redeliver { .. })));
        h.t = next;
        let fx = h.r.apply(next, Input::Tick);
        assert!(has(&fx, |e| matches!(e, Effect::Redeliver { .. })));
    }
    assert_eq!(secs, [2, 4, 8, 16, 32, 60, 60, 60]);
    h.go(Input::DeliveryOk { ticket: id });
    assert_ne!(h.r.stage(id), Some(Stage::PersonDelivery));
}

#[test]
fn the_stage_table_is_read_top_to_bottom_first_match_wins() {
    let mut h = H::new(policy());
    let id = h.open("o", "issue-1");
    assert_eq!(h.r.stage(id), Some(Stage::Queued));
    h.run_to_turn(id);
    assert_eq!(h.r.stage(id), Some(Stage::Working));
    h.go(Input::Report {
        ticket: id,
        report: Report::Ready,
        attention: Some("merge now?".into()),
    });
    assert_eq!(h.r.stage(id), Some(Stage::PersonAnswer));
    h.go(Input::DeliveryFailed { ticket: id });
    assert_eq!(h.r.stage(id), Some(Stage::PersonDelivery));
    h.go(Input::DeliveryOk { ticket: id });
    assert_eq!(h.r.stage(id), Some(Stage::PersonAnswer));
    h.ingest(person("ans", "carol", false, "issue-1"));
    assert_ne!(h.r.stage(id), Some(Stage::PersonAnswer));
}

#[test]
fn a_persons_message_releases_ready_blocked_and_failed_tickets_to_the_executor() {
    for report in [Report::Ready, Report::Blocked, Report::Failed] {
        let mut h = H::new(policy());
        let id = h.open("o", "issue-1");
        h.run_to_turn(id);
        h.go(Input::Report {
            ticket: id,
            report,
            attention: None,
        });
        assert!(h.r.stage(id).unwrap().needs_person());
        h.ingest(person("p", "carol", false, "issue-1"));
        assert!(!h.r.stage(id).unwrap().needs_person());
        assert!(h.r.ticket(id).unwrap().holds_slot());
    }
}

#[test]
fn a_bot_message_does_not_release_a_ticket_that_waits_for_review() {
    let mut h = H::new(policy());
    let id = h.open("o", "issue-1");
    h.run_to_turn(id);
    h.go(Input::Report {
        ticket: id,
        report: Report::Ready,
        attention: None,
    });
    h.ingest(bot("ci", "ci[bot]", false, "issue-1"));
    assert_eq!(h.r.stage(id), Some(Stage::PersonReview));
    assert!(!h.r.ticket(id).unwrap().holds_slot());
}

#[test]
fn an_idle_ticket_wakes_for_a_bot_message_on_its_thread() {
    let mut h = H::new(policy());
    let id = h.open("o", "issue-1");
    h.run_to_turn(id);
    h.go(Input::TurnEnded {
        ticket: id,
        background: false,
    });
    assert_eq!(h.r.stage(id), Some(Stage::Idle));
    let fx = h.ingest(bot("ci", "ci[bot]", false, "issue-1"));
    assert!(has(&fx, |e| matches!(e, Effect::StartTurn { .. })));
    assert_eq!(h.r.stage(id), Some(Stage::Working));
}

#[test]
fn input_that_arrives_during_a_turn_runs_another_turn_instead_of_idling() {
    let mut h = H::new(policy());
    let id = h.open("o", "issue-1");
    h.run_to_turn(id);
    h.ingest(bot("ci", "ci[bot]", false, "issue-1"));
    let fx = h.go(Input::TurnEnded {
        ticket: id,
        background: false,
    });
    assert!(has(&fx, |e| matches!(e, Effect::StartTurn { .. })));
    assert!(h.r.ticket(id).unwrap().holds_slot());
}

#[test]
fn a_dropped_report_ends_the_ticket_and_frees_the_slot() {
    let mut h = H::new(policy());
    let id = h.open("o", "issue-1");
    h.run_to_turn(id);
    h.go(Input::Report {
        ticket: id,
        report: Report::Dropped,
        attention: None,
    });
    assert_eq!(h.r.ticket(id).unwrap().status, Status::Ended(End::Done));
    assert_eq!(h.r.slots_held(), (0, 0));
    assert_eq!(h.r.stage(id), None);
}

#[test]
fn out_of_order_executor_calls_are_rejected_not_applied() {
    let mut h = H::new(policy());
    let id = h.open("o", "issue-1");
    let fx = h.go(Input::TurnEnded {
        ticket: id,
        background: false,
    });
    assert!(has(&fx, |e| matches!(
        e,
        Effect::Rejected {
            why: Refusal::NoSlot,
            ..
        }
    )));
    let fx = h.go(Input::PrepareDone {
        ticket: TicketId(99),
    });
    assert!(has(&fx, |e| matches!(
        e,
        Effect::Rejected {
            why: Refusal::NoSuchTicket,
            ..
        }
    )));
    h.run_to_turn(id);
    let fx = h.go(Input::PrepareDone { ticket: id });
    assert!(has(&fx, |e| matches!(e, Effect::Rejected { .. })));
}

#[test]
fn attention_lists_what_waits_for_a_person_most_urgent_first() {
    let mut h = H::new(policy());
    let ids = open_n(&mut h, 3);
    for id in &ids {
        h.run_to_turn(*id);
    }
    h.go(Input::Report {
        ticket: ids[0],
        report: Report::Failed,
        attention: None,
    });
    h.go(Input::Report {
        ticket: ids[1],
        report: Report::Ready,
        attention: None,
    });
    h.go(Input::Report {
        ticket: ids[2],
        report: Report::Progress,
        attention: Some("which branch?".into()),
    });
    let order: Vec<Stage> =
        h.r.attention()
            .into_iter()
            .filter_map(|a| match a {
                AttentionItem::Ticket { stage, .. } => Some(stage),
                _ => None,
            })
            .collect();
    assert_eq!(
        order,
        [
            Stage::PersonAnswer,
            Stage::PersonReview,
            Stage::PersonBlocked
        ]
    );
}

#[test]
fn healthy_tickets_take_no_space_in_attention() {
    let mut h = H::new(policy());
    let ids = open_n(&mut h, 2);
    h.run_to_turn(ids[0]);
    h.go(Input::TurnEnded {
        ticket: ids[0],
        background: false,
    });
    assert!(h.r.attention().is_empty());
}
