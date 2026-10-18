use uriel_algo::caustic::{
    admissible_error_bound, admissible_precision_bound, audit_orbits, audit_pooling,
    certified_precision_bound, orbit_error_bound, partition, pooling_recovery_bound, recall_floor,
    Claims,
};

const EPS: f64 = 1e-12;

fn no_claims() -> Claims {
    Claims { errors: None, precision: None, recall: None }
}

fn errors(n: usize) -> Claims {
    Claims { errors: Some(n), precision: None, recall: None }
}

#[test]
fn t1_matches_the_caustic_measured_values() {
    assert_eq!(orbit_error_bound(20, 15), Ok(5));
    assert_eq!(orbit_error_bound(20, 1), Ok(19));
    assert_eq!(orbit_error_bound(20, 20), Ok(0));
}

#[test]
fn t1_rejects_impossible_partitions() {
    assert!(orbit_error_bound(0, 0).is_err());
    assert!(orbit_error_bound(5, 0).is_err());
    assert!(orbit_error_bound(5, 6).is_err());
    assert!(orbit_error_bound(usize::MAX, usize::MAX).is_ok());
}

#[test]
fn t1_star_matches_the_caustic_measured_rows() {
    assert_eq!(admissible_error_bound(12, 6), Ok(6));
    assert_eq!(admissible_error_bound(20, 0), Ok(20));
    assert_eq!(admissible_error_bound(16, 0), Ok(16));
    assert_eq!(admissible_error_bound(12, 12), Ok(0));
}

#[test]
fn t1_star_rejects_out_of_range_input() {
    assert!(admissible_error_bound(0, 0).is_err());
    assert!(admissible_error_bound(3, 4).is_err());
}

#[test]
fn t6_matches_the_caustic_values() {
    assert_eq!(certified_precision_bound(8, 4, 5), Ok(0.8));
    assert_eq!(certified_precision_bound(6, 4, 4), Ok(0.5));
    assert_eq!(certified_precision_bound(20, 1, 20), Ok(0.95));
}

#[test]
fn t6_rejects_an_empty_or_inconsistent_flagged_set() {
    assert!(certified_precision_bound(20, 20, 0).is_err());
    assert!(certified_precision_bound(5, 3, 6).is_err());
    assert!(certified_precision_bound(5, 3, 1).is_err());
    assert!(certified_precision_bound(0, 0, 0).is_err());
}

#[test]
fn t6_floor_is_at_least_one_half_on_any_consistent_partition() {
    for n in 2..=30usize {
        for m in 1..n {
            for flagged in (n - m)..=n {
                let blocks = flagged - (n - m);
                if flagged == 0 || 2 * blocks > flagged {
                    continue;
                }
                let v = certified_precision_bound(n, m, flagged).expect("t6");
                assert!(v >= 0.5 - EPS && v <= 1.0, "n={n} m={m} S={flagged} v={v}");
            }
        }
    }
}

#[test]
fn t6_star_matches_the_caustic_values() {
    assert_eq!(admissible_precision_bound(20, 0), Ok(1.0));
    assert_eq!(admissible_precision_bound(5, 1), Ok(0.8));
}

#[test]
fn t6_star_rejects_bad_counts_without_overflow() {
    assert!(admissible_precision_bound(0, 0).is_err());
    assert!(admissible_precision_bound(3, 2).is_err());
    assert!(admissible_precision_bound(4, 2).is_ok());
    assert!(admissible_precision_bound(10, usize::MAX).is_err());
    assert!(admissible_precision_bound(usize::MAX, usize::MAX / 2).is_ok());
}

#[test]
fn t6_star_never_falls_below_t6() {
    for n in 2..=24usize {
        for m in 1..n {
            for flagged in (n - m)..=n {
                let blocks = flagged - (n - m);
                if flagged == 0 || 2 * blocks > flagged {
                    continue;
                }
                let plain = certified_precision_bound(n, m, flagged).expect("t6");
                for b_adm in 0..=blocks {
                    let star = admissible_precision_bound(flagged, b_adm).expect("t6*");
                    assert!(star + EPS >= plain, "n={n} m={m} S={flagged} b_adm={b_adm}");
                }
            }
        }
    }
}

#[test]
fn t2_is_the_reciprocal_block_size() {
    assert_eq!(pooling_recovery_bound(1), Ok(1.0));
    assert_eq!(pooling_recovery_bound(20), Ok(0.05));
    assert_eq!(pooling_recovery_bound(4), Ok(0.25));
    assert!(pooling_recovery_bound(0).is_err());
}

