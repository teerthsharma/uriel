use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde_json::{json, Value as Json};
use uriel_core::{auto_memory_mib, MemoryBudget};

use crate::config::{Config, DEFAULT_MEMORY_MIB};
use crate::tools::Context;

pub const TICK: Duration = Duration::from_secs(2);
pub const SHED_COOLDOWN: Duration = Duration::from_secs(10);
pub const AMBER_PERCENT: u64 = 80;
pub const RED_PERCENT: u64 = 100;
pub const GREEN_BELOW_PERCENT: u64 = 70;
pub const AMBER_BELOW_PERCENT: u64 = 90;

const MIB: f64 = 1024.0 * 1024.0;
const NO_OVERRIDE: u64 = u64::MAX;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Green,
    Amber,
    Red,
}

impl Level {
    pub fn name(self) -> &'static str {
        match self {
            Level::Green => "green",
            Level::Amber => "amber",
            Level::Red => "red",
        }
    }

    fn from_u8(v: u8) -> Level {
        match v {
            0 => Level::Green,
            1 => Level::Amber,
            _ => Level::Red,
        }
    }
}

pub fn next_level(prev: Level, rss: u64, limit: u64) -> Level {
    let pct = if limit == 0 {
        u64::MAX
    } else {
        (u128::from(rss) * 100 / u128::from(limit)).min(u128::from(u64::MAX)) as u64
    };
    match prev {
        Level::Green if pct >= RED_PERCENT => Level::Red,
        Level::Green if pct >= AMBER_PERCENT => Level::Amber,
        Level::Green => Level::Green,
        Level::Amber if pct >= RED_PERCENT => Level::Red,
        Level::Amber if pct < GREEN_BELOW_PERCENT => Level::Green,
        Level::Amber => Level::Amber,
        Level::Red if pct >= AMBER_BELOW_PERCENT => Level::Red,
        Level::Red if pct < GREEN_BELOW_PERCENT => Level::Green,
        Level::Red => Level::Amber,
    }
}

pub struct Governor {
    budget_bytes: u64,
    limit_bytes: u64,
    level: AtomicU8,
    rss: AtomicU64,
    peak: AtomicU64,
    freed: AtomicU64,
    samples: AtomicU64,
    shrinks: AtomicU64,
    reopens: AtomicU64,
    released: AtomicBool,
    forced_rss: AtomicU64,
    last_shed: Mutex<Option<Instant>>,
}

impl Governor {
    pub fn new(budget: MemoryBudget) -> Governor {
        Governor {
            budget_bytes: budget.total() as u64,
            limit_bytes: (budget.total() + budget.reserve()) as u64,
            level: AtomicU8::new(0),
            rss: AtomicU64::new(0),
            peak: AtomicU64::new(0),
            freed: AtomicU64::new(0),
            samples: AtomicU64::new(0),
            shrinks: AtomicU64::new(0),
            reopens: AtomicU64::new(0),
            released: AtomicBool::new(false),
            forced_rss: AtomicU64::new(NO_OVERRIDE),
            last_shed: Mutex::new(None),
        }
    }

    pub fn force_rss(&self, bytes: Option<u64>) {
        self.forced_rss.store(bytes.unwrap_or(NO_OVERRIDE), Ordering::SeqCst);
    }

    pub fn level(&self) -> Level {
        Level::from_u8(self.level.load(Ordering::SeqCst))
    }

    pub fn shrinks(&self) -> u64 {
        self.shrinks.load(Ordering::SeqCst)
    }

    pub fn reopens(&self) -> u64 {
        self.reopens.load(Ordering::SeqCst)
    }

    pub fn limit_bytes(&self) -> u64 {
        self.limit_bytes
    }

    fn sample(&self) -> Option<u64> {
        match self.forced_rss.load(Ordering::SeqCst) {
            NO_OVERRIDE => uriel_sys::process_memory().ok().map(|m| m.working_set),
            forced => Some(forced),
        }
    }

    pub fn tick(&self, ctx: &Context) -> Level {
        let Some(rss) = self.sample() else {
            return self.level();
        };
        self.samples.fetch_add(1, Ordering::SeqCst);
        self.rss.store(rss, Ordering::SeqCst);
        self.peak.fetch_max(rss, Ordering::SeqCst);
        let prev = self.level();
        let next = next_level(prev, rss, self.limit_bytes);
        self.level.store(next as u8, Ordering::SeqCst);
        if next == Level::Green {
            return next;
        }
        let due = {
            let last = self.last_shed.lock().unwrap_or_else(PoisonError::into_inner);
            last.is_none_or(|t| t.elapsed() >= SHED_COOLDOWN)
        };
        if next > prev || due {
            self.shed(ctx, next, rss);
        }
        next
    }

    fn shed(&self, ctx: &Context, level: Level, before: u64) {
        if let Some(open) = ctx.db.peek() {
            open.engine.shed_caches();
        }
        if level == Level::Red && ctx.db.release() {
            self.released.store(true, Ordering::SeqCst);
        }
        uriel_sys::trim_heap();
        self.shrinks.fetch_add(1, Ordering::SeqCst);
        *self.last_shed.lock().unwrap_or_else(PoisonError::into_inner) = Some(Instant::now());
        if let Some(after) = self.sample() {
            self.freed.store(before.saturating_sub(after), Ordering::SeqCst);
            self.rss.store(after, Ordering::SeqCst);
        }
    }

    pub fn settle(&self, db_open: bool) {
        if db_open && self.released.swap(false, Ordering::SeqCst) {
            self.reopens.fetch_add(1, Ordering::SeqCst);
        }
    }

