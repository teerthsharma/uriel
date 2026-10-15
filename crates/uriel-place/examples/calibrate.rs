use std::time::{Duration, Instant};

use uriel_place::calibrate::{machine_info, measure, today, Calibration};

fn human(b: u64) -> String {
    if b >= 1 << 20 {
        format!("{} MiB", b >> 20)
    } else {
        format!("{} KiB", b >> 10)
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let out = args
        .next()
        .unwrap_or_else(|| "calibration.json".to_string());
    let secs: u64 = args
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(12)
        .clamp(1, 20);
    let t = Instant::now();
    let points = measure(Duration::from_secs(secs), 4 << 10, 256 << 20);
    let took = t.elapsed();
    let mut machine = machine_info();
    machine.insert("budget_seconds".to_string(), secs.to_string());
    machine.insert("elapsed_ms".to_string(), took.as_millis().to_string());
    machine.insert(
        "note".to_string(),
        "measured on this machine at this moment; not portable".to_string(),
    );
    let cal = Calibration::from_points(points, machine, today());

    println!(
        "{:>10}  {:>12}  {:>10}",
        "working set", "ns per load", "loads"
    );
    for p in &cal.points {
        println!(
            "{:>10}  {:>12.2}  {:>10}",
            human(p.bytes),
            p.ps_per_load as f64 / 1000.0,
            p.loads
        );
    }
    for k in &cal.knees {
        println!(
            "knee: {} -> {}  x{:.2}",
            human(k.from_bytes),
            human(k.to_bytes),
            k.ratio_x100 as f64 / 100.0
        );
    }
    println!(
        "suggested hot bytes: {}",
        human(cal.suggest_hot_bytes(None))
    );
    println!("elapsed: {:.2} s (budget {secs} s)", took.as_secs_f64());
    match std::fs::write(&out, cal.to_json()) {
        Ok(()) => println!("wrote {out}"),
        Err(e) => eprintln!("could not write {out}: {e}"),
    }
}
