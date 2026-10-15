use uriel_sys::{process_memory, Arena, SysError, Width};

const CAP: usize = 256 << 20;
const SLACK: u64 = 32 << 20;
const SLAB: usize = 1 << 20;

#[test]
fn allocate_until_cap_keeps_rss_growth_bounded_by_cap_plus_slack() {
    let before = process_memory().unwrap();
    let mut arena = Arena::with_cap(CAP).unwrap();
    let after_reserve = process_memory().unwrap();
    let mut slabs = 0usize;
    loop {
        match arena.alloc(SLAB, Width::W8) {
            Ok(slab) => {
                arena
                    .region_mut()
                    .view_mut::<u8>(&slab.write_cap())
                    .unwrap()
                    .fill(0xA5);
                slabs += 1;
            }
            Err(SysError::CapExceeded { .. }) => break,
            Err(other) => panic!("unexpected {other:?}"),
        }
    }
    let at_cap = process_memory().unwrap();
    for _ in 0..10_000 {
        assert!(matches!(
            arena.alloc(SLAB, Width::W8),
            Err(SysError::CapExceeded { .. })
        ));
    }
    let after_overrun = process_memory().unwrap();
    let grow_reserve = after_reserve.working_set.saturating_sub(before.working_set);
    let grow_cap = at_cap.working_set.saturating_sub(before.working_set);
    let grow_over = after_overrun.working_set.saturating_sub(before.working_set);
    let private_over = after_overrun.private_bytes.saturating_sub(before.private_bytes);
    println!(
        "cap={CAP} slack={SLACK} slabs={slabs} rss_before={} rss_after_reserve={} rss_at_cap={} rss_after_overrun={} grow_reserve={grow_reserve} grow_at_cap={grow_cap} grow_after_overrun={grow_over} private_growth={private_over}",
        before.working_set, after_reserve.working_set, at_cap.working_set, after_overrun.working_set
    );
    assert_eq!(slabs, CAP / SLAB);
    assert_eq!(arena.used(), CAP);
    assert!(grow_reserve < SLACK, "reserving the cap must not touch pages: {grow_reserve}");
    assert!(grow_cap >= (CAP as u64) * 9 / 10, "the fill must be visible in RSS: {grow_cap}");
    assert!(grow_cap <= CAP as u64 + SLACK, "{grow_cap} exceeds cap plus slack");
    assert!(grow_over <= CAP as u64 + SLACK, "{grow_over} exceeds cap plus slack");
    assert!(private_over <= CAP as u64 + SLACK, "{private_over} exceeds cap plus slack");
    assert!(grow_over.abs_diff(grow_cap) < SLACK, "failed allocations must not grow RSS");
}
