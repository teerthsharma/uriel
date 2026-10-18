mod common;

use common::*;
use uriel_core::Graph;
use uriel_house::*;

fn chart_of(need: &str, fellow: Fellow, goals: Vec<Goal>) -> Chart {
    Chart {
        need: need.into(),
        fellow,
        goals,
    }
}

fn flat(root: &str, ids: &[&str]) -> Vec<Goal> {
    ids.iter().map(|i| leaf(i, root)).collect()
}

fn second_need_merged(e: &Env) {
    e.house.need("p", "one", &[]).expect("need one");
    e.house
        .submit_chart(Fellow::Foreman, chart(Fellow::Foreman, base_goals()))
        .expect("chart one");
    e.house.merge().expect("merge one");
    assert_eq!(
        e.house.need("p", "two", &[]).expect("need two").as_str(),
        "N2.G0"
    );
    e.house
        .submit_chart(
            Fellow::Foreman,
            chart_of("N2.G0", Fellow::Foreman, flat("N2.G0", &["N2.G1", "N2.G2"])),
        )
        .expect("chart two");
    e.house.merge().expect("merge two");
}

#[test]
fn a_second_need_gets_the_root_n2_g0_and_the_first_keeps_g0() {
    let e = env();
    let ids: Vec<String> = ["one", "two", "three"]
        .iter()
        .map(|g| e.house.need("p", g, &[]).expect("need").to_string())
        .collect();
    assert_eq!(ids, ["G0", "N2.G0", "N3.G0"]);
}

#[test]
fn a_second_need_has_its_own_goal_tree_and_leaves_the_first_tree_untouched() {
    let e = env();
    second_need_merged(&e);
    let listed = |e: &Env| -> Vec<String> {
        e.house.status().expect("status")["goal_list"]
            .as_array()
            .expect("goal_list")
            .iter()
            .map(|g| g["id"].as_str().expect("id").to_string())
            .collect()
    };
    assert_eq!(listed(&e), ["N2.G0", "N2.G1", "N2.G2"]);
    let b = e.house.next().expect("next");
    assert_eq!(ids(&b), ["N2.G1", "N2.G2"]);
}

#[test]
fn a_chart_for_the_second_need_cannot_reuse_ids_of_the_first_namespace() {
    let e = env();
    e.house.need("p", "one", &[]).expect("need one");
    e.house.need("p", "two", &[]).expect("need two");
    let v = e
        .house
        .submit_chart(
            Fellow::Foreman,
            chart_of("N2.G0", Fellow::Foreman, flat("N2.G0", &["G1", "N2.G2"])),
        )
        .expect_err("a G1 goal belongs to the first need");
    assert!(
        v.iter()
            .any(|v| v.rule == 0 && v.goal.as_str() == "G1" && v.message.contains("namespace")),
        "{v:?}"
    );
}

#[test]
fn a_chart_naming_a_need_that_does_not_exist_is_refused() {
    let e = env();
    e.house.need("p", "one", &[]).expect("need one");
    let v = e
        .house
        .submit_chart(
            Fellow::Foreman,
            chart_of("N2.G0", Fellow::Foreman, flat("N2.G0", &["N2.G1", "N2.G2"])),
        )
        .expect_err("no second need yet");
    assert!(v.iter().any(|v| v.rule == 0), "{v:?}");
}

#[test]
fn goal_ids_of_later_needs_sort_after_every_goal_of_the_first_need() {
    let mut all: Vec<GoalId> = ["N3.G1", "N2.G10", "G10", "N2.G2", "G2", "N2.G0", "G0", "G1.10", "G1.2"]
        .iter()
        .map(|s| gid(s))
        .collect();
    all.sort();
    let got: Vec<&str> = all.iter().map(GoalId::as_str).collect();
    assert_eq!(
        got,
        ["G0", "G1.2", "G1.10", "G2", "G10", "N2.G0", "N2.G2", "N2.G10", "N3.G1"]
    );
}

