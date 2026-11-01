use std::path::PathBuf;

use serde_json::Value;
use uriel_dispatch::afk::*;
use uriel_dispatch::*;

const T0: Ms = 1_700_000_000_000;
const MIN: Ms = 60_000;

fn log(dir: &tempfile::TempDir) -> PathBuf {
    dir.path().join("afk").join("session.jsonl")
}

fn starts(a: &[Action]) -> Vec<u64> {
    a.iter()
        .filter_map(|x| match x {
            Action::Start { ticket, .. } => Some(*ticket),
            _ => None,
        })
        .collect()
}

fn names(v: &Value, key: &str) -> Vec<u64> {
    v[key]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_u64().unwrap())
        .collect()
}

#[test]
fn durations_parse_with_units_and_a_bare_number_means_minutes() {
    for (s, ms) in [
        ("90m", 90 * MIN),
        ("2h", 120 * MIN),
        ("1h30m", 90 * MIN),
        ("45", 45 * MIN),
        ("120s", 2 * MIN),
        (" 3H ", 180 * MIN),
    ] {
        assert_eq!(parse_duration(s).unwrap(), ms, "{s}");
    }
    for s in [
        "",
        "0",
        "30s",
        "25h",
        "h",
        "1x",
        "-5m",
        "1h30",
        "99999999999999999999h",
        "1.5h",
    ] {
        assert!(parse_duration(s).is_err(), "{s} should be refused");
    }
}

#[test]
fn only_the_owner_passes_the_gate_and_nothing_inferred_is_ever_admitted() {
    let p = afk_policy("teerth");
    let t = |who: &str, bot: bool, addressed: bool| Trigger {
        id: TriggerId::new("x"),
        source: SOURCE.into(),
        scope: SCOPE.into(),
        thread: ThreadId::new("afk/1"),
        actor: Some(Actor {
            id: who.into(),
            bot,
        }),
        addressed,
    };
    assert!(gate(&p, &t("teerth", false, true), false).is_ok());
    assert_eq!(
        gate(&p, &t("mallory", false, true), false),
        Err(Refusal::Ignored)
    );
    assert_eq!(
        gate(&p, &t("ci[bot]", true, true), false),
        Err(Refusal::BotNotListed)
    );
    assert_eq!(
        gate(&p, &t("teerth", false, false), false),
        Err(Refusal::NotReceivedAll)
    );
}

#[test]
fn start_refuses_a_second_session_in_the_same_place() {
    let d = tempfile::tempdir().unwrap();
    let a = Afk::start(&log(&d), "me", 2 * 60 * MIN, 3, T0).unwrap();
    drop(a);
    assert!(matches!(
        Afk::start(&log(&d), "me", 60 * MIN, 3, T0),
        Err(AfkError::Active)
    ));
}

#[test]
fn goals_start_up_to_the_worker_limit_and_the_rest_wait_for_a_free_slot() {
    let d = tempfile::tempdir().unwrap();
    let mut a = Afk::start(&log(&d), "me", 120 * MIN, 2, T0).unwrap();
    let (t1, s1) = a.add(T0 + 1, Some("G1.1"), "write parser").unwrap();
    let (t2, s2) = a.add(T0 + 2, Some("G1.2"), "write printer").unwrap();
    let (t3, s3) = a.add(T0 + 3, Some("G1.3"), "wire cli").unwrap();
    assert_eq!(starts(&s1), [t1.0]);
    assert_eq!(starts(&s2), [t2.0]);
    assert!(starts(&s3).is_empty(), "third goal queues");
    match &s1[0] {
        Action::Start { goal, scope, .. } => {
            assert_eq!(goal.as_deref(), Some("G1.1"));
            assert_eq!(scope, "write parser");
        }
        other => panic!("{other:?}"),
    }
    let next = a
        .report(T0 + 10, t1, Kind::Ready, Some("branch afk/parser".into()))
        .unwrap();
    assert_eq!(
        starts(&next),
        [t3.0],
        "the freed slot goes to the queued goal"
    );
}

