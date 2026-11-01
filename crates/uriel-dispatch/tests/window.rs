mod common;

use common::*;
use uriel_dispatch::*;

#[test]
fn nothing_new_starts_once_the_window_has_closed() {
    let mut h = H::new(policy());
    h.go(Input::Window {
        closes: Some(h.t + 10_000),
    });
    let a = h.open("a", "issue-1");
    assert!(h.r.ticket(a).unwrap().holds_slot());
    let fx = h.at(20_000, Input::Tick);
    assert!(has(&fx, |e| matches!(e, Effect::WindowClosed)));
    assert!(has(
        &fx,
        |e| matches!(e, Effect::WindDown { ticket } if *ticket == a)
    ));
    let b = h.open("b", "issue-2");
    assert!(!h.r.ticket(b).unwrap().holds_slot());
    assert_eq!(h.r.stage(b), Some(Stage::Queued));
}

#[test]
fn the_close_is_announced_once() {
    let mut h = H::new(policy());
    h.go(Input::Window {
        closes: Some(h.t + 1_000),
    });
    let first = h.at(2_000, Input::Tick);
    let second = h.at(2_000, Input::Tick);
    assert!(has(&first, |e| matches!(e, Effect::WindowClosed)));
    assert!(!has(&second, |e| matches!(e, Effect::WindowClosed)));
}

#[test]
fn a_running_ticket_may_finish_and_report_after_the_close_but_frees_no_new_work() {
    let mut h = H::new(policy());
    h.go(Input::Window {
        closes: Some(h.t + 10_000),
    });
    let a = h.open("a", "issue-1");
    h.run_to_turn(a);
    h.at(20_000, Input::Tick);
    let b = h.open("b", "issue-2");
    h.go(Input::Report {
        ticket: a,
        report: Report::Ready,
        attention: None,
    });
    assert_eq!(h.r.stage(a), Some(Stage::PersonReview));
    assert!(!h.r.ticket(b).unwrap().holds_slot());
}

#[test]
fn extending_the_window_releases_the_queue_and_rearms_the_announcement() {
    let mut h = H::new(policy());
    h.go(Input::Window {
        closes: Some(h.t + 1_000),
    });
    h.at(2_000, Input::Tick);
    let b = h.open("b", "issue-2");
    assert!(!h.r.ticket(b).unwrap().holds_slot());
    h.go(Input::Window {
        closes: Some(h.t + 50_000),
    });
    assert!(h.r.ticket(b).unwrap().holds_slot());
    assert!(h.r.window_open());
    let fx = h.at(60_000, Input::Tick);
    assert!(has(&fx, |e| matches!(e, Effect::WindowClosed)));
}

#[test]
fn without_a_window_work_is_never_gated_by_time() {
    let mut h = H::new(policy());
    let a = h.open("a", "issue-1");
    h.run_to_turn(a);
    h.at(30 * 86_400_000, Input::Tick);
    assert!(h.r.window_open());
    assert!(h.r.ticket(a).unwrap().holds_slot());
}
