//! Tunables of the harness.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::HouseError;
use crate::types::Tier;

/// Environment variable that overrides the Dr House model slot.
pub const DR_HOUSE_ENV: &str = "SEAL_DR_HOUSE_MODEL";

/// Which model each role runs on. Switching a role to a new model is changing one string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelSlots {
    /// Foreman, Chase and Cameron (the charting fellows).
    pub fellows: String,
    /// Residents: leaves that need judgment or code.
    pub residents: String,
    /// Nurses: mechanical leaves.
    pub nurses: String,
    /// Wilson, the fact checker.
    pub wilson: String,
    /// Inspector, the auditor.
    pub inspector: String,
    /// Dr House, the one leap.
    pub dr_house: String,
    /// Model used when a Dr House dispatch to [`ModelSlots::dr_house`] fails.
    pub dr_house_fallback: String,
}

impl Default for ModelSlots {
    fn default() -> Self {
        ModelSlots {
            fellows: "opus".into(),
            residents: "sonnet".into(),
            nurses: "haiku".into(),
            wilson: "opus".into(),
            inspector: "opus".into(),
            dr_house: "opus".into(),
            dr_house_fallback: "opus".into(),
        }
    }
}

impl ModelSlots {
    /// The slot an executing tier runs on: residents for Sonnet leaves, nurses for Haiku.
    pub fn for_tier(&self, tier: Tier) -> &str {
        match tier {
            Tier::Sonnet => &self.residents,
            Tier::Haiku => &self.nurses,
        }
    }
}

/// Configuration for [`crate::House`].
///
/// Every field except `project_root` can be set from a JSON file ([`HouseConfig::from_file`]);
/// a missing key keeps its default.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HouseConfig {
    /// Project root; `reads`, `writes` and covered files resolve against it.
    #[serde(skip)]
    pub project_root: PathBuf,
    /// L2 (full suite) picks are produced only when this is `true`.
    pub allow_full_suite: bool,
    /// Command of the L2 pick.
    pub full_suite_cmd: String,
    /// Absolute floor of the RAM reserve, MB. Spec: 2048.
    pub reserve_floor_mb: u64,
    /// Fraction of total RAM reserved, in percent. Spec: 15.
    pub reserve_pct: u64,
    /// Global cap on concurrent heavy jobs. Spec: 6.
    pub heavy_cap: u32,
    /// Cap on concurrently Active agents (the in-app subagent cap).
    pub max_agents: u32,
    /// `CARGO_BUILD_JOBS` for every wrapped command.
    pub cargo_jobs: u32,
    /// Weight of one second against one token in ALT cost: `cost = tokens + secs * alt_secs_weight`.
    pub alt_secs_weight: f64,
    /// Re-charts allowed per refuted goal before it is Blocked as a leap candidate. Spec: 2.
    pub max_rechart: u8,
    /// How many `CostObs` samples per class the moving average covers.
    pub cost_window: usize,
    /// Model per role.
    pub models: ModelSlots,
}

impl Default for HouseConfig {
    fn default() -> Self {
        HouseConfig {
            project_root: PathBuf::from("."),
            allow_full_suite: false,
            full_suite_cmd: "cargo test --workspace".to_string(),
            reserve_floor_mb: 2048,
            reserve_pct: 15,
            heavy_cap: 6,
            max_agents: 20,
            cargo_jobs: 2,
            alt_secs_weight: 20.0,
            max_rechart: 2,
            cost_window: 5,
            models: ModelSlots::default(),
        }
    }
}

impl HouseConfig {
    /// Default configuration rooted at `root`.
    pub fn at(root: impl Into<PathBuf>) -> HouseConfig {
        HouseConfig {
            project_root: root.into(),
            ..HouseConfig::default()
        }
    }

    /// Reads a JSON config file. Keys that are absent keep their defaults and unknown keys
    /// are ignored; a missing or malformed file is an error. `project_root` is never read
    /// from the file. Environment overrides are not applied; see [`HouseConfig::apply_env`].
    pub fn from_file(path: impl AsRef<Path>) -> Result<HouseConfig, HouseError> {
        let text = std::fs::read_to_string(path.as_ref())?;
        Ok(serde_json::from_str(&text)?)
    }

    /// Applies the environment overrides: [`DR_HOUSE_ENV`] sets the Dr House slot.
    pub fn apply_env(&mut self) {
        self.apply_env_from(|k| std::env::var(k).ok());
    }

    /// [`HouseConfig::apply_env`] over an arbitrary lookup (tests).
    pub fn apply_env_from(&mut self, get: impl Fn(&str) -> Option<String>) {
        if let Some(v) = get(DR_HOUSE_ENV) {
            let v = v.trim();
            if !v.is_empty() {
                self.models.dr_house = v.to_string();
            }
        }
    }
}
