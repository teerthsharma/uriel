use std::time::{Duration, Instant};

use uriel_core::TierCounters;
use uriel_place::calibrate::{machine_info, measure, today, Calibration};
use uriel_place::replay::{Player, Scenario};
use uriel_place::{AccessKind, Config, Event, Level, Region, RegionId};
use serde_json::{json, Value as Json};

use crate::config::PlacementMode;
use crate::placement::region_of;
use crate::render::render;
use crate::tools::{format_arg, format_prop, object, Context, ToolDef, ToolResult};

pub const DEFAULT_SECS: f64 = 8.0;
pub const MAX_SECS: f64 = 20.0;
const MIN_SECS: f64 = 0.05;
const MIN_PROBE_BYTES: u64 = 4 << 10;
const MAX_EVENTS: usize = 100_000;
const MAX_REGIONS: u64 = 65_536;
const MAX_REGION_BYTES: u64 = 1 << 40;
const MAX_PLAN_EVERY: u64 = 1 << 20;
const MAX_REPLAY_WORK: u64 = 8_000_000;
const DEFAULT_REGIONS: u64 = 64;
const DEFAULT_REGION_BYTES: u64 = 4096;
const DEFAULT_PLAN_EVERY: u64 = 4;
const NO_PLAN: &str = "no plan exists under --placement local; start the server with --placement planned to get a plan";
const CALIBRATION_FILE: &str = "calibration.json";

fn node_prop() -> Json {
    json!({"type": "integer", "minimum": 0,
        "description": "Node id. Ids are grouped 16 to a region; the region is what the placement plan tiers."})
}

fn calibrate_schema() -> Json {
    object(
        json!({
            "budget_secs": {"type": "number", "exclusiveMinimum": 0, "maximum": 20,
                "description": "Wall-clock budget of the latency staircase in seconds. Default 8, hard maximum 20."},
            "format": format_prop()
        }),
        &[],
    )
}

fn node_schema() -> Json {
    object(
        json!({"node": node_prop(), "format": format_prop()}),
        &["node"],
    )
}

fn event_schema() -> Json {
    json!({
        "type": "object",
        "properties": {
            "op": {"type": "string", "enum": ["read", "write", "scan", "pin", "unpin", "resize", "pressure"]},
            "region": {"type": "integer", "minimum": 0, "maximum": 4294967295u64},
            "size": {"type": "integer", "minimum": 0, "description": "Bytes; required for resize."},
            "level": {"type": "string", "enum": ["normal", "elevated", "high", "critical"],
                "description": "Memory pressure; required for pressure."}
        },
        "required": ["op"],
        "additionalProperties": false
    })
}

fn replay_schema() -> Json {
    object(
        json!({
            "events": {"type": "array", "items": event_schema(), "maxItems": 100000,
                "description": "Placement event log, replayed in order through a fresh placer."},
            "regions": {"type": "integer", "minimum": 0, "maximum": 65536,
                "description": "Regions 0..regions exist, 8 per community. Default 64."},
            "region_size": {"type": "integer", "minimum": 1, "description": "Bytes per region. Default 4096."},
            "hot_bytes": {"type": "integer", "minimum": 0, "description": "Hot budget. Default a quarter of all region bytes."},
            "warm_bytes": {"type": "integer", "minimum": 0, "description": "Warm budget. Default a quarter of all region bytes."},
            "plan_every": {"type": "integer", "minimum": 1, "description": "Take a placement decision every N events. Default 4."},
            "format": format_prop()
        }),
        &["events"],
    )
}

fn format_only_schema() -> Json {
    object(json!({"format": format_prop()}), &[])
}

pub const TOOLS: &[ToolDef] = &[
    ToolDef {
        name: "calibrate",
        title: "Calibrate the memory hierarchy",
        description: "Run the uriel-place dependent-load latency staircase on this machine within budget_secs (default 8, hard maximum 20) and return the latency knees and suggest_hot_bytes. The full measurement is saved as calibration.json next to the database file (not with --in-memory or --read-only).",
        writes: false,
        schema: calibrate_schema,
    },
    ToolDef {
        name: "why_hot",
        title: "Explain a node's placement",
        description: "Explain why the region holding a node id is hot, warm or cold: tier, heat, topology weight, pin flag, last access index and whether it is in the prefetch set. Needs --placement planned; under local it returns tier_stats and a note that no plan exists.",
        writes: false,
        schema: node_schema,
    },
    ToolDef {
        name: "pin",
        title: "Pin a node's region hot",
        description: "Pin the region holding a node id so the placement plan keeps it hot. Needs --placement planned; under local it reports that nothing was applied.",
        writes: true,
        schema: node_schema,
    },
    ToolDef {
        name: "unpin",
        title: "Unpin a node's region",
        description: "Remove the pin on the region holding a node id. Needs --placement planned; under local it reports that nothing was applied.",
        writes: true,
        schema: node_schema,
    },
    ToolDef {
        name: "replay",
        title: "Replay a placement event log",
        description: "Replay a placement event log (read, write, scan, pin, unpin, resize, pressure per region) through a fresh placer and return the SHA-256 digest of the decision sequence plus the final hot, warm and cold region counts. The same log always returns the same digest.",
        writes: false,
        schema: replay_schema,
    },
    ToolDef {
        name: "tier_stats",
        title: "Storage tier statistics",
        description: "Return the graph's tier statistics: hot and warm cache counters, disk reads, gate rejections, and bytes on disk (stored_bytes) against logical bytes with their compression_ratio.",
        writes: false,
        schema: format_only_schema,
    },
];