#[test]
fn a_question_hands_off_frees_the_slot_and_is_kept_for_the_summary() {
    let d = tempfile::tempdir().unwrap();
    let mut a = Afk::start(&log(&d), "me", 120 * MIN, 1, T0).unwrap();
    let (t1, _) = a.add(T0 + 1, None, "refactor store").unwrap();
    let (t2, s2) = a.add(T0 + 2, None, "docs").unwrap();
    assert!(starts(&s2).is_empty());
    let next = a
        .report(
            T0 + 5,
            t1,
            Kind::Ask,
            Some("keep the old on-disk format?".into()),
        )
        .unwrap();
    assert_eq!(starts(&next), [t2.0]);
    let sum = a.summary();
    assert_eq!(names(&sum, "needs_you"), [t1.0]);
    assert_eq!(sum["needs_you"][0]["ask"], "keep the old on-disk format?");
}

#[test]
fn the_window_closing_winds_down_running_work_and_refuses_new_goals() {
    let d = tempfile::tempdir().unwrap();
    let mut a = Afk::start(&log(&d), "me", 60 * MIN, 3, T0).unwrap();
    let (t1, _) = a.add(T0 + 1, None, "long job").unwrap();
    let st = a.status(T0 + 30 * MIN).unwrap();
    assert_eq!(st["window"]["open"], true);
    let st = a.status(T0 + 61 * MIN).unwrap();
    assert_eq!(st["window"]["open"], false);
    assert!(st["actions"].to_string().contains("wind_down"));
    assert!(matches!(
        a.add(T0 + 62 * MIN, None, "late idea"),
        Err(AfkError::WindowClosed)
    ));
    let after = a.report(T0 + 63 * MIN, t1, Kind::Ready, None).unwrap();
    assert!(starts(&after).is_empty());
}

#[test]
fn a_queued_goal_that_never_got_a_slot_is_reported_as_never_started() {
    let d = tempfile::tempdir().unwrap();
    let mut a = Afk::start(&log(&d), "me", 60 * MIN, 1, T0).unwrap();
    let (t1, _) = a.add(T0 + 1, None, "first").unwrap();
    let (t2, _) = a.add(T0 + 2, None, "second").unwrap();
    a.report(T0 + 5 * MIN, t1, Kind::Ready, Some("pr-ready".into()))
        .unwrap();
    let (t3, _) = a.add(T0 + 6 * MIN, None, "third").unwrap();
    let _ = (t2, t3);
    let (sum, archive) = a.end(T0 + 7 * MIN).unwrap();
    assert!(archive.exists());
    assert!(!log(&d).exists());
    assert_eq!(names(&sum, "ready"), [t1.0]);
    assert_eq!(sum["ready"][0]["result"], "pr-ready");
    assert!(sum["headline"].as_str().unwrap().contains("1 ready"));
}

#[test]
fn ending_closes_the_window_so_unfinished_work_is_listed_not_hidden() {
    let d = tempfile::tempdir().unwrap();
    let mut a = Afk::start(&log(&d), "me", 60 * MIN, 2, T0).unwrap();
    let (t1, _) = a.add(T0 + 1, None, "still going").unwrap();
    let (sum, _) = a.end(T0 + 2).unwrap();
    assert_eq!(names(&sum, "unfinished"), [t1.0]);
}

#[test]
fn reopening_replays_to_the_same_digest_and_state() {
    let d = tempfile::tempdir().unwrap();
    let mut a = Afk::start(&log(&d), "me", 120 * MIN, 2, T0).unwrap();
    let (t1, _) = a.add(T0 + 1, Some("G1"), "one").unwrap();
    a.add(T0 + 2, Some("G2"), "two").unwrap();
    a.add(T0 + 3, Some("G3"), "three").unwrap();
    a.report(
        T0 + 9,
        t1,
        Kind::Done,
        Some("merged nothing, no change needed".into()),
    )
    .unwrap();
    let digest = a.router().digest();
    let sum = a.summary();
    drop(a);
    let b = Afk::open(&log(&d)).unwrap();
    assert_eq!(b.router().digest(), digest);
    assert_eq!(b.summary(), sum);
    assert_eq!(b.closes(), T0 + 120 * MIN);
}

#[test]
fn a_session_can_be_resumed_by_a_later_process_and_keeps_numbering_triggers() {
    let d = tempfile::tempdir().unwrap();
    {
        let mut a = Afk::start(&log(&d), "me", 120 * MIN, 3, T0).unwrap();
        a.add(T0 + 1, Some("G1"), "one").unwrap();
    }
    let mut b = Afk::open(&log(&d)).unwrap();
    let (t, s) = b.add(T0 + 2, Some("G2"), "two").unwrap();
    assert_eq!(t.0, 2);
    assert_eq!(starts(&s), [2]);
}