fn open_at(path: &std::path::Path, root: &std::path::Path) -> House {
    let graph = Graph::open(path, uriel_core::MemoryBudget::mib(80)).expect("graph");
    House::open(
        graph,
        HouseConfig::at(root),
        Box::new(FakeProbe::new(16_000, 16_000)),
        Box::new(VecSink::new()),
    )
    .expect("open")
}

#[test]
fn both_needs_and_their_charts_survive_a_reopen() {
    let dir = project();
    let db = dir.path().join("house.redb");
    {
        let house = open_at(&db, dir.path());
        house.need("p", "one", &[]).expect("need one");
        house
            .submit_chart(Fellow::Foreman, chart(Fellow::Foreman, base_goals()))
            .expect("chart one");
        house.merge().expect("merge one");
        house.need("p", "two", &[]).expect("need two");
        house
            .submit_chart(
                Fellow::Foreman,
                chart_of("N2.G0", Fellow::Foreman, flat("N2.G0", &["N2.G1", "N2.G2"])),
            )
            .expect("chart two");
    }
    let house = open_at(&db, dir.path());
    assert_eq!(house.status().expect("status")["need"]["id"], "N2.G0");
    house.merge().expect("the stored N2 chart merges after a reopen");
    let batch = house.next().expect("next");
    assert_eq!(ids(&batch), ["N2.G1", "N2.G2"]);
    assert_eq!(
        house.need("p", "three", &[]).expect("need three").as_str(),
        "N3.G0"
    );
}

fn two_open_needs(e: &Env) {
    e.house.need("p", "one", &[]).expect("need one");
    e.house
        .submit_chart(
            Fellow::Foreman,
            chart(Fellow::Foreman, flat("G0", &["G1", "G2"])),
        )
        .expect("chart one");
    e.house.merge().expect("merge one");
    e.house.need("p", "two", &[]).expect("need two");
    e.house
        .submit_chart(
            Fellow::Foreman,
            chart_of("N2.G0", Fellow::Foreman, flat("N2.G0", &["N2.G1", "N2.G2"])),
        )
        .expect("chart two");
    e.house.merge().expect("merge two");
}

#[test]
fn a_call_without_a_need_skips_a_complete_need_and_targets_the_newest_open_one() {
    let e = env();
    two_open_needs(&e);
    e.house
        .next_in(Some(&gid("N2.G0")))
        .expect("dispatch the second need");
    prove(&e, "N2.G1");
    prove(&e, "N2.G2");
    assert_eq!(e.house.status().expect("status")["complete"], false);
    let b = e.house.next().expect("next");
    assert_eq!(ids(&b), ["G1", "G2"]);
    assert!(!b.complete);
}

#[test]
fn a_call_without_a_need_falls_back_to_the_newest_need_when_every_need_is_complete() {
    let e = env();
    two_open_needs(&e);
    e.house.next_in(Some(&gid("G0"))).expect("next one");
    e.house.next_in(Some(&gid("N2.G0"))).expect("next two");
    for (need, g) in [("G0", "G1"), ("G0", "G2"), ("N2.G0", "N2.G1"), ("N2.G0", "N2.G2")] {
        prove_in(&e, need, g);
    }
    let b = e.house.next().expect("next");
    assert!(b.complete);
    assert_eq!(e.house.status().expect("status")["need"]["id"], "N2.G0");
}

#[test]
fn next_in_names_an_older_need_while_a_newer_one_is_open() {
    let e = env();
    two_open_needs(&e);
    let first = e.house.next_in(Some(&gid("G0"))).expect("next one");
    assert_eq!(ids(&first), ["G1", "G2"]);
    let second = e.house.next().expect("next default");
    assert_eq!(ids(&second), ["N2.G1", "N2.G2"]);
}