pub fn call(ctx: &Context, name: &str, args: &Json) -> ToolResult {
    let format = format_arg(args, ctx.format)?;
    let body = match name {
        "calibrate" => calibrate(ctx, args)?,
        "why_hot" => why_hot(ctx, args)?,
        "pin" => pin(ctx, args, true)?,
        "unpin" => pin(ctx, args, false)?,
        "replay" => replay(args)?,
        "tier_stats" => tier_stats_json(ctx)?,
        other => return Err(format!("unknown tool '{other}'")),
    };
    Ok(render(&body, format))
}

fn ceiling_bytes(budget: Duration) -> u64 {
    let secs = budget.as_secs_f64();
    if secs < 0.5 {
        8 << 20
    } else if secs < 2.0 {
        32 << 20
    } else if secs < 6.0 {
        128 << 20
    } else {
        256 << 20
    }
}

pub fn budget(args: &Json) -> Result<Duration, String> {
    let secs = match args.get("budget_secs") {
        None | Some(Json::Null) => DEFAULT_SECS,
        Some(v) => v
            .as_f64()
            .ok_or_else(|| "'budget_secs' must be a number".to_string())?,
    };
    if !secs.is_finite() || secs <= 0.0 {
        return Err("'budget_secs' must be greater than 0".into());
    }
    Ok(Duration::from_secs_f64(secs.clamp(MIN_SECS, MAX_SECS)))
}

fn calibrate(ctx: &Context, args: &Json) -> Result<Json, String> {
    let budget = budget(args)?;
    let started = Instant::now();
    let points = measure(budget, MIN_PROBE_BYTES, ceiling_bytes(budget));
    let took = started.elapsed();
    let mut machine = machine_info();
    machine.insert(
        "budget_seconds".into(),
        format!("{:.3}", budget.as_secs_f64()),
    );
    machine.insert("elapsed_ms".into(), took.as_millis().to_string());
    machine.insert(
        "note".into(),
        "measured on this machine at this moment; not portable".into(),
    );
    let cal = Calibration::from_points(points, machine, today());
    let mut out = json!({
        "budget_secs": budget.as_secs_f64(),
        "elapsed_ms": u64::try_from(took.as_millis()).unwrap_or(u64::MAX),
        "points": cal.points.len(),
        "knees": cal.knees.iter().map(|k| json!({
            "from_bytes": k.from_bytes,
            "to_bytes": k.to_bytes,
            "ratio_x100": k.ratio_x100,
        })).collect::<Vec<Json>>(),
        "suggest_hot_bytes": cal.suggest_hot_bytes(None),
        "persisted": Json::Null,
    });
    if let Some(dir) = ctx.placement.persist_dir() {
        let path = dir.join(CALIBRATION_FILE);
        match std::fs::write(&path, cal.to_json()) {
            Ok(()) => out["persisted"] = json!(path.display().to_string()),
            Err(e) => out["persist_error"] = json!(e.to_string()),
        }
    }
    Ok(out)
}

pub(crate) fn node_arg(args: &Json) -> Result<u64, String> {
    let bad = || "'node' is required and must be a non-negative integer node id".to_string();
    match args.get("node") {
        Some(Json::Number(n)) => n.as_u64().ok_or_else(bad),
        Some(Json::String(s)) => s.trim().parse::<u64>().map_err(|_| bad()),
        _ => Err(bad()),
    }
}

fn counters(c: &TierCounters) -> Json {
    json!({
        "hits": c.hits,
        "misses": c.misses,
        "promotions": c.promotions,
        "evictions": c.evictions,
        "entries": c.entries,
        "bytes": c.bytes,
    })
}

