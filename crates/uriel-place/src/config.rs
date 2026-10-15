use crate::event::{Level, RegionId, Tier};
use crate::fixed::{Fx, ONE};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Weights {
    pub heat: Fx,
    pub topo: Fx,
    pub pin: Fx,
    pub size: Fx,
    pub life: Fx,
}

impl Default for Weights {
    fn default() -> Self {
        Weights {
            heat: ONE,
            topo: ONE / 4,
            pin: 1_000_000 * ONE,
            size: ONE / 64,
            life: ONE / 2,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub hot_bytes: u64,
    pub warm_bytes: u64,
    pub prefetch_bytes: u64,
    pub half_life: u64,
    pub weights: Weights,
    pub size_unit: u64,
    pub gate_enabled: bool,
    pub gate_capacity: usize,
    pub compact_threshold: Fx,
    pub compact_min_regions: usize,
    pub durable_span: u64,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            hot_bytes: 1 << 20,
            warm_bytes: 4 << 20,
            prefetch_bytes: 256 << 10,
            half_life: 256,
            weights: Weights::default(),
            size_unit: 4096,
            gate_enabled: true,
            gate_capacity: 1024,
            compact_threshold: ONE + ONE / 2,
            compact_min_regions: 4,
            durable_span: 1024,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Placement {
    pub clock: u64,
    pub pressure: Option<Level>,
    pub hot: Vec<RegionId>,
    pub warm: Vec<RegionId>,
    pub cold: Vec<RegionId>,
    pub prefetch: Vec<RegionId>,
    pub pin_overflow: Vec<RegionId>,
    pub compact: Vec<Tier>,
    pub entropy: [Fx; 3],
    pub hot_bytes: u64,
    pub warm_bytes: u64,
    pub hot_budget: u64,
}

impl Placement {
    pub fn encode(&self) -> Vec<u8> {
        let mut o =
            Vec::with_capacity(64 + 4 * (self.hot.len() + self.warm.len() + self.cold.len()));
        o.extend_from_slice(&self.clock.to_le_bytes());
        o.push(self.pressure.map_or(255, Level::tag));
        for list in [
            &self.hot,
            &self.warm,
            &self.cold,
            &self.prefetch,
            &self.pin_overflow,
        ] {
            o.extend_from_slice(&(list.len() as u64).to_le_bytes());
            for r in list {
                o.extend_from_slice(&r.0.to_le_bytes());
            }
        }
        o.extend_from_slice(&(self.compact.len() as u64).to_le_bytes());
        for t in &self.compact {
            o.push(t.tag());
        }
        for e in self.entropy {
            o.extend_from_slice(&e.to_le_bytes());
        }
        o.extend_from_slice(&self.hot_bytes.to_le_bytes());
        o.extend_from_slice(&self.warm_bytes.to_le_bytes());
        o.extend_from_slice(&self.hot_budget.to_le_bytes());
        o
    }
}
