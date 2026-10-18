use uriel_place::replay::SplitMix;
use uriel_place::{
    lower_bound, plan_arena, plan_first_fit, plan_offsets, ArenaError, Buffer, Plan,
};

fn buf(id: u32, size: u64, align: u64, first_use: u64, last_use: u64) -> Buffer {
    Buffer {
        id,
        size,
        align,
        first_use,
        last_use,
    }
}

fn unit(id: u32, size: u64, first_use: u64, last_use: u64) -> Buffer {
    buf(id, size, 1, first_use, last_use)
}

fn lives_overlap(a: &Buffer, b: &Buffer) -> bool {
    a.first_use <= b.last_use && b.first_use <= a.last_use
}

fn assert_valid(buffers: &[Buffer], plan: &Plan) {
    assert_eq!(plan.offsets.len(), buffers.len());
    let mut top = 0;
    for b in buffers {
        let off = plan.offset(b.id).expect("every buffer has an offset");
        assert_eq!(off % b.align, 0, "buffer {} misaligned", b.id);
        if b.size > 0 {
            top = top.max(off + b.size);
        }
    }
    assert_eq!(plan.peak, top);
    for (i, a) in buffers.iter().enumerate() {
        for b in &buffers[..i] {
            if a.size == 0 || b.size == 0 || !lives_overlap(a, b) {
                continue;
            }
            let (oa, ob) = (plan.offset(a.id).unwrap(), plan.offset(b.id).unwrap());
            assert!(
                oa + a.size <= ob || ob + b.size <= oa,
                "live buffers {} and {} share addresses",
                a.id,
                b.id
            );
        }
    }
}

fn random_instance(seed: u64) -> Vec<Buffer> {
    let mut rng = SplitMix(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ 0xa4e1);
    let n = 1 + rng.below(60) as u32;
    let horizon = 2 + rng.below(40);
    let aligns = [1u64, 2, 4, 8, 16, 64];
    (0..n)
        .map(|id| {
            let size = match rng.below(10) {
                0 => 0,
                1..=6 => 1 + rng.below(512),
                _ => 1 + rng.below(8192),
            };
            let first = rng.below(horizon);
            let span = 1 + rng.below(horizon);
            let last = first + rng.below(span);
            buf(id, size, aligns[rng.below(6) as usize], first, last)
        })
        .collect()
}

fn shuffled(buffers: &[Buffer], rng: &mut SplitMix) -> Vec<Buffer> {
    let mut v = buffers.to_vec();
    for i in (1..v.len()).rev() {
        v.swap(i, rng.below(i as u64 + 1) as usize);
    }
    v
}

#[test]
fn xnnpack_leading_gap_is_reused_when_free_for_the_whole_lifetime() {
    let bufs = [unit(0, 100, 0, 1), unit(1, 80, 1, 2), unit(2, 60, 2, 3)];
    let plan = plan_offsets(&bufs).unwrap();
    assert_eq!(plan.offsets, vec![(0, 0), (1, 100), (2, 0)]);
    assert_eq!(plan.peak, 180);
}

#[test]
fn xnnpack_smaller_internal_gap_beats_a_larger_leading_gap() {
    let bufs = [
        unit(0, 100, 0, 0),
        unit(1, 90, 0, 2),
        unit(2, 80, 0, 0),
        unit(3, 70, 0, 2),
        unit(4, 60, 2, 3),
    ];
    let plan = plan_offsets(&bufs).unwrap();
    assert_eq!(
        plan.offsets,
        vec![(0, 0), (1, 100), (2, 190), (3, 270), (4, 190)]
    );
    assert_eq!(plan.peak, 340);
}

#[test]
fn xnnpack_equal_fit_prefers_the_leading_gap() {
    let bufs = [
        unit(0, 110, 0, 0),
        unit(1, 100, 0, 2),
        unit(2, 60, 0, 0),
        unit(3, 50, 0, 0),
        unit(4, 45, 0, 2),
        unit(5, 40, 2, 3),
    ];
    let plan = plan_offsets(&bufs).unwrap();
    assert_eq!(
        plan.offsets,
        vec![(0, 0), (1, 110), (2, 210), (3, 270), (4, 320), (5, 0)]
    );
    assert_eq!(plan.peak, 365);
}

