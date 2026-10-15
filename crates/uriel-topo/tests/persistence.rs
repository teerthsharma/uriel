use uriel_topo::persistence::{
    persistence_f32, persistence_f64, Diagram, Metric, PersistenceConfig,
};
use uriel_topo::rng::SplitMix64;
use uriel_topo::Error;

fn cfg() -> PersistenceConfig {
    PersistenceConfig::default()
}

fn cloud(rng: &mut SplitMix64, n: usize, d: usize) -> Vec<f64> {
    (0..n * d).map(|_| rng.normal()).collect()
}

fn sorted_deaths(dg: &Diagram) -> Vec<f64> {
    let mut v: Vec<f64> = dg.finite.iter().map(|b| b.death).collect();
    v.sort_by(|a, b| a.total_cmp(b));
    v
}

fn close(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol * (1.0 + a.abs().max(b.abs()))
}

fn reference(data: &[f64], d: usize, ids: &[u64]) -> Vec<(f64, u64)> {
    let n = ids.len();
    let mut pairs = Vec::new();
    for i in 0..n {
        for j in i + 1..n {
            let mut s = 0.0;
            for k in 0..d {
                let t = data[i * d + k] - data[j * d + k];
                s += t * t;
            }
            pairs.push((s.sqrt(), i, j));
        }
    }
    pairs.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    let mut comp: Vec<usize> = (0..n).collect();
    let rep: Vec<u64> = ids.to_vec();
    let mut out = Vec::new();
    for (w, i, j) in pairs {
        let (ci, cj) = (comp[i], comp[j]);
        if ci == cj {
            continue;
        }
        let (dying, living) = if rep[ci] > rep[cj] {
            (ci, cj)
        } else {
            (cj, ci)
        };
        out.push((w, rep[dying]));
        for c in comp.iter_mut() {
            if *c == dying {
                *c = living;
            }
        }
    }
    out.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    out
}

#[test]
fn collinear_hand_computed() {
    let dg = persistence_f64(&[0.0, 1.0, 3.0, 6.0], 1, None, &cfg()).unwrap();
    let got: Vec<(f64, u64)> = dg.finite.iter().map(|b| (b.death, b.id)).collect();
    assert_eq!(got, vec![(1.0, 1), (2.0, 2), (3.0, 3)]);
    assert!(dg.finite.iter().all(|b| b.birth == 0.0));
    assert_eq!(dg.essential_id, Some(0));
    assert_eq!(dg.essential_death, 3.0);
    assert_eq!(dg.salience, vec![3.0, 1.0, 2.0, 3.0]);
}

#[test]
fn two_clusters_hand_computed() {
    let pts = [
        0.0, 0.0, 0.1, 0.0, 0.0, 0.1, 10.0, 0.0, 10.1, 0.0, 10.0, 0.1,
    ];
    let dg = persistence_f64(&pts, 2, None, &cfg()).unwrap();
    let d = sorted_deaths(&dg);
    assert_eq!(d.len(), 5);
    for x in &d[..4] {
        assert!(close(*x, 0.1, 1e-12), "{x}");
    }
    assert!(close(d[4], 9.9, 1e-12), "{}", d[4]);
    let bridge = dg.finite.iter().find(|b| b.death > 1.0).unwrap();
    assert_eq!(bridge.id, 3);
    assert_eq!(dg.essential_id, Some(0));
    assert!(close(dg.salience[3], 9.9, 1e-12));
    assert!(close(dg.salience[0], 9.9, 1e-12), "essential gets the cap");
    for i in [1usize, 2, 4, 5] {
        assert!(close(dg.salience[i], 0.1, 1e-12));
    }
}

#[test]
fn equilateral_triangle_hand_computed() {
    let h = 3f64.sqrt() / 2.0;
    let pts = [0.0, 0.0, 1.0, 0.0, 0.5, h];
    let dg = persistence_f64(&pts, 2, None, &cfg()).unwrap();
    assert_eq!(dg.finite.len(), 2);
    for b in &dg.finite {
        assert!(close(b.death, 1.0, 1e-12));
    }
    let mut who: Vec<u64> = dg.finite.iter().map(|b| b.id).collect();
    who.sort();
    assert_eq!(who, vec![1, 2], "ties: the lowest id survives");
    assert_eq!(dg.essential_id, Some(0));
}

