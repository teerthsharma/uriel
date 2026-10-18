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
    for g in ["G1", "G2", "N2.G1", "N2.G2"] {
        prove(&e, g);
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
