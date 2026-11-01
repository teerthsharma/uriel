use serde::{Deserialize, Serialize};

use crate::types::Ms;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RouterConfig {
    pub max_workers: u32,
    pub eval_timeout_ms: Ms,
    pub eval_retries: u8,
    pub prepare_max_ms: Ms,
    pub background_max_ms: Ms,
    pub poll_horizon_ms: Ms,
    pub redelivery_base_ms: Ms,
    pub redelivery_max_ms: Ms,
    pub worker_cost_mb: u64,
    pub reserve_floor_mb: u64,
    pub reserve_pct: u64,
    pub unavailable_attention_ms: Ms,
    pub unavailable_health_ms: Ms,
}

impl Default for RouterConfig {
    fn default() -> RouterConfig {
        RouterConfig {
            max_workers: 3,
            eval_timeout_ms: 120_000,
            eval_retries: 2,
            prepare_max_ms: 600_000,
            background_max_ms: 1_800_000,
            poll_horizon_ms: 14 * 86_400_000,
            redelivery_base_ms: 2_000,
            redelivery_max_ms: 60_000,
            worker_cost_mb: 800,
            reserve_floor_mb: 2_048,
            reserve_pct: 15,
            unavailable_attention_ms: 86_400_000,
            unavailable_health_ms: 3_600_000,
        }
    }
}

impl RouterConfig {
    pub fn clamped(mut self) -> RouterConfig {
        self.max_workers = self.max_workers.clamp(1, 16);
        self.eval_timeout_ms = self.eval_timeout_ms.clamp(30_000, 600_000);
        self.worker_cost_mb = self.worker_cost_mb.max(1);
        self.redelivery_base_ms = self.redelivery_base_ms.max(1);
        self.redelivery_max_ms = self.redelivery_max_ms.max(self.redelivery_base_ms);
        self
    }

    pub fn reserve_mb(&self, total_mb: u64) -> u64 {
        let pct = total_mb.saturating_mul(self.reserve_pct).div_ceil(100);
        self.reserve_floor_mb.max(pct)
    }

    pub fn ram_slots(&self, free_mb: u64, total_mb: u64) -> u32 {
        let reserve = self.reserve_mb(total_mb);
        if free_mb <= reserve {
            return 0;
        }
        u32::try_from((free_mb - reserve) / self.worker_cost_mb).unwrap_or(u32::MAX)
    }

    pub fn redelivery_delay(&self, attempt: u32) -> Ms {
        let shift = attempt.saturating_sub(1).min(30);
        self.redelivery_base_ms
            .saturating_mul(1u64 << shift)
            .min(self.redelivery_max_ms)
    }
}