#[test]
fn matches_independent_reference_and_counts() {
    let mut rng = SplitMix64::new(1);
    for t in 0..60 {
        let n = 2 + rng.below(40);
        let d = 1 + rng.below(4);
        let mut data = cloud(&mut rng, n, d);
        if t % 3 == 0 {
            for x in data.iter_mut() {
                *x = (*x * 2.0).round();
            }
        }
        let ids: Vec<u64> = (0..n as u64).map(|i| i * 7 + 3).collect();
        let dg = persistence_f64(&data, d, Some(&ids), &cfg()).unwrap();
        assert_eq!(dg.finite.len(), n - 1, "n-1 finite pairs");
        let mut got: Vec<(f64, u64)> = dg.finite.iter().map(|b| (b.death, b.id)).collect();
        got.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        assert_eq!(got, reference(&data, d, &ids), "t={t}");
        assert_eq!(dg.essential_id, Some(*ids.iter().min().unwrap()));
        let mut all: Vec<u64> = dg.finite.iter().map(|b| b.id).collect();
        all.push(dg.essential_id.unwrap());
        all.sort();
        let mut want = ids.clone();
        want.sort();
        assert_eq!(all, want);
    }
}

#[test]
fn stability_two_epsilon_on_200_random_sets() {
    let mut rng = SplitMix64::new(2);
    for _ in 0..200 {
        let n = 8 + rng.below(23);
        let d = 2 + rng.below(2);
        let a = cloud(&mut rng, n, d);
        let eps = 1e-4 + rng.next_f64() * 0.1;
        let mut b = a.clone();
        for i in 0..n {
            let dir: Vec<f64> = (0..d).map(|_| rng.normal()).collect();
            let norm = dir.iter().map(|x| x * x).sum::<f64>().sqrt().max(1e-12);
            let r = eps * rng.next_f64();
            for k in 0..d {
                b[i * d + k] += dir[k] / norm * r;
            }
        }
        let da = sorted_deaths(&persistence_f64(&a, d, None, &cfg()).unwrap());
        let db = sorted_deaths(&persistence_f64(&b, d, None, &cfg()).unwrap());
        let worst = da
            .iter()
            .zip(&db)
            .map(|(x, y)| (x - y).abs())
            .fold(0.0, f64::max);
        assert!(
            worst <= 2.0 * eps + 1e-9,
            "worst {worst} > 2*eps {}",
            2.0 * eps
        );
    }
}

#[test]
fn isometry_invariance_2d() {
    let mut rng = SplitMix64::new(3);
    for _ in 0..40 {
        let n = 5 + rng.below(30);
        let a = cloud(&mut rng, n, 2);
        let th = rng.next_f64() * std::f64::consts::TAU;
        let (s, c) = th.sin_cos();
        let (tx, ty) = (rng.normal() * 5.0, rng.normal() * 5.0);
        let flip = if rng.below(2) == 0 { 1.0 } else { -1.0 };
        let mut b = Vec::new();
        for i in 0..n {
            let (x, y) = (a[2 * i], a[2 * i + 1] * flip);
            b.push(c * x - s * y + tx);
            b.push(s * x + c * y + ty);
        }
        let da = sorted_deaths(&persistence_f64(&a, 2, None, &cfg()).unwrap());
        let db = sorted_deaths(&persistence_f64(&b, 2, None, &cfg()).unwrap());
        for (x, y) in da.iter().zip(&db) {
            assert!(close(*x, *y, 1e-9), "{x} vs {y}");
        }
    }
}

#[test]
fn scale_equivariance() {
    let mut rng = SplitMix64::new(4);
    for &s in &[0.001, 0.37, 3.1, 1000.0] {
        let n = 25;
        let a = cloud(&mut rng, n, 3);
        let b: Vec<f64> = a.iter().map(|x| x * s).collect();
        let da = persistence_f64(&a, 3, None, &cfg()).unwrap();
        let db = persistence_f64(&b, 3, None, &cfg()).unwrap();
        for (x, y) in da.finite.iter().zip(&db.finite) {
            assert!(close(x.death * s, y.death, 1e-9));
            assert_eq!(x.id, y.id);
            assert_eq!(y.birth, 0.0);
        }
        for (x, y) in da.salience.iter().zip(&db.salience) {
            assert!(close(x * s, *y, 1e-9));
        }
    }
}