#[test]
fn merge_in_merges_the_named_need_and_leaves_the_newest_unmerged() {
    let e = env();
    e.house.need("p", "one", &[]).expect("need one");
    e.house
        .submit_chart(
            Fellow::Foreman,
            chart(Fellow::Foreman, flat("G0", &["G1", "G2"])),
        )
        .expect("chart one");
    e.house.need("p", "two", &[]).expect("need two");
    let report = e.house.merge_in(Some(&gid("G0"))).expect("merge one");
    assert_eq!(report.leaves, 2);
    assert!(matches!(
        e.house.next_in(Some(&gid("N2.G0"))),
        Err(HouseError::NotMerged)
    ));
    assert_eq!(
        ids(&e.house.next_in(Some(&gid("G0"))).expect("next one")),
        ["G1", "G2"]
    );
}

#[test]
fn a_fact_in_the_named_need_retires_its_goal_and_leaves_the_other_need_alone() {
    let e = env();
    two_open_needs(&e);
    let fact = Fact {
        id: "F1".into(),
        by: "Wilson".into(),
        refutes: Some(gid("G1")),
        source: "x:1".into(),
        text: "G1 is dead".into(),
    };
    e.house
        .add_fact_in(Some(&gid("G0")), fact)
        .expect("fact in G0");
    let one = e.house.status_in(Some(&gid("G0"))).expect("status one");
    assert!(one["retired"].as_array().expect("retired").len() + one["pending_rechart"].as_array().expect("pending").len() >= 1, "{one}");
    let two = e.house.status_in(Some(&gid("N2.G0"))).expect("status two");
    assert_eq!(two["goals"]["retired"], 0);
    assert_eq!(two["goals"]["blocked"], 0);
}

#[test]
fn status_in_describes_only_the_named_need_and_rejects_an_unknown_one() {
    let e = env();
    two_open_needs(&e);
    let one = e.house.status_in(Some(&gid("G0"))).expect("status one");
    assert_eq!(one["need"]["id"], "G0");
    let listed: Vec<&str> = one["goal_list"]
        .as_array()
        .expect("goal_list")
        .iter()
        .map(|g| g["id"].as_str().expect("id"))
        .collect();
    assert_eq!(listed, ["G0", "G1", "G2"]);
    assert!(matches!(
        e.house.status_in(Some(&gid("N9.G0"))),
        Err(HouseError::UnknownNeed(_))
    ));
}

fn prove_in(e: &Env, need: &str, goal: &str) {
    let need = gid(need);
    let test = format!("t_{goal}");
    for status in [TestStatus::Red, TestStatus::Green] {
        e.house
            .record_test_run_in(Some(&need), &test, status, 1.0, 100, &[])
            .expect("run");
    }
    let out = e
        .house
        .report_in(
            Some(&need),
            &gid(goal),
            GoalStatus::Reached,
            evidence(&test, TestStatus::Green),
        )
        .expect("report");
    assert_eq!(out.outcome, Outcome::Accepted, "{out:?}");
}

fn status_in_of(e: &Env, need: &str, id: &str) -> String {
    e.house.status_in(Some(&gid(need))).expect("status")["goal_list"]
        .as_array()
        .expect("goal_list")
        .iter()
        .find(|g| g["id"] == id)
        .unwrap_or_else(|| panic!("goal {id} not in status"))["status"]
        .as_str()
        .expect("status str")
        .to_string()
}

fn writing(mut g: Goal, file: &str) -> Goal {
    g.writes = vec![file.to_string()];
    g
}

fn tests_of(picks: &[TestPick]) -> Vec<&str> {
    picks.iter().map(|p| p.test.as_str()).collect()
}

fn blocked_evidence() -> TestEvidence {
    evidence("", TestStatus::Green)
}

