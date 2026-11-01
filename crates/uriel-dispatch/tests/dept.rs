use std::collections::BTreeMap;

use uriel_dispatch::dept::*;

fn loads(v: &[(&str, u32)]) -> BTreeMap<String, u32> {
    v.iter().map(|(k, l)| (k.to_string(), *l)).collect()
}

fn spec(json: &str) -> PlanSpec {
    serde_json::from_str(json).unwrap()
}

const SPEC: &str = r#"{"departments":[
 {"name":"Research","goal":"Find the fix point","subgoals":[
   {"goal":"Reproduce","leaves":[{"goal":"Write the failing case","checklist":["seen failing","minimal"]}]},
   {"goal":"Explain","leaves":[{"goal":"Trace the cause","checklist":["a"]},{"goal":"Write it down","checklist":["b","c"]}]}]},
 {"name":"Build & Ship","goal":"Land it","subgoals":[
   {"goal":"Implement","leaves":[{"goal":"Code","checklist":["x"]}]}]}]}"#;

#[test]
fn a_plan_has_three_tiers_with_positional_ids_and_a_checklist_on_every_leaf() {
    let p = Plan::build(&spec(SPEC)).unwrap();
    assert_eq!(p.departments[0].slug, "research");
    assert_eq!(p.departments[1].slug, "build-ship");
    assert_eq!(p.departments[0].subgoals[1].id, "research.2");
    let ids: Vec<&str> = p.departments[0]
        .subgoals
        .iter()
        .flat_map(|s| s.leaves.iter().map(|l| l.id.as_str()))
        .collect();
    assert_eq!(ids, ["research.1.1", "research.2.1", "research.2.2"]);
    assert_eq!(p.progress(), Progress { done: 0, total: 6 });
}

#[test]
fn checking_items_rolls_up_through_leaf_sub_goal_department_and_plan() {
    let mut p = Plan::build(&spec(SPEC)).unwrap();
    p.check("research.2.2", 0, true).unwrap();
    p.check("research.2.2", 1, true).unwrap();
    p.check("build-ship.1.1", 0, true).unwrap();
    let (d, l) = p.leaf("research.2.2").unwrap();
    assert!(l.progress().complete());
    assert_eq!(d.subgoals[1].progress(), Progress { done: 2, total: 3 });
    assert_eq!(d.progress(), Progress { done: 2, total: 5 });
    assert_eq!(p.departments[1].progress().percent(), 100);
    assert_eq!(p.progress(), Progress { done: 3, total: 6 });
    p.check("research.2.2", 0, false).unwrap();
    assert_eq!(p.progress().done, 2);
}

#[test]
fn checking_something_that_does_not_exist_is_an_error_and_changes_nothing() {
    let mut p = Plan::build(&spec(SPEC)).unwrap();
    let before = p.clone();
    assert_eq!(
        p.check("nope.1.1", 0, true),
        Err(PlanError::UnknownLeaf("nope.1.1".into()))
    );
    assert_eq!(
        p.check("research.1.1", 9, true),
        Err(PlanError::BadItem("research.1.1".into(), 9))
    );
    assert_eq!(p, before);
}

