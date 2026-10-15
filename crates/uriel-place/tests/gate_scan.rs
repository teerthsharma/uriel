use uriel_place::fixed::ONE;
use uriel_place::{AccessKind, Config, Event, Placer, Region, RegionId, Weights};

fn run(gate_enabled: bool) -> (Vec<RegionId>, Vec<RegionId>) {
    let regions: Vec<Region> = (0..44)
        .map(|i| Region {
            id: RegionId(i),
            size: 1024,
            community: i,
        })
        .collect();
    let cfg = Config {
        hot_bytes: 4096,
        warm_bytes: 0,
        prefetch_bytes: 0,
        half_life: 1_000_000,
        gate_enabled,
        gate_capacity: 64,
        weights: Weights {
            heat: ONE,
            topo: 0,
            pin: 0,
            size: 0,
            life: 0,
        },
        ..Config::default()
    };
    let mut p = Placer::new(cfg, &regions, &[]);
    for _ in 0..12 {
        for r in 0..4 {
            p.apply(Event::Access {
                region: RegionId(r),
                kind: AccessKind::Read,
            });
        }
        p.plan();
    }
    let before = p.plan().hot;
    for r in 4..44 {
        p.apply(Event::Access {
            region: RegionId(r),
            kind: AccessKind::Scan,
        });
        if r % 4 == 3 {
            p.plan();
        }
    }
    (before, p.plan().hot)
}

#[test]
fn gate_on_keeps_the_hot_set_through_a_scan() {
    let (before, after) = run(true);
    assert_eq!(before, (0..4).map(RegionId).collect::<Vec<_>>());
    assert_eq!(after, before, "a scan flushed the hot set with the gate on");
}

#[test]
fn gate_off_lets_the_same_scan_flush_it() {
    let (before, after) = run(false);
    assert_eq!(
        before,
        (0..4).map(RegionId).collect::<Vec<_>>(),
        "hot set not built before the scan"
    );
    let kept = after.iter().filter(|r| before.contains(r)).count();
    eprintln!("SCAN PAIR: hot regions surviving the scan: gate on = 4/4, gate off = {kept}/4");
    assert!(
        kept < 4,
        "baseline must show the flush, otherwise the pair proves nothing"
    );
}
