use uriel_algo::certify::{
    certified_argmin, certified_threshold, certified_topk, enclose_scores, eta, gamma, topk_set,
    unit_roundoff, Kernel, Precision, Refusal, Trit,
};

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn int(&mut self, lo: i64, hi: i64) -> i64 {
        lo + (self.next_u64() % (hi - lo) as u64) as i64
    }

    fn normal(&mut self) -> f64 {
        let u1 = self.unit().max(1e-12);
        let u2 = self.unit();
        (-2.0 * u1.ln()).sqrt() * (core::f64::consts::TAU * u2).cos()
    }

    fn permutation(&mut self, n: usize) -> Vec<usize> {
        let mut p: Vec<usize> = (0..n).collect();
        for i in (1..n).rev() {
            let j = (self.next_u64() % (i as u64 + 1)) as usize;
            p.swap(i, j);
        }
        p
    }
}

const DIM: usize = 8;
const LATTICE: i32 = 22;

const CMP: i32 = 100;

fn to_f32<const N: usize>(m: &[i64; N]) -> [f32; N] {
    m.map(|v| v as f32 * 2f32.powi(-LATTICE))
}

fn point<const N: usize>(rng: &mut Rng, half_range: i64) -> [i64; N] {
    core::array::from_fn(|_| rng.int(-half_range, half_range))
}

fn exact_sq<const N: usize>(q: &[i64; N], x: &[i64; N]) -> i128 {
    q.iter()
        .zip(x)
        .map(|(&a, &b)| ((a - b) as i128).pow(2))
        .sum()
}

fn exact_scaled(v: f64) -> i128 {
    let s = v * 2f64.powi(CMP);
    assert!(
        s.fract() == 0.0 && s.abs() < 2f64.powi(120),
        "{v:e} is off the 2^-100 grid"
    );
    s as i128
}

fn encloses(d: f64, r: f64, units: i128) -> bool {
    let (d, r) = (exact_scaled(d), (r * 2f64.powi(CMP)).floor() as i128);
    let s = units << (CMP - 2 * LATTICE);
    d - r <= s && s <= d + r
}

fn exact_topk(exact: &[i128], k: usize) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..exact.len()).collect();
    idx.sort_by_key(|&i| (exact[i], i));
    let mut set = idx[..k].to_vec();
    set.sort_unstable();
    set
}

fn strictly_separated(exact: &[i128], set: &[usize]) -> bool {
    let max_in = set.iter().map(|&i| exact[i]).max().unwrap();
    (0..exact.len())
        .filter(|i| !set.contains(i))
        .all(|j| exact[j] > max_in)
}

fn sorted(mut v: Vec<usize>) -> Vec<usize> {
    v.sort_unstable();
    v
}

fn near_tie_corpus(rng: &mut Rng, n: usize, cluster: usize) -> (Vec<[i64; DIM]>, [i64; DIM]) {
    let mut corpus: Vec<[i64; DIM]> = (0..n).map(|_| point(rng, 1 << 23)).collect();
    let c: [i64; DIM] = point(rng, (1 << 23) - 64);
    for &i in rng.permutation(n).iter().take(cluster) {
        corpus[i] = core::array::from_fn(|l| c[l] + rng.int(-16, 17));
    }
    let query = core::array::from_fn(|l| c[l] + rng.int(-16, 17));
    (corpus, query)
}

fn floats(corpus: &[[i64; DIM]]) -> Vec<[f32; DIM]> {
    corpus.iter().map(to_f32).collect()
}

