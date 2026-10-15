//! The resource governor: pure slot math, command wrapping and cost learning.
//!
//! Slots for a heavy class are `min(cap, floor((free - reserve) / cost))` with
//! `reserve = max(2048 MB, 15% of total)`. At or below the reserve heavy slots are 0.
//! Light classes (`read`/`grep`/`collect`/`format`) and `llm` keep running below the
//! reserve, as the spec requires ("only `llm` and light leaves are scheduled").

use std::collections::{BTreeMap, VecDeque};

use crate::config::HouseConfig;
use crate::types::JobClass;

/// `reserve = max(floor, pct% of total)` in MB (rounded up).
pub fn reserve_mb(total_mb: u64, cfg: &HouseConfig) -> u64 {
    let pct = (total_mb.saturating_mul(cfg.reserve_pct)).div_ceil(100);
    cfg.reserve_floor_mb.max(pct)
}

/// Concurrency cap of one class.
pub fn class_cap(class: JobClass, cfg: &HouseConfig) -> u32 {
    match class {
        JobClass::Llm | JobClass::Light => cfg.max_agents,
        JobClass::TestUnit | JobClass::Build => cfg.heavy_cap,
        JobClass::Bench => 1,
    }
}

/// Declared cost of a class before any `CostObs` exists.
pub fn default_cost_mb(class: JobClass) -> u64 {
    match class {
        JobClass::Llm => 0,
        JobClass::Light => 50,
        JobClass::TestUnit => 800,
        JobClass::Build => 1_750,
        JobClass::Bench => 1_750,
    }
}

/// Slots for a class: `min(class cap, floor((free - reserve) / cost))`; heavy classes get
/// 0 at or below the reserve; light classes ignore the reserve; `Llm` is cap-bound.
pub fn slots(class: JobClass, free_mb: u64, total_mb: u64, cost_mb: u64, cfg: &HouseConfig) -> u32 {
    let cap = class_cap(class, cfg);
    let by_ram = |avail: u64| -> u32 {
        avail
            .checked_div(cost_mb)
            .map_or(cap, |n| u32::try_from(n).unwrap_or(u32::MAX))
    };
    match class {
        JobClass::Llm => cap,
        JobClass::Light => cap.min(by_ram(free_mb)),
        JobClass::TestUnit | JobClass::Build => {
            let reserve = reserve_mb(total_mb, cfg);
            if free_mb <= reserve {
                0
            } else {
                cap.min(by_ram(free_mb - reserve))
            }
        }
        JobClass::Bench => {
            if free_mb > reserve_mb(total_mb, cfg) {
                1
            } else {
                0
            }
        }
    }
}

/// Wraps a shell command for Windows: `CARGO_BUILD_JOBS` set and below-normal priority.
pub fn wrap_command(cmd: &str, cargo_jobs: u32) -> String {
    format!(
        "set CARGO_BUILD_JOBS={cargo_jobs} && start /belownormal /wait /b {}",
        cmd.trim()
    )
}

/// Moving average of observed peak RAM per class.
#[derive(Debug, Clone, Default)]
pub struct CostModel {
    obs: BTreeMap<JobClass, VecDeque<u64>>,
}

impl CostModel {
    /// Adds one observation, keeping the last `window`.
    pub fn record(&mut self, class: JobClass, ram_mb: u64, window: usize) {
        let q = self.obs.entry(class).or_default();
        q.push_back(ram_mb);
        while q.len() > window.max(1) {
            q.pop_front();
        }
    }

    /// Mean of the retained observations, if any.
    pub fn average(&self, class: JobClass) -> Option<u64> {
        let q = self.obs.get(&class).filter(|q| !q.is_empty())?;
        let sum: u64 = q.iter().sum();
        Some(sum / q.len() as u64)
    }