#[test]
fn every_structural_mistake_is_refused_with_the_place_it_is_in() {
    let cases = [
        (r#"{"departments":[]}"#, "at least one department"),
        (
            r#"{"departments":[{"name":" ","goal":"g","subgoals":[]}]}"#,
            "department name",
        ),
        (
            r#"{"departments":[{"name":"A","goal":"g","subgoals":[]}]}"#,
            "department 'A'",
        ),
        (
            r#"{"departments":[{"name":"A","goal":"g","subgoals":[{"goal":"s","leaves":[]}]}]}"#,
            "sub-goal a.1",
        ),
        (
            r#"{"departments":[{"name":"A","goal":"g","subgoals":[{"goal":"s","leaves":[{"goal":"l","checklist":[]}]}]}]}"#,
            "leaf a.1.1",
        ),
        (
            r#"{"departments":[{"name":"A","goal":"g","subgoals":[{"goal":"s","leaves":[{"goal":"l","checklist":["  "]}]}]}]}"#,
            "item 1 of leaf a.1.1",
        ),
        (
            r#"{"departments":[{"name":"A b","goal":"g","subgoals":[{"goal":"s","leaves":[{"goal":"l","checklist":["i"]}]}]},{"name":"a-B","goal":"g","subgoals":[{"goal":"s","leaves":[{"goal":"l","checklist":["i"]}]}]}]}"#,
            "collides",
        ),
    ];
    for (json, want) in cases {
        let e = Plan::build(&spec(json)).unwrap_err().to_string();
        assert!(e.contains(want), "{json} -> {e}");
    }
}

#[test]
fn unknown_fields_and_oversized_text_are_refused() {
    assert!(serde_json::from_str::<PlanSpec>(r#"{"departments":[],"extra":1}"#).is_err());
    let long = "x".repeat(MAX_TEXT + 1);
    let json = format!(r#"{{"departments":[{{"name":"A","goal":"{long}","subgoals":[]}}]}}"#);
    assert!(Plan::build(&spec(&json))
        .unwrap_err()
        .to_string()
        .contains("longer than"));
}

#[test]
fn size_caps_hold() {
    let depts: Vec<String> = (0..=MAX_DEPARTMENTS)
        .map(|i| format!(r#"{{"name":"D{i}","goal":"g","subgoals":[{{"goal":"s","leaves":[{{"goal":"l","checklist":["i"]}}]}}]}}"#))
        .collect();
    let json = format!(r#"{{"departments":[{}]}}"#, depts.join(","));
    assert!(Plan::build(&spec(&json))
        .unwrap_err()
        .to_string()
        .contains("too many departments"));
}

#[test]
fn slugs_are_lowercase_dashed_and_trimmed() {
    assert_eq!(slug("Build & Ship"), "build-ship");
    assert_eq!(slug("  Docs!! "), "docs");
    assert_eq!(slug("R&D 2"), "r-d-2");
    assert_eq!(slug("!!!"), "");
}

#[test]
fn distribution_gives_each_busy_department_a_slot_then_splits_the_rest_by_load() {
    let d = distribute(3, &loads(&[("a", 6), ("b", 2)]));
    assert_eq!((d["a"], d["b"]), (2, 1));
    let d = distribute(4, &loads(&[("a", 1), ("b", 1), ("c", 10)]));
    assert_eq!((d["a"], d["b"], d["c"]), (1, 1, 2));
}

#[test]
fn a_department_never_gets_more_slots_than_it_has_work() {
    let d = distribute(8, &loads(&[("a", 1), ("b", 2)]));
    assert_eq!((d["a"], d["b"]), (1, 2));
}

#[test]
fn idle_departments_get_nothing_and_a_scarce_capacity_goes_to_the_busiest() {
    let d = distribute(2, &loads(&[("a", 5), ("b", 0), ("c", 9), ("d", 1)]));
    assert_eq!((d["a"], d["b"], d["c"], d["d"]), (1, 0, 1, 0));
    assert_eq!(distribute(0, &loads(&[("a", 5)]))["a"], 0);
    assert!(distribute(4, &BTreeMap::new()).is_empty());
}

#[test]
fn ties_break_by_name_so_the_result_is_deterministic() {
    let d = distribute(1, &loads(&[("b", 3), ("a", 3)]));
    assert_eq!((d["a"], d["b"]), (1, 0));
    let d = distribute(4, &loads(&[("b", 3), ("a", 3), ("c", 3)]));
    assert_eq!((d["a"], d["b"], d["c"]), (2, 1, 1));
}

#[test]
fn distribution_properties_hold_over_many_random_loads() {
    let mut x = 0x2545_f491_4f6c_dd1du64;
    let mut next = |n: u64| {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x % n
    };
    for _ in 0..4000 {
        let n = 1 + next(6) as usize;
        let l: BTreeMap<String, u32> = (0..n).map(|i| (format!("d{i}"), next(9) as u32)).collect();
        let cap = next(12) as u32;
        let d = distribute(cap, &l);
        let active = l.values().filter(|v| **v > 0).count() as u32;
        let total: u32 = l.values().sum();
        let given: u32 = d.values().sum();
        assert!(given <= cap, "{l:?} {cap} {d:?}");
        assert_eq!(given, cap.min(total), "work conserving: {l:?} {cap} {d:?}");
        for (k, v) in &d {
            assert!(*v <= l[k], "{k} over its load: {l:?} {cap} {d:?}");
            if l[k] == 0 {
                assert_eq!(*v, 0);
            }
        }
        if cap >= active {
            for (k, v) in &l {
                if *v > 0 {
                    assert!(d[k] >= 1, "starved {k}: {l:?} {cap} {d:?}");
                }
            }
        }
        assert_eq!(d, distribute(cap, &l));
    }
}

#[test]
fn a_plan_survives_a_json_round_trip() {
    let mut p = Plan::build(&spec(SPEC)).unwrap();
    p.check("research.1.1", 1, true).unwrap();
    let j = serde_json::to_string(&p).unwrap();
    assert_eq!(serde_json::from_str::<Plan>(&j).unwrap(), p);
}