#[test]
fn unit_roundoff_and_gamma_are_the_textbook_values_and_refuse_where_vacuous() {
    assert_eq!(unit_roundoff(Precision::F16), 2f64.powi(-11));
    assert_eq!(unit_roundoff(Precision::F32), 2f64.powi(-24));
    assert_eq!(unit_roundoff(Precision::F64), 2f64.powi(-53));

    for (n, p, want) in [
        (386, Precision::F32, 2.300792e-05),
        (786, Precision::F32, 4.685145e-05),
        (386, Precision::F16, 2.322503e-01),
        (786, Precision::F16, 6.228209e-01),
    ] {
        let nu = n as f64 * unit_roundoff(p);
        let by_hand = nu / (1.0 - nu);
        let got = gamma(n, p).unwrap();
        assert!(
            got >= by_hand && got - by_hand <= by_hand * 1e-15,
            "{n} {p:?}"
        );
        assert!((got - want).abs() <= want * 1e-6, "{n} {p:?}: {got:e}");
    }

    assert_eq!(
        gamma(2050, Precision::F16),
        Err(Refusal::BoundVacuous { n: 2050 })
    );
    assert!(gamma(1024, Precision::F16).unwrap() > 0.0);
    let n32 = (1 << 23) + 1;
    assert_eq!(
        gamma(n32, Precision::F32),
        Err(Refusal::BoundVacuous { n: n32 })
    );
    assert!(matches!(gamma(0, Precision::F32), Err(Refusal::Usage(_))));

    assert_eq!(eta(8, Precision::F32), 40.0 * 2f64.powi(-149));
}

#[test]
fn gram_enclosure_contains_the_exact_score_and_cheap_dominates_tight() {
    let mut pairs = 0;
    for seed in 0..60u64 {
        let mut rng = Rng::new(0x6A0 + seed);
        let (corpus_m, query_m) = near_tie_corpus(&mut rng, 32, 6);
        let (corpus, query) = (floats(&corpus_m), to_f32(&query_m));
        let cheap = enclose_scores(&corpus, &query, Kernel::GramCheap).unwrap();
        let tight = enclose_scores(&corpus, &query, Kernel::GramTight).unwrap();
        assert_eq!(
            cheap.scores, tight.scores,
            "the bound must not change the score"
        );
        for (j, x) in corpus_m.iter().enumerate() {
            let s = exact_sq(&query_m, x);
            assert!(
                encloses(cheap.scores[j], cheap.radii[j], s),
                "seed {seed} row {j} cheap"
            );
            assert!(
                encloses(tight.scores[j], tight.radii[j], s),
                "seed {seed} row {j} tight"
            );
            assert!(
                cheap.radii[j] >= tight.radii[j],
                "seed {seed} row {j}: ladder inverted"
            );
            pairs += 1;
        }
    }
    assert_eq!(pairs, 60 * 32);
}

#[test]
fn direct_radius_counts_the_rounded_difference_twice() {
    fn run<const N: usize>(seed: u64) -> (usize, usize) {
        let mut rng = Rng::new(seed);
        let g1 = gamma(N + 1, Precision::F32).unwrap();
        let (mut escapes, mut control_escapes) = (0, 0);
        for _ in 0..40_000 {
            let (q, x): ([i64; N], [i64; N]) = (point(&mut rng, 1 << 24), point(&mut rng, 1 << 24));
            let e = enclose_scores(&[to_f32(&x)], &to_f32(&q), Kernel::Direct).unwrap();
            let (d, r, s) = (e.scores[0], e.radii[0], exact_sq(&q, &x));
            escapes += usize::from(!encloses(d, r, s));
            control_escapes += usize::from(!encloses(d, d * g1 / (1.0 - g1) * (1.0 + 1e-12), s));
        }
        (escapes, control_escapes)
    }
    for (escapes, control) in [run::<1>(0xD1), run::<2>(0xD2)] {
        assert_eq!(escapes, 0, "the gamma_(d+2) radius was escaped");
        assert!(
            control > 0,
            "the gamma_(d+1) control was never escaped; the test has no teeth"
        );
    }
}

