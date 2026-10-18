use crate::{Graph, MemoryBudget, Options};

const MIB: usize = 1024 * 1024;

fn parts(b: MemoryBudget) -> [usize; 7] {
    [
        b.reserve(),
        b.redb_cache(),
        b.hot(),
        b.warm(),
        b.query_cache(),
        b.rag_index(),
        b.session(),
    ]
}

#[test]
fn parts_never_sum_past_the_total_and_lose_only_rounding() {
    for total in [16 * MIB, 24 * MIB, 50 * MIB, 64 * MIB, 80 * MIB, 256 * MIB, 1024 * MIB] {
        let b = MemoryBudget::new(total);
        let sum: usize = parts(b).iter().sum();
        assert!(sum <= b.total(), "{total}: {sum} > {}", b.total());
        assert!(b.total() - sum < parts(b).len(), "{total}: lost {}", b.total() - sum);
    }
}

#[test]
fn the_split_is_deterministic_and_grows_with_the_total() {
    let a = MemoryBudget::new(50 * MIB);
    assert_eq!(parts(a), parts(MemoryBudget::new(50 * MIB)));
    let bigger = MemoryBudget::new(100 * MIB);
    for (small, big) in parts(a).iter().zip(parts(bigger)) {
        assert!(*small < big);
    }
}

#[test]
fn the_fifty_mib_split_is_pinned() {
    let b = MemoryBudget::new(50 * MIB);
    assert_eq!(b.total(), 50 * MIB);
    assert_eq!(
        parts(b),
        [
            25 * MIB / 2,
            25 * MIB / 2,
            4 * MIB,
            4 * MIB,
            4 * MIB,
            12 * MIB,
            MIB
        ]
    );
}

#[test]
fn a_total_below_the_floor_is_raised_to_the_floor() {
    assert_eq!(MemoryBudget::new(0).total(), MemoryBudget::MIN_BYTES);
    assert_eq!(MemoryBudget::new(MIB).total(), MemoryBudget::MIN_BYTES);
    assert_eq!(MemoryBudget::new(MemoryBudget::MIN_BYTES + 1).total(), MemoryBudget::MIN_BYTES + 1);
}

#[test]
fn options_take_every_graph_cap_from_the_budget() {
    let b = MemoryBudget::new(50 * MIB);
    let o = Options::new(b);
    assert_eq!(o.budget, b);
    assert_eq!(o.cache_bytes, b.redb_cache());
    assert_eq!(o.hot_bytes, b.hot());
    assert_eq!(o.warm_bytes, b.warm());
    assert_eq!(o.hot_entries, b.hot() / 2048);
}

#[test]
fn an_opened_graph_reports_the_budget_and_sizes_its_tiers_from_it() {
    let b = MemoryBudget::new(32 * MIB);
    let g = Graph::open_in_memory(b).unwrap();
    assert_eq!(g.budget(), b);
    assert_eq!(g.tier_stats().unwrap().budget_bytes, b.hot() + b.warm());
}

#[test]
fn two_budgets_give_two_tier_sizes() {
    let small = Graph::open_in_memory(MemoryBudget::new(16 * MIB)).unwrap();
    let large = Graph::open_in_memory(MemoryBudget::new(256 * MIB)).unwrap();
    assert!(small.tier_stats().unwrap().budget_bytes < large.tier_stats().unwrap().budget_bytes);
}