#[test]
fn pytorch_static_split_pool_reaches_max_live_of_512() {
    let bufs = [
        buf(0, 256, 64, 2, 4),
        buf(1, 256, 64, 5, 5),
        buf(2, 192, 64, 3, 5),
        buf(3, 256, 64, 1, 2),
    ];
    let plan = plan_offsets(&bufs).unwrap();
    assert_valid(&bufs, &plan);
    assert_eq!(lower_bound(&bufs), 512);
    assert_eq!(plan.peak, 512);
}

#[test]
fn tail_append_is_padded_up_to_the_alignment() {
    let bufs = [buf(0, 10, 1, 0, 5), buf(1, 10, 64, 0, 5)];
    let plan = plan_offsets(&bufs).unwrap();
    assert_eq!(plan.offset(0), Some(0));
    assert_eq!(plan.offset(1), Some(64));
    assert_eq!(plan.peak, 74);
}

#[test]
fn gap_that_is_too_small_after_alignment_padding_is_skipped() {
    let bufs = [
        buf(0, 10, 1, 0, 0),
        buf(1, 100, 1, 0, 1),
        buf(2, 20, 1, 0, 0),
        buf(3, 8, 1, 1, 1),
        buf(4, 64, 64, 0, 2),
    ];
    let plan = plan_offsets(&bufs).unwrap();
    assert_valid(&bufs, &plan);
    assert_eq!(plan.offset(4).unwrap() % 64, 0);
}

#[test]
fn aligned_offset_inside_a_gap_is_used_when_the_block_still_fits() {
    let bufs = [
        buf(0, 100, 1, 0, 1),
        buf(1, 10, 1, 0, 0),
        buf(2, 100, 1, 0, 1),
        buf(3, 20, 8, 1, 1),
    ];
    let plan = plan_offsets(&bufs).unwrap();
    assert_valid(&bufs, &plan);
    assert_eq!(plan.offset(3), Some(104));
    assert_eq!(plan.peak, 210);
}

#[test]
fn zero_size_buffers_get_offset_zero_and_never_block() {
    let bufs = [unit(0, 0, 0, 9), unit(1, 50, 0, 9), unit(2, 0, 3, 3)];
    let plan = plan_offsets(&bufs).unwrap();
    assert_eq!(plan.offsets, vec![(0, 0), (1, 0), (2, 0)]);
    assert_eq!(plan.peak, 50);
}

#[test]
fn disjoint_lifetimes_share_one_slot() {
    let bufs: Vec<Buffer> = (0..8).map(|i| unit(i, 100 + i as u64, i as u64 * 2, i as u64 * 2 + 1)).collect();
    let plan = plan_offsets(&bufs).unwrap();
    assert!(plan.offsets.iter().all(|&(_, o)| o == 0));
    assert_eq!(plan.peak, 107);
}

#[test]
fn lifetimes_sharing_one_tick_are_live_together() {
    let bufs = [unit(0, 10, 0, 1), unit(1, 10, 1, 2)];
    let plan = plan_offsets(&bufs).unwrap();
    assert_valid(&bufs, &plan);
    assert_eq!(plan.peak, 20);
}

#[test]
fn empty_input_gives_an_empty_plan() {
    let plan = plan_offsets(&[]).unwrap();
    assert!(plan.offsets.is_empty());
    assert_eq!(plan.peak, 0);
    assert_eq!(lower_bound(&[]), 0);
}

#[test]
fn duplicate_ids_are_rejected() {
    let bufs = [unit(7, 1, 0, 0), unit(7, 2, 1, 1)];
    assert_eq!(plan_offsets(&bufs), Err(ArenaError::DuplicateId(7)));
}

#[test]
fn zero_alignment_is_rejected() {
    let bufs = [buf(3, 8, 0, 0, 0)];
    assert_eq!(plan_offsets(&bufs), Err(ArenaError::BadAlignment(3)));
}

#[test]
fn non_power_of_two_alignment_is_rejected() {
    let bufs = [buf(4, 8, 12, 0, 0)];
    assert_eq!(plan_offsets(&bufs), Err(ArenaError::BadAlignment(4)));
}

#[test]
fn inverted_lifetime_is_rejected() {
    let bufs = [unit(5, 8, 4, 3)];
    assert_eq!(plan_offsets(&bufs), Err(ArenaError::InvertedLifetime(5)));
}

#[test]
fn offset_overflow_is_rejected_instead_of_wrapping() {
    let bufs = [unit(0, u64::MAX - 4, 0, 1), unit(1, 64, 0, 1)];
    assert_eq!(plan_offsets(&bufs), Err(ArenaError::Overflow));
}