#[test]
fn no_certified_topk_contradicts_exact_arithmetic_on_adversarial_near_ties() {
    let (mut rounding_chose, mut certified, mut refused) = (0, 0, 0);
    for seed in 0..300u64 {
        let mut rng = Rng::new(0xC0FFEE ^ (seed << 8));
        let k = if seed % 2 == 0 { 1 } else { 3 };
        let cluster = if seed % 3 == 0 { 1 } else { k + 1 };
        let (corpus_m, query_m) = near_tie_corpus(&mut rng, 40, cluster);
        let (corpus, query) = (floats(&corpus_m), to_f32(&query_m));
        let exact: Vec<i128> = corpus_m.iter().map(|x| exact_sq(&query_m, x)).collect();
        let truth = exact_topk(&exact, k);

        for kernel in [Kernel::GramCheap, Kernel::GramTight, Kernel::Direct] {
            let e = enclose_scores(&corpus, &query, kernel).unwrap();
            let differs = sorted(topk_set(&e.scores, k, false)) != truth;
            if kernel == Kernel::GramCheap {
                rounding_chose += usize::from(differs);
            }
            match certified_topk(&e.scores, &e.radii, k, false, false) {
                Ok(set) => {
                    assert!(
                        !differs,
                        "seed {seed} {kernel:?}: certified a set rounding chose"
                    );
                    assert_eq!(set, truth, "seed {seed} {kernel:?}");
                    assert!(strictly_separated(&exact, &set), "seed {seed} {kernel:?}");
                    if k == 1 {
                        assert_eq!(certified_argmin(&e.scores, &e.radii), Ok(truth[0]));
                    }
                    certified += 1;
                }
                Err(Refusal::BoundaryUndetermined { .. }) => refused += 1,
                Err(other) => panic!("seed {seed} {kernel:?}: unexpected {other:?}"),
            }
        }
    }
    assert!(
        rounding_chose >= 50,
        "binary32 rounding chose only {rounding_chose} rankings"
    );
    assert!(
        certified >= 300,
        "only {certified} certified; nothing was at stake"
    );
    assert!(refused >= 150, "only {refused} refused");
}

#[test]
fn a_threshold_trit_never_places_a_score_on_the_wrong_side() {
    assert_eq!(
        certified_threshold(&[1.0, 5.0, 3.0], &[0.1, 0.1, 2.0], 3.0),
        Ok(vec![Trit::Below, Trit::Above, Trit::Undetermined])
    );

    let mut seen = [0usize; 3];
    for seed in 0..80u64 {
        let mut rng = Rng::new(0x7A1 + seed);
        let (corpus_m, query_m) = near_tie_corpus(&mut rng, 32, 5);
        let e = enclose_scores(&floats(&corpus_m), &to_f32(&query_m), Kernel::GramCheap).unwrap();
        let exact: Vec<i128> = corpus_m.iter().map(|x| exact_sq(&query_m, x)).collect();
        let pivot = rng.int(0, 32) as usize;
        let t = exact[pivot] as f64 * 2f64.powi(-2 * LATTICE);
        let trits = certified_threshold(&e.scores, &e.radii, t).unwrap();
        assert_eq!(
            trits[pivot],
            Trit::Undetermined,
            "seed {seed}: the pair on t was decided"
        );
        for (j, trit) in trits.iter().enumerate() {
            match trit {
                Trit::Above => assert!(exact[j] > exact[pivot], "seed {seed} row {j}"),
                Trit::Below => assert!(exact[j] < exact[pivot], "seed {seed} row {j}"),
                Trit::Undetermined => {}
            }
            seen[(*trit as i8 + 1) as usize] += 1;
        }
    }
    assert!(
        seen.iter().all(|&c| c > 0),
        "every side must occur: {seen:?}"
    );
}

#[test]
fn well_separated_data_certifies_and_the_certificate_is_the_exact_set() {
    for seed in 0..200u64 {
        let mut rng = Rng::new(0x5E9 + seed);
        let corpus_m: Vec<[i64; DIM]> = (0..32).map(|_| point(&mut rng, 1 << 23)).collect();
        let query_m: [i64; DIM] = point(&mut rng, 1 << 23);
        let exact: Vec<i128> = corpus_m.iter().map(|x| exact_sq(&query_m, x)).collect();
        for kernel in [Kernel::GramCheap, Kernel::Direct] {
            let e = enclose_scores(&floats(&corpus_m), &to_f32(&query_m), kernel).unwrap();
            for k in [1, 5] {
                assert_eq!(
                    certified_topk(&e.scores, &e.radii, k, false, false),
                    Ok(exact_topk(&exact, k)),
                    "seed {seed} {kernel:?} k {k}"
                );
            }
        }
    }
}