#[test]
fn a_tampered_line_is_refused_with_its_line_number() {
    let d = tempfile::tempdir().unwrap();
    {
        let mut a = Afk::start(&log(&d), "me", 120 * MIN, 3, T0).unwrap();
        a.add(T0 + 1, Some("G1"), "one").unwrap();
        a.add(T0 + 2, Some("G2"), "two").unwrap();
    }
    let text = std::fs::read_to_string(log(&d)).unwrap();
    let forged = text.replacen("afk-1", "afk-9", 1);
    std::fs::write(log(&d), forged).unwrap();
    match Afk::open(&log(&d)) {
        Err(AfkError::Corrupt { line, why }) => {
            assert!(line >= 2);
            assert_eq!(why, "digest mismatch");
        }
        Err(e) => panic!("unexpected error {e}"),
        Ok(_) => panic!("a forged log was accepted"),
    }
}

#[test]
fn a_half_written_last_line_from_a_crash_is_dropped_and_the_session_continues() {
    let d = tempfile::tempdir().unwrap();
    {
        let mut a = Afk::start(&log(&d), "me", 120 * MIN, 3, T0).unwrap();
        a.add(T0 + 1, Some("G1"), "one").unwrap();
    }
    let mut text = std::fs::read_to_string(log(&d)).unwrap();
    text.push_str("{\"k\":\"Step\",\"at\":17000000");
    std::fs::write(log(&d), text).unwrap();
    let mut b = Afk::open(&log(&d)).unwrap();
    assert_eq!(b.router().tickets().count(), 1);
    b.add(T0 + 5, Some("G2"), "two").unwrap();
    drop(b);
    assert!(Afk::open(&log(&d)).is_ok());
}

#[test]
fn a_missing_log_says_there_is_no_session() {
    let d = tempfile::tempdir().unwrap();
    assert!(matches!(Afk::open(&log(&d)), Err(AfkError::NoSession)));
}

#[test]
fn memory_pressure_holds_back_new_goals_and_recovery_releases_them() {
    let d = tempfile::tempdir().unwrap();
    let mut a = Afk::start(&log(&d), "me", 120 * MIN, 3, T0).unwrap();
    a.pressure(T0 + 1, 1_000, 16_000).unwrap();
    let (t1, s1) = a.add(T0 + 2, None, "heavy build").unwrap();
    assert!(starts(&s1).is_empty());
    let up = a.pressure(T0 + 3, 12_000, 16_000).unwrap();
    assert_eq!(starts(&up), [t1.0]);
    assert!(a.pressure(T0 + 4, 12_100, 16_000).unwrap().is_empty());
}

#[test]
fn empty_goal_unknown_ticket_and_garbage_are_errors_that_leave_the_log_valid() {
    let d = tempfile::tempdir().unwrap();
    let mut a = Afk::start(&log(&d), "me", 120 * MIN, 3, T0).unwrap();
    assert!(matches!(a.add(T0 + 1, None, "   "), Err(AfkError::Empty)));
    assert!(matches!(
        a.report(T0 + 2, TicketId(77), Kind::Ready, None),
        Err(AfkError::UnknownTicket(_))
    ));
    drop(a);
    assert!(Afk::open(&log(&d)).is_ok());
}

#[test]
fn the_afk_policy_admits_nothing_inferred_even_where_everything_is_received() {
    let mut p = afk_policy("me");
    assert_eq!(p.mode, AcceptMode::Off);
    p.sources
        .get_mut(SOURCE)
        .unwrap()
        .scopes
        .get_mut(SCOPE)
        .unwrap()
        .receive_all = true;
    let quiet = Trigger {
        id: TriggerId::new("x"),
        source: SOURCE.into(),
        scope: SCOPE.into(),
        thread: ThreadId::new("afk/1"),
        actor: Some(Actor {
            id: "me".into(),
            bot: false,
        }),
        addressed: false,
    };
    assert_eq!(gate(&p, &quiet, false), Err(Refusal::ModeOff));
}