#[test]
fn a_report_for_a_goal_of_another_need_is_refused_and_leaves_that_goal_untouched() {
    let e = env();
    two_open_needs(&e);
    e.house.next_in(Some(&gid("G0"))).expect("dispatch one");
    let by_default = e
        .house
        .report(&gid("G1"), GoalStatus::Blocked, blocked_evidence());
    assert!(
        matches!(by_default, Err(HouseError::Invalid(_))),
        "{by_default:?}"
    );
    let by_name = e.house.report_in(
        Some(&gid("N2.G0")),
        &gid("G1"),
        GoalStatus::Blocked,
        blocked_evidence(),
    );
    assert!(matches!(by_name, Err(HouseError::Invalid(_))), "{by_name:?}");
    assert_eq!(status_in_of(&e, "G0", "G1"), "active");
    let ok = e
        .house
        .report_in(
            Some(&gid("G0")),
            &gid("G1"),
            GoalStatus::Blocked,
            blocked_evidence(),
        )
        .expect("report in its own need");
    assert_eq!(ok.outcome, Outcome::Recorded);
}

#[test]
fn a_test_run_for_a_test_of_another_need_is_refused() {
    let e = env();
    two_open_needs(&e);
    let by_default = e
        .house
        .record_test_run("t_G1", TestStatus::Red, 1.0, 10, &[]);
    assert!(
        matches!(by_default, Err(HouseError::Invalid(_))),
        "{by_default:?}"
    );
    e.house
        .record_test_run_in(Some(&gid("G0")), "t_G1", TestStatus::Red, 1.0, 10, &[])
        .expect("run in its own need");
}

#[test]
fn a_fact_refuting_a_goal_of_another_need_is_refused() {
    let e = env();
    two_open_needs(&e);
    let fact = |id: &str| Fact {
        id: id.into(),
        by: "Wilson".into(),
        refutes: Some(gid("G1")),
        source: "x:1".into(),
        text: "G1 is dead".into(),
    };
    let by_default = e.house.add_fact(fact("F1"));
    assert!(
        matches!(by_default, Err(HouseError::Invalid(_))),
        "{by_default:?}"
    );
    assert_eq!(status_of(&e, "N2.G1"), "pending");
    e.house
        .add_fact_in(Some(&gid("G0")), fact("F1"))
        .expect("fact in its own need");
}

#[test]
fn a_leap_for_a_goal_of_another_need_is_refused() {
    let e = env();
    two_open_needs(&e);
    let r = e.house.leap_in(Some(&gid("N2.G0")), &gid("G1"), "dead");
    assert!(matches!(r, Err(HouseError::Invalid(_))), "{r:?}");
}

#[test]
fn a_test_name_already_bound_in_another_need_is_rejected_at_merge() {
    let e = env();
    two_open_needs(&e);
    let mut clash = leaf("N2.G3", "N2.G0");
    clash.test.as_mut().expect("test").name = "t_G1".into();
    let mut goals = flat("N2.G0", &["N2.G1", "N2.G2"]);
    goals.push(clash);
    e.house
        .submit_chart(Fellow::Foreman, chart_of("N2.G0", Fellow::Foreman, goals))
        .expect("chart");
    let v = e.house.merge().expect_err("clashing test name");
    assert!(
        v.iter()
            .any(|v| v.rule == 8 && v.goal.as_str() == "N2.G3" && v.message.contains("t_G1")),
        "{v:?}"
    );
}

#[test]
fn select_tests_in_returns_only_the_tests_of_the_named_need() {
    let e = env();
    e.house.need("p", "one", &[]).expect("need one");
    e.house
        .submit_chart(
            Fellow::Foreman,
            chart(
                Fellow::Foreman,
                vec![
                    writing(leaf("G1", "G0"), "src/a.rs"),
                    writing(leaf("G2", "G0"), "src/b.rs"),
                ],
            ),
        )
        .expect("chart one");
    e.house.merge().expect("merge one");
    e.house.need("p", "two", &[]).expect("need two");
    e.house
        .submit_chart(
            Fellow::Foreman,
            chart_of(
                "N2.G0",
                Fellow::Foreman,
                vec![
                    writing(leaf("N2.G1", "N2.G0"), "src/a.rs"),
                    leaf("N2.G2", "N2.G0"),
                ],
            ),
        )
        .expect("chart two");
    e.house.merge().expect("merge two");
    let touched = vec!["src/a.rs".to_string()];
    let one = e
        .house
        .select_tests_in(Some(&gid("G0")), &touched)
        .expect("select one");
    assert_eq!(tests_of(&one), ["t_G1"]);
    let newest = e.house.select_tests(&touched).expect("select default");
    assert_eq!(tests_of(&newest), ["t_N2.G1"]);
}