#[test]
fn the_direct_kernel_decides_what_the_gram_identity_cannot() {
    let p: [[f64; 2]; 3] = [[1e6, 0.0], [1e6 + 1e-6, 0.0], [0.0, 0.0]];
    let gram = enclose_scores(&p, &p[0], Kernel::GramCheap).unwrap();
    assert_eq!((gram.scores[0], gram.scores[1]), (0.0, 0.0));
    match certified_argmin(&gram.scores, &gram.radii) {
        Err(Refusal::BoundaryUndetermined { frontier, .. }) => {
            assert_eq!((frontier.inside, frontier.outside), (0, 1));
        }
        other => panic!("the Gram identity certified a cancelled pair: {other:?}"),
    }
    let direct = enclose_scores(&p, &p[0], Kernel::Direct).unwrap();
    assert!(direct.scores[1] > 0.0);
    assert_eq!(certified_argmin(&direct.scores, &direct.radii), Ok(0));
}

#[test]
fn the_boundary_pair_rule_is_a_false_theorem_and_the_refusal_names_the_blocker() {
    let (d, r) = ([0.0, 1.0, 2.0, 10.0], [12.0, 0.0, 0.0, 0.0]);

    let order = topk_set(&d, 4, false);
    let (a, b) = (order[1], order[2]);
    assert!(d[a] + r[a] < d[b] - r[b]);
    let witness = [11.0, 1.0, 2.0, 10.0];
    assert!(witness
        .iter()
        .zip(&d)
        .zip(&r)
        .all(|((w, d), r)| (w - d).abs() <= *r));
    assert_eq!(sorted(topk_set(&witness, 2, false)), vec![1, 2]);

    match certified_topk(&d, &r, 2, false, false) {
        Err(Refusal::BoundaryUndetermined {
            frontier,
            straddling,
        }) => {
            assert_eq!((frontier.inside, frontier.outside), (0, 2));
            assert!(frontier.inside_lo <= -12.0 && frontier.inside_hi >= 12.0);
            assert_eq!((frontier.gap, frontier.width), (2.0, 12.0));
            assert!(frontier.deficit() <= 0.0);
            assert_eq!(straddling, vec![0, 2, 3]);
        }
        other => panic!("the false theorem was certified: {other:?}"),
    }
}

#[test]
fn argmin_is_decided_against_every_lower_endpoint_not_the_runner_up() {
    let d = [0.0, 1.0, 3.0];
    match certified_argmin(&d, &[0.1, 0.1, 5.0]) {
        Err(Refusal::BoundaryUndetermined {
            frontier,
            straddling,
        }) => {
            assert_eq!((frontier.inside, frontier.outside), (0, 2));
            assert_eq!(straddling, vec![0, 2]);
        }
        other => panic!("a runner-up comparison was certified: {other:?}"),
    }
    assert_eq!(certified_argmin(&d, &[0.1]), Ok(0));
    assert_eq!(
        certified_topk(&d, &[0.1], 1, true, false),
        Ok(vec![2]),
        "argmax"
    );
}

#[test]
fn a_certified_set_survives_the_worst_corner_of_the_box() {
    let mut rng = Rng::new(0xB0C5);
    let mut certified = 0;
    for _ in 0..5000 {
        let n = rng.int(4, 20) as usize;
        let k = rng.int(1, n as i64 - 1) as usize;
        let scale = [1.0, 1e-6, 1e6][rng.int(0, 3) as usize];
        let d: Vec<f64> = (0..n).map(|_| rng.normal() * scale).collect();
        let peak = d.iter().fold(0.0f64, |m, v| m.max(v.abs()));
        let r: Vec<f64> = (0..n).map(|_| rng.unit().powi(2) * peak * 0.3).collect();
        let largest = rng.int(0, 2) == 1;
        let Ok(set) = certified_topk(&d, &r, k, largest, false) else {
            continue;
        };
        let toward = if largest { -1.0 } else { 1.0 };
        let corner: Vec<f64> = (0..n)
            .map(|i| {
                let push = if set.contains(&i) { toward } else { -toward };
                d[i] + push * r[i]
            })
            .collect();
        assert_eq!(sorted(topk_set(&corner, k, largest)), set);
        certified += 1;
    }
    assert!(certified > 500, "only {certified} certified");
}