#[test]
fn permutation_invariance_of_deaths_and_salience_by_id() {
    let mut rng = SplitMix64::new(5);
    for _ in 0..40 {
        let n = 6 + rng.below(40);
        let d = 2 + rng.below(3);
        let mut a = cloud(&mut rng, n, d);
        for x in a.iter_mut() {
            *x = (*x * 3.0).round() / 3.0;
        }
        let ids: Vec<u64> = (0..n as u64).collect();
        let mut perm: Vec<usize> = (0..n).collect();
        rng.shuffle(&mut perm);
        let mut b = Vec::new();
        let mut pids = Vec::new();
        for &p in &perm {
            b.extend_from_slice(&a[p * d..(p + 1) * d]);
            pids.push(ids[p]);
        }
        let da = persistence_f64(&a, d, Some(&ids), &cfg()).unwrap();
        let db = persistence_f64(&b, d, Some(&pids), &cfg()).unwrap();
        assert_eq!(
            sorted_deaths(&da),
            sorted_deaths(&db),
            "bit-identical multiset"
        );
        for (pos, &id) in pids.iter().enumerate() {
            assert_eq!(db.salience[pos], da.salience[id as usize], "id {id}");
        }
        let mut fa: Vec<(u64, u64)> = da
            .finite
            .iter()
            .map(|b| (b.id, b.death.to_bits()))
            .collect();
        let mut fb: Vec<(u64, u64)> = db
            .finite
            .iter()
            .map(|b| (b.id, b.death.to_bits()))
            .collect();
        fa.sort();
        fb.sort();
        assert_eq!(fa, fb);
        assert_eq!(da.essential_id, db.essential_id);
    }
}

#[test]
fn duplicates_die_at_zero() {
    let pts = [1.0, 2.0, 1.0, 2.0, 1.0, 2.0, 5.0, 5.0];
    let dg = persistence_f64(&pts, 2, None, &cfg()).unwrap();
    let zeros = dg.finite.iter().filter(|b| b.death == 0.0).count();
    assert_eq!(zeros, 2);
    assert_eq!(dg.salience[1], 0.0);
    assert_eq!(dg.salience[2], 0.0);
    let c = PersistenceConfig {
        metric: Metric::Cosine,
        ..cfg()
    };
    let dg = persistence_f64(&[0.3, 0.7, 0.3, 0.7, 0.3, 0.7, -1.0, 2.0], 2, None, &c).unwrap();
    assert_eq!(dg.finite.iter().filter(|b| b.death == 0.0).count(), 2);
}

#[test]
fn cosine_is_scale_invariant_per_point_and_bounded() {
    let mut rng = SplitMix64::new(6);
    let n = 30;
    let a = cloud(&mut rng, n, 5);
    let mut b = a.clone();
    for i in 0..n {
        let s = 0.5 + 10.0 * rng.next_f64();
        for k in 0..5 {
            b[i * 5 + k] *= s;
        }
    }
    let c = PersistenceConfig {
        metric: Metric::Cosine,
        ..cfg()
    };
    let da = persistence_f64(&a, 5, None, &c).unwrap();
    let db = persistence_f64(&b, 5, None, &c).unwrap();
    for (x, y) in da.finite.iter().zip(&db.finite) {
        assert!(close(x.death, y.death, 1e-9));
        assert!((0.0..=2.0).contains(&x.death));
    }
}

#[test]
fn f32_agrees_with_f64() {
    let mut rng = SplitMix64::new(7);
    let a: Vec<f32> = (0..20 * 4).map(|_| rng.normal() as f32).collect();
    let b: Vec<f64> = a.iter().map(|&x| f64::from(x)).collect();
    assert_eq!(
        persistence_f32(&a, 4, None, &cfg()).unwrap(),
        persistence_f64(&b, 4, None, &cfg()).unwrap()
    );
}