    /// Number of retained observations.
    pub fn samples(&self, class: JobClass) -> usize {
        self.obs.get(&class).map_or(0, VecDeque::len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> HouseConfig {
        HouseConfig::default()
    }

    #[test]
    fn reserve_is_max_of_floor_and_fifteen_percent() {
        let c = cfg();
        assert_eq!(reserve_mb(16_000, &c), 2_400);
        assert_eq!(reserve_mb(8_000, &c), 2_048);
        assert_eq!(reserve_mb(4_000, &c), 2_048);
        assert_eq!(reserve_mb(32_000, &c), 4_800);
        assert_eq!(reserve_mb(0, &c), 2_048);
    }

    #[test]
    fn test_unit_slot_table() {
        let c = cfg();
        // (free, total, expected slots at 800 MB)
        let table = [
            (6_000, 16_000, 4), // spec worked example: floor(3600/800)
            (2_000, 16_000, 0), // below reserve
            (2_400, 16_000, 0), // exactly the reserve
            (2_401, 16_000, 0), // 1 MB over: not a whole job
            (3_200, 16_000, 1), // exactly one job over
            (3_199, 16_000, 0),
            (5_000, 8_000, 3), // reserve is the 2048 floor
            (1_000, 4_000, 0),
            (0, 16_000, 0),
            (20_000, 32_000, 6), // 19 by RAM, clamped by the heavy cap
            (100_000, 128_000, 6),
        ];
        for (free, total, want) in table {
            assert_eq!(
                slots(JobClass::TestUnit, free, total, 800, &c),
                want,
                "free={free} total={total}"
            );
        }
    }

    #[test]
    fn build_slot_table() {
        let c = cfg();
        assert_eq!(slots(JobClass::Build, 6_000, 16_000, 1_750, &c), 2);
        assert_eq!(slots(JobClass::Build, 4_000, 16_000, 1_750, &c), 0);
        assert_eq!(slots(JobClass::Build, 1_000, 16_000, 1_750, &c), 0);
    }

    #[test]
    fn light_and_llm_ignore_the_reserve() {
        let c = cfg();
        assert_eq!(slots(JobClass::Light, 1_000, 16_000, 50, &c), 20);
        assert_eq!(slots(JobClass::Light, 500, 16_000, 50, &c), 10);
        assert_eq!(slots(JobClass::Light, 10, 16_000, 50, &c), 0);
        assert_eq!(slots(JobClass::Llm, 0, 16_000, 0, &c), 20);
    }

    #[test]
    fn heavy_cap_is_configurable() {
        let c = HouseConfig {
            heavy_cap: 2,
            ..cfg()
        };
        assert_eq!(slots(JobClass::TestUnit, 60_000, 64_000, 800, &c), 2);
        assert_eq!(class_cap(JobClass::Build, &c), 2);
    }

    #[test]
    fn default_costs_match_the_spec() {
        assert_eq!(default_cost_mb(JobClass::Llm), 0);
        assert_eq!(default_cost_mb(JobClass::Light), 50);
        assert_eq!(default_cost_mb(JobClass::TestUnit), 800);
        assert!((1_500..=2_000).contains(&default_cost_mb(JobClass::Build)));
    }

    #[test]
    fn wrapper_sets_jobs_and_lowers_priority() {
        assert_eq!(
            wrap_command("cargo test -p x foo", 2),
            "set CARGO_BUILD_JOBS=2 && start /belownormal /wait /b cargo test -p x foo"
        );
        assert_eq!(
            wrap_command("  cargo build -p y  ", 4),
            "set CARGO_BUILD_JOBS=4 && start /belownormal /wait /b cargo build -p y"
        );
    }

    #[test]
    fn cost_model_is_a_windowed_mean() {
        let mut m = CostModel::default();
        assert_eq!(m.average(JobClass::TestUnit), None);
        for v in [600, 900, 1200] {
            m.record(JobClass::TestUnit, v, 5);
        }
        assert_eq!(m.average(JobClass::TestUnit), Some(900));
        m.record(JobClass::TestUnit, 1500, 2); // window 2 keeps 1200, 1500
        assert_eq!(m.samples(JobClass::TestUnit), 2);
        assert_eq!(m.average(JobClass::TestUnit), Some(1350));
        assert_eq!(m.average(JobClass::Build), None);
    }
}