#[test]
fn order_is_certified_only_when_asked() {
    let (d, r) = ([0.0, 0.01, 5.0], [0.05]);
    assert_eq!(certified_topk(&d, &r, 2, false, false), Ok(vec![0, 1]));
    match certified_topk(&d, &r, 2, false, true) {
        Err(Refusal::BoundaryUndetermined {
            frontier,
            straddling,
        }) => {
            assert_eq!((frontier.inside, frontier.outside), (0, 1));
            assert_eq!(straddling, vec![0, 1]);
        }
        other => panic!("an undetermined order was certified: {other:?}"),
    }
    let d = [3.0, 1.0, 2.0, 9.0];
    assert_eq!(
        certified_topk(&d, &[0.1], 3, false, false),
        Ok(vec![0, 1, 2])
    );
    assert_eq!(
        certified_topk(&d, &[0.1], 3, false, true),
        Ok(vec![1, 2, 0])
    );
}

#[test]
fn touching_enclosures_do_not_certify_and_a_zero_radius_certifies_the_floats() {
    assert!(matches!(
        certified_topk(&[0.0, 1.0], &[0.5, 0.5], 1, false, false),
        Err(Refusal::BoundaryUndetermined { .. })
    ));
    assert_eq!(
        certified_topk(&[0.0, 1.0], &[0.4995], 1, false, false),
        Ok(vec![0])
    );
    assert_eq!(
        certified_topk(&[3.0, 1.0, 2.0, 9.0], &[0.0], 2, false, false),
        Ok(vec![1, 2])
    );
}

#[test]
fn the_straddling_set_holds_every_index_whose_membership_can_move() {
    let mut rng = Rng::new(0x5AD);
    let mut refusals = 0;
    for _ in 0..1500 {
        let n = rng.int(4, 16) as usize;
        let k = rng.int(1, n as i64 - 1) as usize;
        let d: Vec<f64> = (0..n).map(|_| rng.normal()).collect();
        let r: Vec<f64> = (0..n).map(|_| rng.unit().powi(3)).collect();
        let largest = rng.int(0, 2) == 1;
        let Err(Refusal::BoundaryUndetermined {
            frontier,
            straddling,
        }) = certified_topk(&d, &r, k, largest, false)
        else {
            continue;
        };
        assert!(straddling.contains(&frontier.inside) && straddling.contains(&frontier.outside));
        let float_set = sorted(topk_set(&d, k, largest));
        for trial in 0..200 {
            let v: Vec<f64> = (0..n)
                .map(|i| match trial {
                    0 => d[i] + r[i],
                    1 => d[i] - r[i],
                    _ => d[i] + r[i] * (2.0 * rng.unit() - 1.0),
                })
                .collect();
            let moved = sorted(topk_set(&v, k, largest));
            for i in (0..n).filter(|i| !straddling.contains(i)) {
                assert_eq!(
                    float_set.contains(&i),
                    moved.contains(&i),
                    "index {i} moved unnamed"
                );
            }
        }
        refusals += 1;
    }
    assert!(refusals > 300, "only {refusals} refusals");
}

