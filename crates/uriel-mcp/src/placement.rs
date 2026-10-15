use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use uriel_place::fixed::ONE;
use uriel_place::{
    AccessKind, Config, Event, Lifetime, Placement as Plan, Placer, Region, RegionId, Tier,
};
use serde_json::{json, Value as Json};

use crate::config::PlacementMode;

pub const REGION_IDS: u64 = 16;
pub const MAX_PINS: usize = 4096;
const REGION_BYTES: u64 = 4096;
const HOT_REGIONS: u64 = 64;
const WARM_FACTOR: u64 = 4;
const PREFETCH_REGIONS: u64 = 16;
const PLAN_EVERY: usize = 8;
const MAX_OBSERVED: usize = 10_000;
const MAX_DEPTH: usize = 32;

pub fn region_of(node: u64) -> Result<u32, String> {
    u32::try_from(node / REGION_IDS).map_err(|_| {
        format!(
            "node id {node} is outside the modelled range (below {})",
            REGION_IDS * (u64::from(u32::MAX) + 1)
        )
    })
}

pub fn tier_name(t: Tier) -> &'static str {
    match t {
        Tier::Hot => "hot",
        Tier::Warm => "warm",
        Tier::Cold => "cold",
    }
}

fn lifetime_name(l: Lifetime) -> &'static str {
    match l {
        Lifetime::Ephemeral => "ephemeral",
        Lifetime::Session => "session",
        Lifetime::Durable => "durable",
    }
}

fn fixed(v: i64) -> f64 {
    v as f64 / ONE as f64
}

fn model_config() -> Config {
    let mut config = Config {
        hot_bytes: HOT_REGIONS * REGION_BYTES,
        warm_bytes: HOT_REGIONS * WARM_FACTOR * REGION_BYTES,
        prefetch_bytes: PREFETCH_REGIONS * REGION_BYTES,
        size_unit: REGION_BYTES,
        ..Config::default()
    };
    config.weights.topo = 0;
    config
}

struct Model {
    placer: Placer,
    last_access: BTreeMap<u32, u64>,
    pins: BTreeSet<u32>,
    plan: Plan,
    dirty: bool,
    since_plan: usize,
}

impl Model {
    fn new() -> Model {
        Model {
            placer: Placer::new(model_config(), &[], &[]),
            last_access: BTreeMap::new(),
            pins: BTreeSet::new(),
            plan: Plan::default(),
            dirty: false,
            since_plan: 0,
        }
    }

    fn ensure(&mut self, rid: u32) {
        self.placer.add_region(Region {
            id: RegionId(rid),
            size: REGION_BYTES,
            community: 0,
        });
    }

    fn replan(&mut self) {
        self.plan = self.placer.plan();
        self.dirty = false;
        self.since_plan = 0;
    }

    fn read(&mut self, rid: u32) {
        self.ensure(rid);
        self.last_access.insert(rid, self.placer.clock());
        self.placer.apply(Event::Access {
            region: RegionId(rid),
            kind: AccessKind::Read,
        });
        self.dirty = true;
        self.since_plan += 1;
        if self.since_plan >= PLAN_EVERY {
            self.replan();
        }
    }

    fn link(&mut self, a: u32, b: u32) {
        self.ensure(a);
        self.ensure(b);
        self.placer.add_edge(RegionId(a), RegionId(b));
        self.dirty = true;
    }

    fn pin(&mut self, rid: u32) -> Result<(), String> {
        if !self.pins.contains(&rid) && self.pins.len() >= MAX_PINS {
            return Err(format!("at most {MAX_PINS} regions can be pinned"));
        }
        self.ensure(rid);
        self.placer.apply(Event::Pin {
            region: RegionId(rid),
        });
        self.pins.insert(rid);
        self.replan();
        Ok(())
    }

    fn unpin(&mut self, rid: u32) -> bool {
        let was = self.pins.remove(&rid);
        if was {
            self.placer.apply(Event::Unpin {
                region: RegionId(rid),
            });
            self.replan();
        }
        was
    }

    fn explain(&mut self, rid: u32) -> Json {
        if self.dirty {
            self.replan();
        }
        let id = RegionId(rid);
        let known = self.placer.tier_of(id).is_some();
        json!({
            "tier": tier_name(self.placer.tier_of(id).unwrap_or(Tier::Cold)),
            "known": known,
            "heat": fixed(self.placer.heat(id).unwrap_or(0)),
            "score": self.placer.score(id).map(fixed),
            "topology_weight": fixed(model_config().weights.topo),
            "pinned": self.pins.contains(&rid),
            "last_access": self.last_access.get(&rid),
            "prefetch": self.plan.prefetch.contains(&id),
            "lifetime": self.placer.lifetime(id).map(lifetime_name),
            "clock": self.placer.clock(),
        })
    }
}

pub struct PlaceState {
    mode: PlacementMode,
    persist_dir: Option<PathBuf>,
    model: Mutex<Model>,
}

fn collect(v: &Json, depth: usize, nodes: &mut Vec<u64>, rels: &mut Vec<(u64, u64)>) {
    if depth > MAX_DEPTH || nodes.len() + rels.len() >= MAX_OBSERVED {
        return;
    }
    match v {
        Json::Array(items) => {
            for item in items {
                collect(item, depth + 1, nodes, rels);
            }
        }
        Json::Object(map) => {
            if map.contains_key("_labels") {
                if let Some(id) = map.get("_id").and_then(Json::as_u64) {
                    nodes.push(id);
                }
            } else if let (Some(a), Some(b)) = (
                map.get("_start").and_then(Json::as_u64),
                map.get("_end").and_then(Json::as_u64),
            ) {
                rels.push((a, b));
            }
            for child in map.values() {
                collect(child, depth + 1, nodes, rels);
            }
        }
        _ => {}
    }
}

