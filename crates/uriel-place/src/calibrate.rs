use std::collections::BTreeMap;
use std::hint::black_box;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::replay::SplitMix;

pub const DEFAULT_HOT_BYTES: u64 = 1 << 20;
const SLOT_BYTES: u64 = 64;
const WORDS_PER_SLOT: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Point {
    pub bytes: u64,
    pub ps_per_load: u64,
    pub loads: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Knee {
    pub from_bytes: u64,
    pub to_bytes: u64,
    pub ratio_x100: u64,
}

pub fn find_knees(points: &[Point]) -> Vec<Knee> {
    let mut p: Vec<Point> = points.to_vec();
    p.sort_by_key(|x| x.bytes);
    let mut out: Vec<Knee> = Vec::new();
    let mut run: Option<(u64, u64, u64, u64)> = None;
    let flush = |run: &mut Option<(u64, u64, u64, u64)>, out: &mut Vec<Knee>| {
        if let Some((from_bytes, before, after, to_bytes)) = run.take() {
            let ratio = u128::from(after) * 100 / u128::from(before.max(1));
            out.push(Knee {
                from_bytes,
                to_bytes,
                ratio_x100: ratio.min(u128::from(u64::MAX)) as u64,
            });
        }
    };
    for w in p.windows(2) {
        let (a, b) = (w[0], w[1]);
        let rise = a.ps_per_load > 0
            && b.ps_per_load > 0
            && u128::from(b.ps_per_load) * 10 > u128::from(a.ps_per_load) * 13;
        if rise {
            run = Some(match run {
                Some((f, before, _, _)) => (f, before, b.ps_per_load, b.bytes),
                None => (a.bytes, a.ps_per_load, b.ps_per_load, b.bytes),
            });
        } else {
            flush(&mut run, &mut out);
        }
    }
    flush(&mut run, &mut out);
    out
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Calibration {
    pub points: Vec<Point>,
    pub knees: Vec<Knee>,
    pub machine: BTreeMap<String, String>,
    pub date: String,
}

impl Calibration {
    pub fn from_points(
        mut points: Vec<Point>,
        machine: BTreeMap<String, String>,
        date: String,
    ) -> Calibration {
        points.sort_by_key(|p| p.bytes);
        let knees = find_knees(&points);
        Calibration {
            points,
            knees,
            machine,
            date,
        }
    }

    pub fn suggest_hot_bytes(&self, override_bytes: Option<u64>) -> u64 {
        if let Some(b) = override_bytes {
            return b;
        }
        match self.knees.last() {
            Some(k) => (k.from_bytes / 2).max(4096),
            None => DEFAULT_HOT_BYTES,
        }
    }

    pub fn to_json(&self) -> String {
        let mut o = String::new();
        o.push_str("{\n");
        o.push_str("  \"schema\": \"uriel-place.calibration.v1\",\n");
        o.push_str(&format!("  \"date\": \"{}\",\n", esc(&self.date)));
        o.push_str("  \"unit\": \"picoseconds per dependent load (min over repeats)\",\n");
        o.push_str("  \"machine\": {");
        let m: Vec<String> = self
            .machine
            .iter()
            .map(|(k, v)| format!("\"{}\": \"{}\"", esc(k), esc(v)))
            .collect();
        o.push_str(&m.join(", "));
        o.push_str("},\n  \"points\": [\n");
        let pts: Vec<String> = self
            .points
            .iter()
            .map(|p| {
                format!(
                    "    {{\"bytes\": {}, \"ps_per_load\": {}, \"loads\": {}}}",
                    p.bytes, p.ps_per_load, p.loads
                )
            })
            .collect();
        o.push_str(&pts.join(",\n"));
        o.push_str("\n  ],\n  \"knees\": [\n");
        let ks: Vec<String> = self
            .knees
            .iter()
            .map(|k| {
                format!(
                    "    {{\"from_bytes\": {}, \"to_bytes\": {}, \"ratio_x100\": {}}}",
                    k.from_bytes, k.to_bytes, k.ratio_x100
                )
            })
            .collect();
        o.push_str(&ks.join(",\n"));
        o.push_str(&format!(
            "\n  ],\n  \"suggested_hot_bytes\": {}\n}}\n",
            self.suggest_hot_bytes(None)
        ));
        o
    }
}

fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o
}

pub fn civil_date(unix_secs: u64) -> String {
    let z = (unix_secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

pub fn today() -> String {
    let s = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    civil_date(s)
}

pub fn machine_info() -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    for k in [
        "PROCESSOR_IDENTIFIER",
        "NUMBER_OF_PROCESSORS",
        "PROCESSOR_ARCHITECTURE",
        "OS",
    ] {
        if let Ok(v) = std::env::var(k) {
            m.insert(k.to_string(), v);
        }
    }
    m.insert("target_os".to_string(), std::env::consts::OS.to_string());
    m.insert(
        "target_arch".to_string(),
        std::env::consts::ARCH.to_string(),
    );
    m
}

fn build_chain(slots: usize, seed: u64) -> Vec<u64> {
    let mut order: Vec<u32> = (0..slots as u32).collect();
    let mut rng = SplitMix(seed);
    for i in (1..slots).rev() {
        let j = rng.below(i as u64 + 1) as usize;
        order.swap(i, j);
    }
    let mut buf = vec![0u64; slots * WORDS_PER_SLOT];
    for i in 0..slots {
        let from = order[i] as usize;
        let to = order[(i + 1) % slots];
        buf[from * WORDS_PER_SLOT] = u64::from(to);
    }
    buf
}

fn chase(buf: &[u64], start: usize, loads: u64) -> usize {
    let mut idx = start;
    for _ in 0..loads {
        idx = buf.get(idx * WORDS_PER_SLOT).copied().unwrap_or(0) as usize;
    }
    black_box(idx)
}

pub fn measure(total: Duration, min_bytes: u64, max_bytes: u64) -> Vec<Point> {
    let start = Instant::now();
    let mut sizes = Vec::new();
    let mut b = min_bytes.max(SLOT_BYTES * 4).next_power_of_two();
    while b <= max_bytes && b <= 1 << 33 {
        sizes.push(b);
        b <<= 1;
    }
    let mut out = Vec::with_capacity(sizes.len());
    for (i, &bytes) in sizes.iter().enumerate() {
        let left = total.saturating_sub(start.elapsed());
        if left < Duration::from_millis(5) {
            break;
        }
        let slice = left / (sizes.len() - i) as u32;
        let slots = (bytes / SLOT_BYTES) as usize;
        let buf = build_chain(slots, 0x5ea1_0000 ^ bytes);
        let probe_loads = (slots as u64).clamp(1024, 65_536);
        let t = Instant::now();
        let mut at = chase(&buf, 0, probe_loads);
        let est_ns = (t.elapsed().as_nanos() / u128::from(probe_loads)).max(1);
        let run_budget = slice
            .saturating_sub(t.elapsed())
            .max(Duration::from_millis(1));
        let loads = ((run_budget.as_nanos() / 4) / est_ns).clamp(2048, 1 << 21) as u64;
        let mut best: Option<(u64, u64)> = None;
        let mut repeats = 0u32;
        let run_start = Instant::now();
        while repeats < 3 || (run_start.elapsed() < run_budget && repeats < 16) {
            let t = Instant::now();
            at = chase(&buf, at, loads);
            let ps = (t.elapsed().as_nanos().saturating_mul(1000) / u128::from(loads)) as u64;
            if best.is_none_or(|(b, _)| ps < b) {
                best = Some((ps.max(1), loads));
            }
            repeats += 1;
            if start.elapsed() >= total.saturating_add(Duration::from_millis(250)) {
                break;
            }
        }
        black_box(at);
        if let Some((ps_per_load, loads)) = best {
            out.push(Point {
                bytes,
                ps_per_load,
                loads,
            });
        }
    }
    out
}