#[test]
fn certified_results_are_invariant_under_corpus_permutation() {
    let mut certified = 0;
    for seed in 0..120u64 {
        let mut rng = Rng::new(0x9E3 + seed);
        let k = [1, 2, 4][(seed % 3) as usize];
        let (corpus_m, query_m) =
            near_tie_corpus(&mut rng, 36, if seed % 2 == 0 { 1 } else { k + 1 });
        let perm = rng.permutation(36);
        let corpus = floats(&corpus_m);
        let shuffled: Vec<[f32; DIM]> = perm.iter().map(|&i| corpus[i]).collect();
        let query = to_f32(&query_m);
        for kernel in [Kernel::GramCheap, Kernel::Direct] {
            let a = enclose_scores(&corpus, &query, kernel).unwrap();
            let b = enclose_scores(&shuffled, &query, kernel).unwrap();
            for (i, &p) in perm.iter().enumerate() {
                assert_eq!((b.scores[i], b.radii[i]), (a.scores[p], a.radii[p]));
            }
            match (
                certified_topk(&a.scores, &a.radii, k, false, false),
                certified_topk(&b.scores, &b.radii, k, false, false),
            ) {
                (Ok(x), Ok(y)) => {
                    assert_eq!(
                        sorted(y.iter().map(|&i| perm[i]).collect()),
                        x,
                        "seed {seed}"
                    );
                    certified += 1;
                }
                (Err(_), Err(_)) => {}
                (x, y) => panic!("seed {seed} {kernel:?}: {x:?} vs permuted {y:?}"),
            }
        }
    }
    assert!(certified > 60, "only {certified} certified");
}

#[test]
fn largest_k_is_smallest_k_of_the_negated_scores() {
    let mut rng = Rng::new(0x1A9);
    let mut checked = 0;
    for _ in 0..4000 {
        let n = rng.int(4, 14) as usize;
        let k = rng.int(1, n as i64 - 1) as usize;
        let d: Vec<f64> = (0..n).map(|_| rng.normal()).collect();
        let neg: Vec<f64> = d.iter().map(|v| -v).collect();
        let r: Vec<f64> = (0..n).map(|_| rng.unit().powi(3)).collect();
        match (
            certified_topk(&d, &r, k, true, false),
            certified_topk(&neg, &r, k, false, false),
        ) {
            (Ok(x), Ok(y)) => assert_eq!(x, y),
            (
                Err(Refusal::BoundaryUndetermined {
                    frontier: f,
                    straddling: s,
                }),
                Err(Refusal::BoundaryUndetermined {
                    frontier: g,
                    straddling: t,
                }),
            ) => {
                assert_eq!(
                    (f.inside, f.outside, f.gap, f.width),
                    (g.inside, g.outside, g.gap, g.width)
                );
                assert_eq!(s, t);
                assert!(f.inside_lo <= d[f.inside] && d[f.inside] <= f.inside_hi);
                assert!(f.outside_lo <= d[f.outside] && d[f.outside] <= f.outside_hi);
            }
            (x, y) => panic!("largest {x:?} vs negated {y:?}"),
        }
        checked += 1;
    }
    assert_eq!(checked, 4000);
}

#[test]
fn nonfinite_negative_and_empty_inputs_refuse_and_say_where() {
    fn nonfinite<T>(operand: &'static str, index: usize) -> Result<T, Refusal> {
        Err(Refusal::NonFiniteInput { operand, index })
    }
    assert_eq!(
        certified_argmin(&[0.0, f64::NAN, 2.0], &[0.1]),
        nonfinite("scores", 1)
    );
    assert_eq!(
        certified_argmin(&[0.0, 1.0], &[0.1, f64::INFINITY]),
        nonfinite("radii", 1)
    );
    assert_eq!(
        certified_threshold(&[0.0, f64::NEG_INFINITY], &[0.1], 1.0),
        nonfinite("scores", 1)
    );
    assert_eq!(
        certified_threshold(&[0.0], &[0.1], f64::NAN),
        nonfinite("threshold", 0)
    );

    assert!(matches!(
        certified_argmin(&[0.0, 1.0], &[-0.1]),
        Err(Refusal::Usage(_))
    ));
    assert!(matches!(
        certified_threshold(&[0.0], &[-0.1], 1.0),
        Err(Refusal::Usage(_))
    ));
    assert!(matches!(certified_argmin(&[], &[]), Err(Refusal::Usage(_))));
    assert!(matches!(
        certified_topk(&[0.0, 1.0], &[0.1], 2, false, false),
        Err(Refusal::Usage(_))
    ));
    assert!(matches!(
        certified_topk(&[0.0, 1.0], &[0.1], 0, false, false),
        Err(Refusal::Usage(_))
    ));
    assert!(matches!(
        certified_argmin(&[0.0, 1.0, 2.0], &[0.1, 0.1]),
        Err(Refusal::Usage(_))
    ));
    assert_eq!(certified_threshold(&[], &[], 1.0), Ok(vec![]));

    let mut corpus = [[1.0f32; 4]; 5];
    corpus[3][2] = f32::NAN;
    let query = [0.5f32; 4];
    assert_eq!(
        enclose_scores(&corpus, &query, Kernel::GramCheap),
        nonfinite("corpus", 3)
    );
    let corpus = [[1.0f32; 4]; 5];
    let query = [0.5, f32::INFINITY, 0.5, 0.5];
    assert_eq!(
        enclose_scores(&corpus, &query, Kernel::Direct),
        nonfinite("query", 1)
    );
}