#[test]
fn lower_bound_is_the_largest_sum_of_simultaneously_live_sizes() {
    let bufs = [unit(0, 5, 0, 2), unit(1, 7, 2, 4), unit(2, 3, 5, 6), unit(3, 11, 3, 3)];
    assert_eq!(lower_bound(&bufs), 5 + 7 + 0);
    let more = [bufs[0], bufs[1], bufs[3], unit(9, 1, 2, 3)];
    assert_eq!(lower_bound(&more), 5 + 7 + 1);
}

#[test]
fn plan_arena_returns_offsets_in_input_order() {
    let items = [(100u64, 0u64, 1u64), (80, 1, 2), (60, 2, 3)];
    let (offsets, peak) = plan_arena(&items, 1).unwrap();
    assert_eq!(offsets, vec![0, 100, 0]);
    assert_eq!(peak, 180);
}

#[test]
fn plan_arena_aligns_every_offset() {
    let items = [(10u64, 0u64, 5u64), (10, 0, 5), (10, 0, 5)];
    let (offsets, peak) = plan_arena(&items, 64).unwrap();
    assert!(offsets.iter().all(|o| o % 64 == 0));
    assert_eq!(peak, 138);
}

#[test]
fn no_two_live_buffers_share_addresses_on_4000_random_instances() {
    for seed in 0..4000 {
        let bufs = random_instance(seed);
        let plan = plan_offsets(&bufs).unwrap();
        assert_valid(&bufs, &plan);
    }
}

#[test]
fn first_fit_baseline_is_also_valid_on_4000_random_instances() {
    for seed in 0..4000 {
        let bufs = random_instance(seed);
        let plan = plan_first_fit(&bufs).unwrap();
        assert_valid(&bufs, &plan);
    }
}

#[test]
fn peak_never_goes_below_the_lower_bound_on_4000_random_instances() {
    for seed in 0..4000 {
        let bufs = random_instance(seed);
        let plan = plan_offsets(&bufs).unwrap();
        assert!(plan.peak >= lower_bound(&bufs), "seed {seed}");
    }
}

#[test]
fn shuffled_input_gives_identical_bytes_on_2000_random_instances() {
    for seed in 0..2000 {
        let bufs = random_instance(seed);
        let want = plan_offsets(&bufs).unwrap().to_bytes();
        let mut rng = SplitMix(seed ^ 0x5eed);
        for _ in 0..4 {
            let got = plan_offsets(&shuffled(&bufs, &mut rng)).unwrap().to_bytes();
            assert_eq!(got, want, "seed {seed}");
        }
    }
}

#[test]
fn identical_lifetimes_and_sizes_tie_break_by_id_not_input_order() {
    let bufs: Vec<Buffer> = (0..6).map(|i| unit(i, 32, 0, 3)).collect();
    let mut rev = bufs.clone();
    rev.reverse();
    assert_eq!(plan_offsets(&bufs).unwrap(), plan_offsets(&rev).unwrap());
    let plan = plan_offsets(&bufs).unwrap();
    assert_eq!(plan.offsets, (0..6).map(|i| (i, 32 * i as u64)).collect::<Vec<_>>());
}

const MIB_F32: u64 = 4;

fn mobilenet_like(seed: u64) -> Vec<Buffer> {
    let mut rng = SplitMix(seed ^ 0x30b1);
    let stages: [(u64, u64); 6] = [(112, 32), (112, 64), (56, 128), (28, 256), (14, 512), (7, 1024)];
    let mut out = Vec::new();
    let mut t = 0u64;
    let mut id = 0u32;
    out.push(buf(id, 224 * 224 * 3 * MIB_F32, 64, 0, 0));
    id += 1;
    for (hw, c) in stages {
        let reps = if hw == 14 { 5 } else { 2 };
        for _ in 0..reps {
            t += 1;
            let dw = hw * hw * c * MIB_F32;
            out.push(buf(id, dw, 64, t, t + 1));
            id += 1;
            t += 1;
            out.push(buf(id, hw * hw * c * 2 * MIB_F32 / 2, 64, t, t + 1));
            id += 1;
            if rng.below(3) == 0 {
                out.push(buf(id, 64 * (1 + rng.below(256)), 64, t, t));
                id += 1;
            }
        }
    }
    out
}