pub(crate) fn tier_stats_json(ctx: &Context) -> Result<Json, String> {
    let s = ctx.db()?.engine.graph().tier_stats().map_err(|e| e.to_string())?;
    Ok(json!({
        "hot": counters(&s.hot),
        "warm": counters(&s.warm),
        "disk_reads": s.disk_reads,
        "bypassed": s.bypassed,
        "gate_rejected": s.gate_rejected,
        "invalidated": s.invalidated,
        "budget_bytes": s.budget_bytes,
        "stored_bytes": s.stored_bytes,
        "logical_bytes": s.logical_bytes,
        "compression_ratio": s.compression_ratio(),
    }))
}

fn why_hot(ctx: &Context, args: &Json) -> Result<Json, String> {
    let node = node_arg(args)?;
    let region = region_of(node)?;
    match ctx.placement.mode() {
        PlacementMode::Local => Ok(json!({
            "mode": "local",
            "node": node,
            "region": region,
            "plan": false,
            "note": NO_PLAN,
            "tier_stats": tier_stats_json(ctx)?,
        })),
        PlacementMode::Planned => {
            let mut out = ctx.placement.explain(region);
            out["mode"] = json!("planned");
            out["node"] = json!(node);
            out["region"] = json!(region);
            Ok(out)
        }
    }
}

fn pin(ctx: &Context, args: &Json, on: bool) -> Result<Json, String> {
    let node = node_arg(args)?;
    let region = region_of(node)?;
    match ctx.placement.mode() {
        PlacementMode::Local => Ok(json!({
            "mode": "local",
            "node": node,
            "region": region,
            "pinned": false,
            "applied": false,
            "note": NO_PLAN,
        })),
        PlacementMode::Planned => {
            if on {
                ctx.placement.pin(region)?;
            } else {
                ctx.placement.unpin(region);
            }
            Ok(json!({
                "mode": "planned",
                "node": node,
                "region": region,
                "pinned": on,
                "applied": true,
            }))
        }
    }
}

fn bounded(args: &Json, key: &str, default: u64, min: u64, max: u64) -> Result<u64, String> {
    match args.get(key) {
        None | Some(Json::Null) => Ok(default),
        Some(v) => v
            .as_u64()
            .filter(|n| (min..=max).contains(n))
            .ok_or_else(|| format!("'{key}' must be an integer from {min} to {max}")),
    }
}

fn level_of(name: &str) -> Option<Level> {
    match name {
        "normal" => Some(Level::Normal),
        "elevated" => Some(Level::Elevated),
        "high" => Some(Level::High),
        "critical" => Some(Level::Critical),
        _ => None,
    }
}