#[test]
fn a_range_unsafe_corpus_refuses_before_any_score_is_read() {
    let x = [1e19f32, 1e19, 0.0, 0.0];
    for kernel in [Kernel::GramCheap, Kernel::GramTight, Kernel::Direct] {
        match enclose_scores(&[x, [0.0; 4]], &x, kernel) {
            Err(Refusal::RangeUnsafe { headroom, limit }) => {
                assert_eq!(limit, f32::MAX as f64);
                assert!(headroom > limit);
            }
            other => panic!("{kernel:?}: {other:?}"),
        }
    }
    let n2 = x.iter().fold(0.0f32, |s, &a| s + a * a);
    let dot = n2;
    assert!(!((n2 + n2) - (dot + dot)).is_finite());
    let y = x.map(|v| v * 1e-3);
    assert!(enclose_scores(&[y, [0.0; 4]], &y, Kernel::GramCheap).is_ok());
}

#[test]
fn endpoints_are_rounded_outward_so_enclosures_within_two_ulps_do_not_certify() {
    let two_ulps = 2.0f64.next_up();
    assert!(matches!(
        certified_topk(&[0.0, two_ulps], &[1.0], 1, false, false),
        Err(Refusal::BoundaryUndetermined { .. })
    ));
    assert!(matches!(
        certified_topk(&[two_ulps, 0.0], &[1.0], 1, true, false),
        Err(Refusal::BoundaryUndetermined { .. })
    ));
    assert_eq!(
        certified_topk(&[0.0, two_ulps.next_up()], &[1.0], 1, false, false),
        Ok(vec![0])
    );
}

#[test]
fn threshold_endpoints_are_rounded_outward() {
    assert_eq!(
        certified_threshold(&[0.0], &[1.0], 1.0f64.next_up()),
        Ok(vec![Trit::Undetermined])
    );
    assert_eq!(
        certified_threshold(&[0.0], &[1.0], (-1.0f64).next_down()),
        Ok(vec![Trit::Undetermined])
    );
    assert_eq!(
        certified_threshold(&[0.0], &[1.0], 1.0f64.next_up().next_up()),
        Ok(vec![Trit::Below])
    );
}

#[test]
fn a_zero_distance_radius_is_the_underflow_floor_rounded_upward() {
    let x = [[1.0f32, 2.0, 3.0, 4.0]];
    let e = enclose_scores(&x, &x[0], Kernel::Direct).unwrap();
    assert_eq!(e.scores, vec![0.0]);
    assert_eq!(e.radii, vec![eta(4, Precision::F32).next_up()]);
}

fn plain_topk(keys: &[i128], k: usize) -> Vec<usize> {
    exact_topk(keys, k)
}

fn corner_separates(keys: &[i128], rad: &[i128], set: &[usize]) -> bool {
    let max_in = set.iter().map(|&i| keys[i] + rad[i]).max().unwrap();
    (0..keys.len())
        .filter(|i| !set.contains(i))
        .all(|j| keys[j] - rad[j] > max_in)
}