fn inception_like(seed: u64) -> Vec<Buffer> {
    let mut rng = SplitMix(seed ^ 0x1ce9);
    let mut out = Vec::new();
    let mut id = 0u32;
    let mut t = 0u64;
    let blocks = [(35u64, 288u64), (35, 288), (17, 768), (17, 768), (17, 768), (8, 1280), (8, 2048)];
    let mut input = buf(id, 35 * 35 * 192 * 4, 64, t, t);
    id += 1;
    for (hw, c) in blocks {
        let area = hw * hw;
        let mut branch_last = Vec::new();
        let mut last_read_of_input = t;
        for _ in 0..4 {
            let depth = 1 + rng.below(3);
            t += 1;
            last_read_of_input = t;
            let mut prev: Option<Buffer> = None;
            for d in 0..depth {
                if d > 0 {
                    t += 1;
                }
                if let Some(mut p) = prev.take() {
                    p.last_use = t;
                    out.push(p);
                }
                let ch = 32 * (1 + rng.below(c / 32 / 3));
                prev = Some(buf(id, area * ch * 4, 64, t, t));
                id += 1;
            }
            branch_last.push(prev.unwrap());
        }
        t += 1;
        for mut b in branch_last {
            b.last_use = t;
            out.push(b);
        }
        input.last_use = last_read_of_input;
        out.push(input);
        input = buf(id, area * c * 4, 64, t, t);
        id += 1;
    }
    input.last_use = t + 1;
    out.push(input);
    out
}

struct Stats {
    name: &'static str,
    n: usize,
    sum_planner: u64,
    sum_first_fit: u64,
    sum_lower: u64,
    ratio_planner: Vec<f64>,
    ratio_first_fit: Vec<f64>,
    planner_worse: usize,
    planner_better: usize,
}

impl Stats {
    fn new(name: &'static str) -> Self {
        Stats {
            name,
            n: 0,
            sum_planner: 0,
            sum_first_fit: 0,
            sum_lower: 0,
            ratio_planner: Vec::new(),
            ratio_first_fit: Vec::new(),
            planner_worse: 0,
            planner_better: 0,
        }
    }

    fn add(&mut self, bufs: &[Buffer]) {
        let lb = lower_bound(bufs);
        let p = plan_offsets(bufs).unwrap();
        let f = plan_first_fit(bufs).unwrap();
        assert_valid(bufs, &p);
        assert_valid(bufs, &f);
        assert!(p.peak >= lb && f.peak >= lb);
        self.n += 1;
        self.sum_planner += p.peak;
        self.sum_first_fit += f.peak;
        self.sum_lower += lb;
        if lb > 0 {
            self.ratio_planner.push(p.peak as f64 / lb as f64);
            self.ratio_first_fit.push(f.peak as f64 / lb as f64);
        }
        if p.peak > f.peak {
            self.planner_worse += 1;
        }
        if p.peak < f.peak {
            self.planner_better += 1;
        }
    }

    fn print(&self) {
        let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
        let worst = |v: &[f64]| v.iter().cloned().fold(0.0, f64::max);
        println!(
            "{}: instances={} sum_peak planner={} first_fit={} lower_bound={} | peak/LB planner mean={:.6} worst={:.6} | first_fit mean={:.6} worst={:.6} | planner_better={} planner_worse={} equal={}",
            self.name,
            self.n,
            self.sum_planner,
            self.sum_first_fit,
            self.sum_lower,
            mean(&self.ratio_planner),
            worst(&self.ratio_planner),
            mean(&self.ratio_first_fit),
            worst(&self.ratio_first_fit),
            self.planner_better,
            self.planner_worse,
            self.n - self.planner_better - self.planner_worse,
        );
    }
}

#[test]
fn baseline_comparison_prints_ratios_and_planner_is_never_worse_than_first_fit() {
    let mut random = Stats::new("random");
    for seed in 0..4000 {
        random.add(&random_instance(seed));
    }
    let mut mobile = Stats::new("mobilenet_like");
    for seed in 0..200 {
        mobile.add(&mobilenet_like(seed));
    }
    let mut incep = Stats::new("inception_like");
    for seed in 0..200 {
        incep.add(&inception_like(seed));
    }
    for s in [&random, &mobile, &incep] {
        s.print();
    }
    for s in [&random, &mobile, &incep] {
        assert_eq!(s.planner_worse, 0, "{}", s.name);
    }
}