#[test]
fn t8_is_zero_exactly_at_the_shuffle_witness() {
    assert_eq!(recall_floor(12, 12), Ok(0.0));
    assert_eq!(recall_floor(12, 11), Ok(1.0 / 12.0));
    assert_eq!(recall_floor(20, 0), Ok(1.0));
    assert!(recall_floor(0, 0).is_err());
    assert!(recall_floor(3, 4).is_err());
}

#[test]
fn every_bound_holds_exhaustively_and_t8_matches_the_caustic_check_count() {
    let mut checks = 0u64;
    let mut positive = 0u64;
    let mut tightest = f64::INFINITY;
    let mut t1_tight = false;
    let mut t6_checked = 0u64;
    let mut t6_star_checked = 0u64;
    for n in 2..=5usize {
        let vocab = n + 2;
        let mut truth: Vec<usize> = (0..n).collect();
        let mut perms: Vec<Vec<usize>> = Vec::new();
        permute(&mut truth, 0, &mut perms);
        let total = vocab.pow(n as u32);
        for perm in &perms {
            for code in 0..total {
                let mut f = [0usize; 5];
                let mut c = code;
                for slot in f.iter_mut().take(n) {
                    *slot = c % vocab;
                    c /= vocab;
                }
                let wrong: Vec<usize> = (0..n).filter(|&i| f[i] != perm[i]).collect();
                if wrong.is_empty() {
                    continue;
                }
                let mut size = [0usize; 8];
                for &a in f.iter().take(n) {
                    size[a] += 1;
                }
                let m = size.iter().filter(|&&s| s > 0).count();
                let m_star = (0..n).filter(|&a| size[a] > 0).count();
                let flagged_set: Vec<usize> =
                    (0..n).filter(|&i| size[f[i]] > 1 || f[i] >= n).collect();
                let s_set: Vec<usize> = (0..n).filter(|&i| size[f[i]] > 1).collect();
                let b_adm = (0..n).filter(|&a| size[a] > 1).count();

                let t1 = orbit_error_bound(n, m).expect("t1");
                let t1s = admissible_error_bound(n, m_star).expect("t1*");
                assert!(t1 <= wrong.len(), "T1 n={n} f={f:?} t={perm:?}");
                assert!(t1s <= wrong.len(), "T1* n={n} f={f:?} t={perm:?}");
                assert!(t1s >= t1);
                if t1 == wrong.len() {
                    t1_tight = true;
                }

                let recall = flagged_set.iter().filter(|i| wrong.contains(i)).count() as f64
                    / wrong.len() as f64;
                let floor = recall_floor(n, m_star).expect("t8");
                assert!(recall + EPS >= floor, "T8 n={n} f={f:?} t={perm:?}");
                checks += 1;
                if floor > 0.0 {
                    positive += 1;
                }
                tightest = tightest.min(recall - floor);

                if !s_set.is_empty() {
                    let precision = s_set.iter().filter(|i| wrong.contains(i)).count() as f64
                        / s_set.len() as f64;
                    let t6 = certified_precision_bound(n, m, s_set.len()).expect("t6");
                    assert!(precision + EPS >= t6, "T6 n={n} f={f:?} t={perm:?}");
                    t6_checked += 1;
                    let t6s = admissible_precision_bound(s_set.len(), b_adm).expect("t6*");
                    assert!(precision + EPS >= t6s, "T6* n={n} f={f:?} t={perm:?}");
                    t6_star_checked += 1;
                }
            }
        }
    }
    assert_eq!(checks, 2_048_574);
    assert!(tightest.abs() < EPS, "tightest margin {tightest}");
    assert!(positive as f64 / checks as f64 > 0.99);
    assert!(t1_tight);
    assert!(t6_checked > 0 && t6_star_checked > 0);
}

fn permute(items: &mut Vec<usize>, k: usize, out: &mut Vec<Vec<usize>>) {
    if k == items.len() {
        out.push(items.clone());
        return;
    }
    for i in k..items.len() {
        items.swap(k, i);
        permute(items, k + 1, out);
        items.swap(k, i);
    }
}

#[test]
fn the_partition_groups_by_first_appearance() {
    let p = partition(&["b", "a", "b", "c", "a"]);
    assert_eq!(p.n, 5);
    assert_eq!(p.orbits, vec![vec![0, 2], vec![1, 4], vec![3]]);
    assert_eq!(p.m(), 3);
    assert_eq!(p.flagged(), 4);
}

#[test]
fn an_empty_relation_is_rejected_not_panicked_on() {
    let none: [&str; 0] = [];
    assert!(audit_orbits(&none, None, &errors(0)).is_err());
    assert!(audit_pooling(&none, &[]).is_err());
}