fn parse_event(v: &Json, i: usize) -> Result<Event, String> {
    let obj = v
        .as_object()
        .ok_or_else(|| format!("event {i} must be an object"))?;
    let op = obj
        .get("op")
        .and_then(Json::as_str)
        .ok_or_else(|| format!("event {i} needs a string 'op'"))?;
    if op == "pressure" {
        let level = obj
            .get("level")
            .and_then(Json::as_str)
            .and_then(level_of)
            .ok_or_else(|| {
                format!("event {i}: 'level' must be normal, elevated, high or critical")
            })?;
        return Ok(Event::Pressure { level });
    }
    let region = obj
        .get("region")
        .and_then(Json::as_u64)
        .and_then(|r| u32::try_from(r).ok())
        .map(RegionId)
        .ok_or_else(|| format!("event {i}: 'region' must be an integer from 0 to 4294967295"))?;
    let access = |kind| Ok(Event::Access { region, kind });
    match op {
        "read" => access(AccessKind::Read),
        "write" => access(AccessKind::Write),
        "scan" => access(AccessKind::Scan),
        "pin" => Ok(Event::Pin { region }),
        "unpin" => Ok(Event::Unpin { region }),
        "resize" => {
            let size = obj
                .get("size")
                .and_then(Json::as_u64)
                .ok_or_else(|| format!("event {i}: resize needs an integer 'size'"))?;
            Ok(Event::Resize { region, size })
        }
        other => Err(format!(
            "event {i}: unknown op '{other}' (read, write, scan, pin, unpin, resize, pressure)"
        )),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn replay(args: &Json) -> Result<Json, String> {
    let raw = args
        .get("events")
        .and_then(Json::as_array)
        .ok_or_else(|| "'events' is required and must be an array of events".to_string())?;
    if raw.len() > MAX_EVENTS {
        return Err(format!("a replay takes at most {MAX_EVENTS} events"));
    }
    let regions = bounded(args, "regions", DEFAULT_REGIONS, 0, MAX_REGIONS)?;
    let region_size = bounded(
        args,
        "region_size",
        DEFAULT_REGION_BYTES,
        1,
        MAX_REGION_BYTES,
    )?;
    let quarter = regions * region_size / 4;
    let hot_bytes = bounded(args, "hot_bytes", quarter, 0, u64::MAX)?;
    let warm_bytes = bounded(args, "warm_bytes", quarter, 0, u64::MAX)?;
    let plan_every = bounded(args, "plan_every", DEFAULT_PLAN_EVERY, 1, MAX_PLAN_EVERY)?;
    let plans = raw.len() as u64 / plan_every + 1;
    if plans.saturating_mul(regions.max(1)) > MAX_REPLAY_WORK {
        return Err(format!(
            "{} events over {regions} regions deciding every {plan_every} is too much work; lower regions or raise plan_every",
            raw.len()
        ));
    }
    let events = raw
        .iter()
        .enumerate()
        .map(|(i, v)| parse_event(v, i))
        .collect::<Result<Vec<Event>, String>>()?;
    let scenario = Scenario {
        config: Config {
            hot_bytes,
            warm_bytes,
            ..Config::default()
        },
        regions: (0..regions as u32)
            .map(|i| Region {
                id: RegionId(i),
                size: region_size,
                community: i / 8,
            })
            .collect(),
        edges: Vec::new(),
        events,
        plan_every: plan_every as usize,
    };
    let mut player = Player::new(&scenario);
    player.feed(&scenario.events);
    let ignored = player.placer().ignored();
    let (digest, placements) = player.finish();
    let last = placements
        .last()
        .ok_or_else(|| "the replay produced no decision".to_string())?;
    Ok(json!({
        "digest": hex(&digest),
        "hot": last.hot.len(),
        "warm": last.warm.len(),
        "cold": last.cold.len(),
        "regions": regions,
        "events": scenario.events.len(),
        "plans": placements.len(),
        "ignored": ignored,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(v: Json) -> Result<Duration, String> {
        budget(&json!({"budget_secs": v}))
    }

    #[test]
    fn the_budget_defaults_to_eight_seconds() {
        assert_eq!(budget(&json!({})), Ok(Duration::from_secs(8)));
        assert_eq!(budget(&Json::Null), Ok(Duration::from_secs(8)));
        assert_eq!(secs(Json::Null), Ok(Duration::from_secs(8)));
    }

    #[test]
    fn the_budget_is_capped_at_twenty_seconds() {
        assert_eq!(secs(json!(20)), Ok(Duration::from_secs(20)));
        assert_eq!(secs(json!(25)), Ok(Duration::from_secs(20)));
        assert_eq!(secs(json!(1e308)), Ok(Duration::from_secs(20)));
        assert_eq!(secs(json!(u64::MAX)), Ok(Duration::from_secs(20)));
    }

    #[test]
    fn a_fractional_budget_is_kept() {
        assert_eq!(secs(json!(0.5)), Ok(Duration::from_millis(500)));
    }

    #[test]
    fn a_non_positive_or_non_numeric_budget_is_refused() {
        for bad in [
            json!(0),
            json!(-1),
            json!(-0.0),
            json!("8"),
            json!(true),
            json!([]),
        ] {
            assert!(secs(bad.clone()).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_tiny_budget_is_raised_to_the_floor() {
        assert_eq!(secs(json!(1e-9)), Ok(Duration::from_secs_f64(MIN_SECS)));
    }

    #[test]
    fn node_ids_parse_from_integers_and_strings_only() {
        assert_eq!(node_arg(&json!({"node": 7})), Ok(7));
        assert_eq!(node_arg(&json!({"node": " 7 "})), Ok(7));
        for bad in [
            json!({}),
            json!({"node": -1}),
            json!({"node": 1.5}),
            json!({"node": "x"}),
            json!({"node": null}),
            json!({"node": [1]}),
        ] {
            assert!(node_arg(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn levels_parse_by_name() {
        assert_eq!(level_of("high"), Some(Level::High));
        assert_eq!(level_of("High"), None);
    }

    #[test]
    fn an_expensive_replay_is_refused_before_it_runs() {
        let events: Vec<Json> = (0..50_000)
            .map(|_| json!({"op": "read", "region": 1}))
            .collect();
        let e = replay(&json!({"events": events, "regions": 65536, "plan_every": 1}));
        assert!(e.is_err_and(|m| m.contains("too much work")));
    }

    #[test]
    fn hex_is_lowercase_and_two_digits_per_byte() {
        assert_eq!(hex(&[0, 15, 255]), "000fff");
    }
}