#[test]
fn certified_topk_equals_the_plain_exact_topk_on_random_integer_score_sets() {
    let mut rng = Rng::new(0x1E7);
    let (mut certified, mut refused) = (0, 0);
    for case in 0..2000 {
        let n = rng.int(3, 17) as usize;
        let k = rng.int(1, n as i64) as usize;
        let largest = rng.int(0, 2) == 1;
        let values: Vec<i64> = (0..n).map(|_| rng.int(0, 8)).collect();
        let quarter: Vec<i64> = (0..n).map(|_| rng.int(0, 2)).collect();
        let scores: Vec<f64> = values.iter().map(|&v| v as f64).collect();
        let radii: Vec<f64> = quarter.iter().map(|&q| q as f64 * 0.25).collect();
        let sgn = if largest { -1 } else { 1 };
        let keys: Vec<i128> = values.iter().map(|&v| (sgn * 4 * v) as i128).collect();
        let rad: Vec<i128> = quarter.iter().map(|&q| q as i128).collect();
        let truth = plain_topk(&keys, k);
        let decidable = {
            let mut order: Vec<usize> = (0..n).collect();
            order.sort_by_key(|&i| (keys[i], i));
            corner_separates(&keys, &rad, &order[..k])
        };
        match certified_topk(&scores, &radii, k, largest, false) {
            Ok(set) => {
                assert!(decidable, "case {case}: certified an undecidable boundary");
                assert_eq!(set, truth, "case {case}");
                certified += 1;
            }
            Err(Refusal::BoundaryUndetermined { .. }) => {
                assert!(!decidable, "case {case}: refused a decidable boundary");
                refused += 1;
            }
            Err(other) => panic!("case {case}: unexpected {other:?}"),
        }
    }
    assert!(certified >= 300, "only {certified} certified");
    assert!(refused >= 300, "only {refused} refused");
}

#[test]
fn certified_topk_never_returns_a_wrong_set_on_scores_one_ulp_apart() {
    let mut rng = Rng::new(0xADE);
    let (mut certified, mut refused) = (0, 0);
    for case in 0..2000 {
        let n = rng.int(3, 9) as usize;
        let k = rng.int(1, n as i64) as usize;
        let largest = rng.int(0, 2) == 1;
        let base = (1.0 + rng.unit() * 0.25) * 2f64.powi(rng.int(-30, 30) as i32);
        let ulp = base.next_up() - base;
        let steps: Vec<i64> = (0..n).map(|_| rng.int(0, 14)).collect();
        let scores: Vec<f64> = steps
            .iter()
            .map(|&j| (0..j).fold(base, |x, _| x.next_up()))
            .collect();
        let half = rng.int(0, 2) == 1;
        let radii = [if half { ulp / 2.0 } else { 0.0 }];
        let sgn = if largest { -1 } else { 1 };
        let keys: Vec<i128> = steps.iter().map(|&j| (sgn * 2 * j) as i128).collect();
        let rad = vec![i128::from(half); n];
        match certified_topk(&scores, &radii, k, largest, false) {
            Ok(set) => {
                assert!(
                    corner_separates(&keys, &rad, &set),
                    "case {case}: certified a set the box does not decide"
                );
                assert_eq!(set, plain_topk(&keys, k), "case {case}");
                certified += 1;
            }
            Err(Refusal::BoundaryUndetermined { .. }) => refused += 1,
            Err(other) => panic!("case {case}: unexpected {other:?}"),
        }
        let mut order: Vec<usize> = (0..n).collect();
        order.sort_by_key(|&i| (keys[i], i));
        if keys[order[k - 1]] == keys[order[k.min(n - 1)]] && k < n {
            assert!(
                certified_topk(&scores, &radii, k, largest, false).is_err(),
                "case {case}: certified through an exact tie"
            );
        }
    }
    assert!(certified >= 400, "only {certified} certified");
    assert!(refused >= 1000, "only {refused} refused");
}