#[test]
fn adversarial_inputs_return_defined_results() {
    let dg = persistence_f64(&[], 3, None, &cfg()).unwrap();
    assert!(dg.finite.is_empty() && dg.essential_id.is_none() && dg.salience.is_empty());
    let dg = persistence_f64(&[1.0, 2.0], 2, Some(&[42]), &cfg()).unwrap();
    assert!(dg.finite.is_empty());
    assert_eq!(dg.essential_id, Some(42));
    assert_eq!(dg.salience, vec![0.0]);
    assert!(matches!(
        persistence_f64(&[0.0, f64::NAN], 1, None, &cfg()),
        Err(Error::NonFinite(_))
    ));
    assert!(matches!(
        persistence_f64(&[0.0, f64::INFINITY], 1, None, &cfg()),
        Err(Error::NonFinite(_))
    ));
    assert!(matches!(
        persistence_f32(&[0.0, f32::NEG_INFINITY], 1, None, &cfg()),
        Err(Error::NonFinite(_))
    ));
    assert!(matches!(
        persistence_f64(&[0.0, 1.0, 2.0], 2, None, &cfg()),
        Err(Error::Shape(_))
    ));
    assert!(matches!(
        persistence_f64(&[0.0, 1.0], 0, None, &cfg()),
        Err(Error::Shape(_)) | Err(Error::InvalidParameter(_))
    ));
    assert!(matches!(
        persistence_f64(&[0.0, 1.0], 1, Some(&[5, 5]), &cfg()),
        Err(Error::DuplicateId(5))
    ));
    assert!(matches!(
        persistence_f64(&[0.0, 1.0], 1, Some(&[5]), &cfg()),
        Err(Error::Shape(_))
    ));
    assert!(matches!(
        persistence_f64(&[-1e308, 1e308], 1, None, &cfg()),
        Err(Error::Overflow)
    ));
    let dg = persistence_f64(&[1.0; 12], 3, None, &cfg()).unwrap();
    assert!(dg.finite.iter().all(|b| b.death == 0.0));
    assert!(dg.salience.iter().all(|&s| s == 0.0));
    let c = PersistenceConfig {
        metric: Metric::Cosine,
        ..cfg()
    };
    let dg = persistence_f64(&[0.0, 0.0, 0.0, 0.0, 1.0, 0.0], 2, None, &c).unwrap();
    assert_eq!(dg.finite.len(), 2);
    assert!(dg.finite.iter().all(|b| b.death.is_finite()));
    let bad = PersistenceConfig {
        essential_cap: Some(f64::NAN),
        ..cfg()
    };
    assert!(persistence_f64(&[0.0, 1.0], 1, None, &bad).is_err());
    let bad = PersistenceConfig { knn: 0, ..cfg() };
    assert!(persistence_f64(&[0.0, 1.0], 1, None, &bad).is_err());
}

#[test]
fn essential_cap_is_documented_and_overridable() {
    let dg = persistence_f64(&[0.0, 1.0, 3.0, 6.0], 1, None, &cfg()).unwrap();
    assert_eq!(
        dg.essential_death, 3.0,
        "default cap = largest finite death"
    );
    let c = PersistenceConfig {
        essential_cap: Some(10.0),
        ..cfg()
    };
    let dg = persistence_f64(&[0.0, 1.0, 3.0, 6.0], 1, None, &c).unwrap();
    assert_eq!(dg.essential_death, 10.0);
    assert_eq!(dg.salience[0], 10.0);
}

#[test]
fn knn_approximation_dominates_exact_and_is_exact_on_clusters() {
    let mut rng = SplitMix64::new(8);
    for _ in 0..10 {
        let n = 120;
        let a = cloud(&mut rng, n, 3);
        let exact = persistence_f64(&a, 3, None, &cfg()).unwrap();
        let approx_cfg = PersistenceConfig {
            dense_limit: 10,
            knn: 5,
            ..cfg()
        };
        let ap = persistence_f64(&a, 3, None, &approx_cfg).unwrap();
        assert!(ap.approximate && !exact.approximate);
        assert_eq!(ap.finite.len(), n - 1);
        let (de, da) = (sorted_deaths(&exact), sorted_deaths(&ap));
        for (e, x) in de.iter().zip(&da) {
            assert!(*x >= *e, "approximate death {x} below exact {e}");
        }
    }
    let a = cloud(&mut rng, 40, 2);
    let full = PersistenceConfig {
        dense_limit: 10,
        knn: 100,
        ..cfg()
    };
    let ap = persistence_f64(&a, 2, None, &full).unwrap();
    let ex = persistence_f64(&a, 2, None, &cfg()).unwrap();
    assert_eq!(sorted_deaths(&ap), sorted_deaths(&ex));
    let mut pts = Vec::new();
    for c in 0..4 {
        for _ in 0..30 {
            pts.push(c as f64 * 100.0 + rng.normal() * 0.1);
            pts.push(rng.normal() * 0.1);
        }
    }
    let small = PersistenceConfig {
        dense_limit: 10,
        knn: 6,
        ..cfg()
    };
    let ap = persistence_f64(&pts, 2, None, &small).unwrap();
    let ex = persistence_f64(&pts, 2, None, &cfg()).unwrap();
    let (dx, da) = (sorted_deaths(&ex), sorted_deaths(&ap));
    assert_eq!(&dx[dx.len() - 3..], &da[da.len() - 3..]);
}

#[test]
fn identical_across_threads() {
    let mut rng = SplitMix64::new(9);
    let a = cloud(&mut rng, 80, 4);
    let base = persistence_f64(&a, 4, None, &cfg()).unwrap();
    let hs: Vec<_> = (0..4)
        .map(|_| {
            let a = a.clone();
            std::thread::spawn(move || persistence_f64(&a, 4, None, &cfg()).unwrap())
        })
        .collect();
    for h in hs {
        assert_eq!(h.join().unwrap(), base);
    }
}