#[test]
fn a_collision_contradicts_the_claim_that_every_answer_is_correct() {
    let a = audit_orbits(&["x", "y", "x", "z"], None, &errors(0)).expect("audit");
    assert!(a.violated());
    assert_eq!(a.bound(), "T1");
    assert_eq!(a.collisions, vec![0]);
    assert_eq!(a.m, 3);
    let t1 = a.checks.iter().find(|c| c.bound == "T1").expect("t1");
    assert!(t1.violated);
    assert_eq!(t1.floor, 1.0);
    assert_eq!(t1.claimed, Some(0.0));
}

#[test]
fn a_clean_relation_with_the_same_claim_finds_no_violation() {
    let a = audit_orbits(&["x", "y", "z"], None, &errors(0)).expect("audit");
    assert!(!a.violated());
    assert!(a.collisions.is_empty());
    assert!(a.checks.iter().all(|c| !c.violated));
}

#[test]
fn a_claim_at_exactly_the_floor_is_not_a_violation() {
    let a = audit_orbits(&["x", "x", "x"], None, &errors(2)).expect("audit");
    assert!(!a.violated());
    let b = audit_orbits(&["x", "x", "x"], None, &errors(1)).expect("audit");
    assert!(b.violated());
}

#[test]
fn no_claim_means_no_violation_but_the_floors_are_still_reported() {
    let a = audit_orbits(&["x", "x", "y"], None, &no_claims()).expect("audit");
    assert!(!a.violated());
    let t1 = a.checks.iter().find(|c| c.bound == "T1").expect("t1");
    assert_eq!(t1.floor, 1.0);
    assert_eq!(t1.claimed, None);
}

#[test]
fn the_admissible_set_exposes_what_t1_cannot_see() {
    let answers = ["a", "b", "c", "d"];
    let gold = ["a", "b", "c", "e"];
    let a = audit_orbits(&answers, Some(&gold), &errors(0)).expect("audit");
    assert!(a.violated());
    assert_eq!(a.bound(), "T1*");
    assert_eq!(a.inadmissible, vec![3]);
    assert!(a.collisions.is_empty());
    let t1 = a.checks.iter().find(|c| c.bound == "T1").expect("t1");
    assert!(!t1.violated);
    assert_eq!(a.m_star, Some(3));
}

#[test]
fn an_admissible_set_smaller_than_n_voids_the_star_bounds() {
    let answers = ["a", "b", "c", "d"];
    let gold = ["a", "b"];
    let a = audit_orbits(&answers, Some(&gold), &errors(0)).expect("audit");
    assert!(!a.violated());
    assert_eq!(a.m_star, None);
    let voided: Vec<&str> = a.inapplicable.iter().map(|(b, _)| *b).collect();
    for b in ["T1*", "T6*", "T8"] {
        assert!(voided.contains(&b), "{b} not voided: {voided:?}");
    }
}

#[test]
fn a_superset_admissible_set_still_applies() {
    let answers = ["a", "b", "z"];
    let gold = ["a", "b", "c", "d"];
    let a = audit_orbits(&answers, Some(&gold), &errors(0)).expect("audit");
    assert!(a.violated());
    assert_eq!(a.m_star, Some(2));
}

#[test]
fn a_precision_claim_below_the_t6_floor_is_a_violation() {
    let claims = Claims { errors: None, precision: Some(0.5), recall: None };
    let a = audit_orbits(&["x", "x", "x", "x"], None, &claims).expect("audit");
    assert!(a.violated());
    assert_eq!(a.bound(), "T6");
    let t6 = a.checks.iter().find(|c| c.bound == "T6").expect("t6");
    assert_eq!(t6.floor, 0.75);
    let ok = Claims { errors: None, precision: Some(0.75), recall: None };
    assert!(!audit_orbits(&["x", "x", "x", "x"], None, &ok).expect("audit").violated());
}

#[test]
fn a_precision_claim_is_judged_against_t6_star_when_the_set_applies() {
    let answers = ["t", "t", "t", "t", "u"];
    let gold = ["a", "b", "c", "d", "e"];
    let claims = Claims { errors: None, precision: Some(0.9), recall: None };
    let a = audit_orbits(&answers, Some(&gold), &claims).expect("audit");
    assert!(a.violated());
    assert_eq!(a.bound(), "T6*");
    let star = a.checks.iter().find(|c| c.bound == "T6*").expect("t6*");
    assert_eq!(star.floor, 1.0);
    let plain = a.checks.iter().find(|c| c.bound == "T6").expect("t6");
    assert_eq!(plain.floor, 0.75);
    assert!(!plain.violated);
}