#[test]
fn merging_another_need_leaves_the_first_needs_status_identical() {
    let e = env();
    e.house.need("p", "one", &[]).expect("need one");
    e.house
        .submit_chart(Fellow::Foreman, chart(Fellow::Foreman, base_goals()))
        .expect("chart one");
    e.house.merge().expect("merge one");
    e.house.next().expect("dispatch one");
    let of_need = |e: &Env| {
        let mut v = e.house.status_in(Some(&gid("G0"))).expect("status");
        let obj = v.as_object_mut().expect("object");
        obj.remove("needs");
        obj.remove("current");
        v
    };
    let before = of_need(&e);
    e.house.need("p", "two", &[]).expect("need two");
    e.house
        .submit_chart(
            Fellow::Foreman,
            chart_of("N2.G0", Fellow::Foreman, flat("N2.G0", &["N2.G1", "N2.G2"])),
        )
        .expect("chart two");
    e.house.merge().expect("merge two");
    e.house.next().expect("dispatch two");
    assert_eq!(of_need(&e), before);
}

#[test]
fn a_completed_need_keeps_its_dead_theory_and_fact_and_never_enters_the_next_batch() {
    let e = env();
    e.house.need("p", "one", &[]).expect("need one");
    e.house
        .submit_chart(
            Fellow::Foreman,
            chart(Fellow::Foreman, flat("G0", &["G1", "G2"])),
        )
        .expect("chart one");
    let mut alt = leaf("A1", "G0");
    alt.parent = None;
    alt.alt_of = Some(gid("G1"));
    alt.est = Some(Est {
        ram_mb: 800,
        secs: 60.0,
        tokens: 20_000,
    });
    e.house
        .submit_chart(Fellow::Cameron, chart(Fellow::Cameron, vec![alt]))
        .expect("cameron");
    e.house.merge().expect("merge one");
    e.house
        .add_fact(Fact {
            id: "F1".into(),
            by: "Wilson".into(),
            refutes: Some(gid("G1")),
            source: "x:1".into(),
            text: "G1 is a dead theory".into(),
        })
        .expect("fact");
    let first = e.house.next().expect("dispatch one");
    assert_eq!(ids(&first), ["A1", "G2"]);
    prove(&e, "A1");
    prove(&e, "G2");
    assert!(e.house.next_in(Some(&gid("G0"))).expect("done").complete);
    e.house.need("p", "two", &[]).expect("need two");
    e.house
        .submit_chart(
            Fellow::Foreman,
            chart_of("N2.G0", Fellow::Foreman, flat("N2.G0", &["N2.G1", "N2.G2"])),
        )
        .expect("chart two");
    e.house.merge().expect("merge two");
    let second = e.house.next().expect("dispatch two");
    assert_eq!(ids(&second), ["N2.G1", "N2.G2"]);
    let memory = e.house.status_in(Some(&gid("G0"))).expect("memory");
    assert_eq!(memory["complete"], true);
    assert_eq!(memory["retired"][0]["goal"], "G1");
    assert_eq!(memory["facts"][0]["refutes"], "G1");
}