impl PlaceState {
    pub fn new(mode: PlacementMode, persist_dir: Option<PathBuf>) -> PlaceState {
        PlaceState {
            mode,
            persist_dir,
            model: Mutex::new(Model::new()),
        }
    }

    pub fn mode(&self) -> PlacementMode {
        self.mode
    }

    pub fn persist_dir(&self) -> Option<&Path> {
        self.persist_dir.as_deref()
    }

    fn model(&self) -> MutexGuard<'_, Model> {
        self.model.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn observe_rows(&self, rows: &Json) {
        if self.mode != PlacementMode::Planned {
            return;
        }
        let (mut nodes, mut rels) = (Vec::new(), Vec::new());
        collect(rows, 0, &mut nodes, &mut rels);
        let mut model = self.model();
        for id in nodes {
            if let Ok(rid) = region_of(id) {
                model.read(rid);
            }
        }
        for (a, b) in rels {
            if let (Ok(ra), Ok(rb)) = (region_of(a), region_of(b)) {
                if ra != rb {
                    model.link(ra, rb);
                }
            }
        }
    }

    pub fn observe_chunks(&self, ids: &[u64]) {
        if self.mode != PlacementMode::Planned {
            return;
        }
        let mut model = self.model();
        for id in ids.iter().take(MAX_OBSERVED) {
            if let Ok(rid) = region_of(*id) {
                model.read(rid);
            }
        }
    }

    pub fn explain(&self, rid: u32) -> Json {
        self.model().explain(rid)
    }

    pub fn pin(&self, rid: u32) -> Result<(), String> {
        self.model().pin(rid)
    }

    pub fn unpin(&self, rid: u32) -> bool {
        self.model().unpin(rid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn region_of_groups_sixteen_ids_and_rejects_the_far_range() {
        assert_eq!(region_of(0), Ok(0));
        assert_eq!(region_of(15), Ok(0));
        assert_eq!(region_of(16), Ok(1));
        assert_eq!(region_of(68_719_476_735), Ok(u32::MAX));
        assert!(region_of(68_719_476_736).is_err());
        assert!(region_of(u64::MAX).is_err());
    }

    #[test]
    fn the_pin_table_is_capped() {
        let mut m = Model::new();
        for r in 0..MAX_PINS as u32 {
            assert!(m.pin(r).is_ok());
        }
        assert!(m.pin(MAX_PINS as u32).is_err());
        assert!(m.pin(0).is_ok());
        assert!(m.unpin(1));
        assert!(m.pin(MAX_PINS as u32).is_ok());
    }

    #[test]
    fn an_untouched_region_is_unknown_and_cold() {
        let mut m = Model::new();
        let e = m.explain(9);
        assert_eq!(e["tier"], "cold");
        assert_eq!(e["known"], false);
        assert!(e["last_access"].is_null());
    }

    #[test]
    fn a_read_makes_the_region_known_hot_and_stamps_its_access_index() {
        let mut m = Model::new();
        m.read(3);
        m.read(4);
        m.read(3);
        let e = m.explain(3);
        assert_eq!(e["tier"], "hot");
        assert_eq!(e["last_access"], 2);
        assert_eq!(m.explain(4)["last_access"], 1);
    }

    #[test]
    fn a_linked_neighbour_of_a_hot_region_is_prefetched() {
        let mut m = Model::new();
        m.link(1, 2);
        m.read(1);
        let e = m.explain(2);
        assert_eq!(e["prefetch"], true);
        assert_eq!(e["tier"], "cold");
        assert_eq!(m.explain(1)["prefetch"], false);
    }

    #[test]
    fn rows_are_walked_for_nodes_and_relationships() {
        let rows = json!([
            {"n": {"_id": 40, "_labels": ["F"], "i": 1}},
            {"r": {"_id": 0, "_type": "R", "_start": 40, "_end": 200}},
            {"deep": [[[{"_id": 7, "_labels": []}]]]},
            {"plain": {"_id": 99}}
        ]);
        let (mut nodes, mut rels) = (Vec::new(), Vec::new());
        collect(&rows, 0, &mut nodes, &mut rels);
        assert_eq!(nodes, vec![40, 7]);
        assert_eq!(rels, vec![(40, 200)]);
    }

    #[test]
    fn the_row_walk_stops_at_the_depth_limit() {
        let mut v = json!({"_id": 1, "_labels": []});
        for _ in 0..(MAX_DEPTH + 8) {
            v = json!([v]);
        }
        let (mut nodes, mut rels) = (Vec::new(), Vec::new());
        collect(&v, 0, &mut nodes, &mut rels);
        assert!(nodes.is_empty() && rels.is_empty());
    }

    #[test]
    fn local_mode_observes_nothing() {
        let s = PlaceState::new(PlacementMode::Local, None);
        s.observe_chunks(&[1, 2, 3]);
        s.observe_rows(&json!([{"_id": 1, "_labels": []}]));
        assert_eq!(s.explain(0)["known"], false);
    }
}