#[test]
fn a_recall_claim_below_the_t8_floor_is_a_violation() {
    let answers = ["t", "t", "t", "t"];
    let gold = ["a", "b", "c", "d"];
    let claims = Claims { errors: None, precision: None, recall: Some(0.5) };
    let a = audit_orbits(&answers, Some(&gold), &claims).expect("audit");
    assert!(a.violated());
    assert_eq!(a.bound(), "T8");
    let t8 = a.checks.iter().find(|c| c.bound == "T8").expect("t8");
    assert_eq!(t8.floor, 1.0);
}

#[test]
fn the_shuffle_adversary_is_never_reported_as_a_violation() {
    let answers: Vec<String> = (0..12).map(|i| format!("a{}", (i + 1) % 12)).collect();
    let refs: Vec<&str> = answers.iter().map(String::as_str).collect();
    let gold: Vec<String> = (0..12).map(|i| format!("a{i}")).collect();
    let grefs: Vec<&str> = gold.iter().map(String::as_str).collect();
    let claims = Claims { errors: Some(0), precision: Some(0.0), recall: Some(0.0) };
    let a = audit_orbits(&refs, Some(&grefs), &claims).expect("audit");
    assert!(!a.violated());
    assert_eq!(a.m_star, Some(12));
    let t8 = a.checks.iter().find(|c| c.bound == "T8").expect("t8");
    assert_eq!(t8.floor, 0.0);
}

#[test]
fn invalid_claim_numbers_are_rejected() {
    for bad in [f64::NAN, f64::INFINITY, -0.1, 1.1] {
        let p = Claims { errors: None, precision: Some(bad), recall: None };
        let r = Claims { errors: None, precision: None, recall: Some(bad) };
        assert!(audit_orbits(&["a", "a"], None, &p).is_err(), "{bad}");
        assert!(audit_orbits(&["a", "a"], None, &r).is_err(), "{bad}");
    }
}

#[test]
fn a_single_entity_and_a_huge_claim_do_not_panic() {
    let a = audit_orbits(&["only"], None, &errors(usize::MAX)).expect("audit");
    assert!(!a.violated());
    assert_eq!(a.m, 1);
}

#[test]
fn ten_thousand_identical_answers_are_audited() {
    let answers = vec!["same"; 10_000];
    let a = audit_orbits(&answers, None, &errors(9_998)).expect("audit");
    assert!(a.violated());
    let t1 = a.checks.iter().find(|c| c.bound == "T1").expect("t1");
    assert_eq!(t1.floor, 9_999.0);
}

#[test]
fn empty_and_unicode_answers_are_ordinary_keys() {
    let a = audit_orbits(&["", "", "\u{1f600}", "\u{1f600}x"], None, &errors(0)).expect("audit");
    assert_eq!(a.m, 3);
    assert_eq!(a.collisions, vec![0]);
}

#[test]
fn the_audit_is_deterministic() {
    let answers = ["q", "w", "q", "e", "w", "r", "q"];
    let gold = ["q", "w", "e", "r", "t", "y", "u"];
    let claims = Claims { errors: Some(1), precision: Some(0.7), recall: Some(0.2) };
    let first = audit_orbits(&answers, Some(&gold), &claims).expect("audit");
    for _ in 0..50 {
        assert_eq!(audit_orbits(&answers, Some(&gold), &claims).expect("audit"), first);
    }
}

#[test]
fn pooling_flags_a_recovery_claim_above_one_over_k() {
    let answers = ["g", "g", "g", "h"];
    let hits = audit_pooling(&answers, &[(0, 0.5), (3, 1.0), (1, 1.0 / 3.0)]).expect("audit");
    assert_eq!(hits.len(), 3);
    assert!(hits[0].violated);
    assert_eq!(hits[0].k, 3);
    assert!(!hits[1].violated);
    assert_eq!(hits[1].k, 1);
    assert!(!hits[2].violated);
    assert_eq!(hits[0].members, vec![0, 1, 2]);
}

#[test]
fn pooling_rejects_bad_entities_and_numbers() {
    let answers = ["a", "b"];
    assert!(audit_pooling(&answers, &[(2, 0.1)]).is_err());
    assert!(audit_pooling(&answers, &[(0, f64::NAN)]).is_err());
    assert!(audit_pooling(&answers, &[(0, 1.5)]).is_err());
    assert!(audit_pooling(&answers, &[(0, -0.5)]).is_err());
}