#[test]
fn two_leaves_of_different_needs_writing_one_file_are_serialised_by_the_file_lock() {
    let e = env();
    e.house.need("p", "one", &[]).expect("need one");
    e.house
        .submit_chart(
            Fellow::Foreman,
            chart(
                Fellow::Foreman,
                vec![
                    writing(leaf("G1", "G0"), "src/a.rs"),
                    writing(leaf("G2", "G0"), "src/b.rs"),
                ],
            ),
        )
        .expect("chart one");
    e.house.merge().expect("merge one");
    e.house.need("p", "two", &[]).expect("need two");
    e.house
        .submit_chart(
            Fellow::Foreman,
            chart_of(
                "N2.G0",
                Fellow::Foreman,
                vec![
                    writing(leaf("N2.G1", "N2.G0"), "src/a.rs"),
                    writing(leaf("N2.G2", "N2.G0"), "src/c.rs"),
                ],
            ),
        )
        .expect("chart two");
    e.house.merge().expect("merge two");
    let first = e.house.next_in(Some(&gid("G0"))).expect("dispatch one");
    assert_eq!(ids(&first), ["G1", "G2"]);
    let second = e.house.next().expect("dispatch two");
    assert_eq!(ids(&second), ["N2.G2"]);
    let held = second
        .governor
        .deferrals
        .iter()
        .find(|d| d.goal.as_str() == "N2.G1")
        .expect("N2.G1 deferred");
    assert_eq!(held.reason, DeferReason::FileLock);
    assert!(
        held.detail.contains("src/a.rs") && held.detail.contains("G1"),
        "{held:?}"
    );
    prove_in(&e, "G0", "G1");
    let third = e.house.next().expect("dispatch after release");
    assert_eq!(ids(&third), ["N2.G1"]);
}

#[test]
fn the_heavy_cap_counts_active_goals_of_every_need() {
    let e = env_with(16_000, 16_000, |c| c.heavy_cap = 1);
    two_open_needs(&e);
    let first = e.house.next_in(Some(&gid("G0"))).expect("dispatch one");
    assert_eq!(ids(&first).len(), 1);
    let second = e.house.next().expect("dispatch two");
    assert!(second.dispatches.is_empty(), "{:?}", ids(&second));
    assert!(second
        .governor
        .deferrals
        .iter()
        .any(|d| d.reason == DeferReason::HeavyCap));
}

#[test]
fn status_lists_every_need_with_goal_counts_by_status_and_marks_the_current_one() {
    let e = env();
    assert_eq!(e.house.status().expect("empty")["needs"], serde_json::json!([]));
    two_open_needs(&e);
    e.house.next_in(Some(&gid("G0"))).expect("dispatch one");
    prove_in(&e, "G0", "G1");
    let s = e.house.status().expect("status");
    assert_eq!(s["current"], "N2.G0");
    let needs = s["needs"].as_array().expect("needs");
    assert_eq!(needs.len(), 2);
    assert_eq!(needs[0]["id"], "G0");
    assert_eq!(needs[0]["goal"], "one");
    assert_eq!(needs[0]["current"], false);
    assert_eq!(needs[0]["complete"], false);
    assert_eq!(needs[0]["merged"], true);
    assert_eq!(needs[0]["goals"]["total"], 2);
    assert_eq!(needs[0]["goals"]["reached"], 1);
    assert_eq!(needs[0]["goals"]["active"], 1);
    assert_eq!(needs[1]["id"], "N2.G0");
    assert_eq!(needs[1]["current"], true);
    assert_eq!(needs[1]["goals"]["pending"], 2);
    let named = e.house.status_in(Some(&gid("G0"))).expect("named");
    assert_eq!(named["need"]["id"], "G0");
    assert_eq!(named["current"], "N2.G0");
    assert_eq!(named["needs"].as_array().expect("needs").len(), 2);
}

#[test]
fn a_complete_need_is_flagged_and_stops_being_current() {
    let e = env();
    two_open_needs(&e);
    e.house.next_in(Some(&gid("N2.G0"))).expect("dispatch two");
    prove_in(&e, "N2.G0", "N2.G1");
    prove_in(&e, "N2.G0", "N2.G2");
    let s = e.house.status().expect("status");
    assert_eq!(s["current"], "G0");
    let needs = s["needs"].as_array().expect("needs");
    assert_eq!(needs[1]["complete"], true);
    assert_eq!(needs[1]["current"], false);
    assert_eq!(needs[0]["current"], true);
}