    pub fn snapshot(&self) -> Json {
        let rss = match self.sample() {
            Some(v) => v,
            None => self.rss.load(Ordering::SeqCst),
        };
        let free = uriel_sys::memory_status().ok();
        json!({
            "rss_mib": round1(rss as f64 / MIB),
            "peak_rss_mib": round1(self.peak.load(Ordering::SeqCst).max(rss) as f64 / MIB),
            "budget_mib": round1(self.budget_bytes as f64 / MIB),
            "limit_mib": round1(self.limit_bytes as f64 / MIB),
            "level": self.level().name(),
            "shrinks": self.shrinks(),
            "reopens": self.reopens(),
            "last_freed_mib": round1(self.freed.load(Ordering::SeqCst) as f64 / MIB),
            "free_ram_mib": free.map(|s| round1(s.available as f64 / MIB)),
            "total_ram_mib": free.map(|s| round1(s.total as f64 / MIB)),
            "os": format!("{}/{}", std::env::consts::OS, std::env::consts::ARCH),
            "rss_source": uriel_sys::rss_source(),
            "free_ram_source": free.map(|s| s.source),
        })
    }
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

pub fn auto_budget_mib() -> (usize, Option<uriel_sys::MemoryStatus>) {
    match uriel_sys::memory_status() {
        Ok(status) => (auto_memory_mib(status.available), Some(status)),
        Err(_) => (DEFAULT_MEMORY_MIB, None),
    }
}

pub fn resolve(cfg: &Config) -> Config {
    let mut out = cfg.clone();
    if cfg.memory_auto {
        out.memory = MemoryBudget::mib(auto_budget_mib().0);
    }
    out
}

pub struct PlatformReport {
    pub os: String,
    pub rss_bytes: Option<u64>,
    pub rss_source: &'static str,
    pub free_bytes: Option<u64>,
    pub total_bytes: Option<u64>,
    pub free_source: Option<&'static str>,
    pub auto_mib: usize,
    pub large_pages: String,
}

pub fn platform_report() -> PlatformReport {
    let (auto_mib, status) = auto_budget_mib();
    let large_pages = match uriel_sys::large_page_status() {
        uriel_sys::LargePageStatus::Ready { minimum } => format!("ready ({minimum} byte pages)"),
        uriel_sys::LargePageStatus::PrivilegeMissing => {
            "privilege missing, falls back to normal pages".to_string()
        }
        uriel_sys::LargePageStatus::Unsupported => {
            format!("not used on {}, falls back to the system allocator", std::env::consts::OS)
        }
    };
    PlatformReport {
        os: format!("{}/{}", std::env::consts::OS, std::env::consts::ARCH),
        rss_bytes: uriel_sys::process_memory().ok().map(|m| m.working_set),
        rss_source: uriel_sys::rss_source(),
        free_bytes: status.map(|s| s.available),
        total_bytes: status.map(|s| s.total),
        free_source: status.map(|s| s.source),
        auto_mib,
        large_pages,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIMIT: u64 = 100;

    #[test]
    fn green_climbs_at_eighty_and_one_hundred_percent() {
        assert_eq!(next_level(Level::Green, 79, LIMIT), Level::Green);
        assert_eq!(next_level(Level::Green, 80, LIMIT), Level::Amber);
        assert_eq!(next_level(Level::Green, 100, LIMIT), Level::Red);
    }

    #[test]
    fn amber_only_returns_to_green_below_seventy() {
        assert_eq!(next_level(Level::Amber, 79, LIMIT), Level::Amber);
        assert_eq!(next_level(Level::Amber, 70, LIMIT), Level::Amber);
        assert_eq!(next_level(Level::Amber, 69, LIMIT), Level::Green);
        assert_eq!(next_level(Level::Amber, 100, LIMIT), Level::Red);
    }

    #[test]
    fn red_steps_down_only_below_ninety_and_green_below_seventy() {
        assert_eq!(next_level(Level::Red, 99, LIMIT), Level::Red);
        assert_eq!(next_level(Level::Red, 90, LIMIT), Level::Red);
        assert_eq!(next_level(Level::Red, 89, LIMIT), Level::Amber);
        assert_eq!(next_level(Level::Red, 69, LIMIT), Level::Green);
    }

    #[test]
    fn a_reading_that_wobbles_around_a_threshold_does_not_flap() {
        let mut level = Level::Green;
        let mut changes = 0;
        for rss in [78, 82, 79, 81, 78, 83, 79, 80, 77, 82] {
            let next = next_level(level, rss, LIMIT);
            changes += usize::from(next != level);
            level = next;
        }
        assert_eq!(level, Level::Amber);
        assert_eq!(changes, 1);
    }

    #[test]
    fn a_zero_limit_is_red_and_never_divides_by_zero() {
        assert_eq!(next_level(Level::Green, 1, 0), Level::Red);
    }

    #[test]
    fn the_limit_is_the_budget_plus_its_reserve() {
        let b = MemoryBudget::mib(50);
        let g = Governor::new(b);
        assert_eq!(g.limit_bytes(), (b.total() + b.reserve()) as u64);
        assert_eq!(g.level(), Level::Green);
    }

    #[test]
    fn resolve_keeps_an_explicit_budget_and_sizes_an_auto_one_from_free_ram() {
        let mut cfg = match crate::config::parse_args(["--memory-mib", "64"]).expect("parse") {
            crate::config::Cli::Run(c) => c,
            _ => panic!("run"),
        };
        assert!(!cfg.memory_auto);
        assert_eq!(resolve(&cfg).memory, MemoryBudget::mib(64));
        cfg.memory_auto = true;
        let mib = resolve(&cfg).memory.total() / (1024 * 1024);
        assert!((16..=50).contains(&mib), "{mib}");
    }
}
