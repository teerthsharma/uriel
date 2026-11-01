use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value as Json};
use uriel_dispatch::afk::{parse_duration, Afk, Kind};
use uriel_dispatch::{Ms, TicketId};

use crate::services::probe_from_env;
use crate::tools::{object, opt_usize, req_str, str_prop, Context, ToolDef, ToolResult};

pub const DEFAULT_WORKERS: usize = 3;

fn now_ms() -> Ms {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

fn log_path(ctx: &Context) -> PathBuf {
    ctx.services
        .root
        .join(".uriel")
        .join("afk")
        .join("session.jsonl")
}

fn owner() -> String {
    ["USER", "USERNAME"]
        .iter()
        .find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))
        .unwrap_or_else(|| "owner".into())
}

fn open_fed(ctx: &Context, now: Ms) -> Result<Afk, String> {
    let mut afk = Afk::open(&log_path(ctx)).map_err(|e| e.to_string())?;
    let snap = probe_from_env().snapshot();
    afk.pressure(now, snap.free_ram_mb, snap.total_ram_mb)
        .map_err(|e| e.to_string())?;
    Ok(afk)
}

fn open_schema() -> Json {
    object(
        json!({
            "duration": str_prop("90m, 2h or 1h30m; between 1m and 24h."),
            "workers": {"type": "integer", "description": "Parallel workers, 1 to 16 (default 3)."}
        }),
        &["duration"],
    )
}

fn add_schema() -> Json {
    object(
        json!({
            "scope": str_prop("One line: what changes, where, and the test that proves it."),
            "goal_id": str_prop("House goal id this ticket carries out.")
        }),
        &["scope"],
    )
}

fn report_schema() -> Json {
    object(
        json!({
            "ticket": {"type": "integer", "description": "Ticket number."},
            "kind": {"type": "string", "enum": ["ready", "done", "ask", "blocked", "failed", "waiting"], "description": "ready: awaiting review; ask: needs the user."},
            "note": str_prop("One line: branch and test result, or the question.")
        }),
        &["ticket", "kind"],
    )
}

fn none() -> Json {
    object(json!({}), &[])
}

pub const TOOLS: &[ToolDef] = &[
    ToolDef {
        name: "dispatch_open",
        title: "Open a dispatch window",
        description: "Open a time window for unattended work: sub-goals become gated tickets run by a limited number of workers. Lists the other dispatch tools.",
        writes: true,
        schema: open_schema,
    },
    ToolDef {
        name: "dispatch_add",
        title: "Add a sub-goal ticket",
        description: "Gate and open a ticket for one sub-goal. Returns actions: a `start` action means start a worker now; none means queued.",
        writes: true,
        schema: add_schema,
    },
    ToolDef {
        name: "dispatch_report",
        title: "Report a ticket",
        description: "Report a worker's outcome. Frees its slot; returns the tickets to start next.",
        writes: true,
        schema: report_schema,
    },
    ToolDef {
        name: "dispatch_status",
        title: "Dispatch status",
        description: "Window left, slots, tickets and their stage.",
        writes: true,
        schema: none,
    },
    ToolDef {
        name: "dispatch_close",
        title: "Close the dispatch window",
        description: "End the window and return what needs the user, what is ready, unfinished and never started. Hides the dispatch tools.",
        writes: true,
        schema: none,
    },
];

pub fn call(ctx: &Context, name: &str, args: &Json) -> ToolResult {
    let now = now_ms();
    match name {
        "dispatch_open" => {
            let ms = parse_duration(req_str(args, "duration")?).map_err(|e| e.to_string())?;
            let workers = opt_usize(args, "workers", DEFAULT_WORKERS)?;
            let workers = u32::try_from(workers).unwrap_or(16);
            let afk = Afk::start(&log_path(ctx), &owner(), ms, workers, now)
                .map_err(|e| e.to_string())?;
            Ok(json!({"open": true, "minutes": ms / 60_000, "workers": workers.clamp(1, 16), "closes": afk.closes()}).to_string())
        }
        "dispatch_add" => {
            let scope = req_str(args, "scope")?;
            let goal = args
                .get("goal_id")
                .and_then(Json::as_str)
                .filter(|g| !g.is_empty());
            let mut afk = open_fed(ctx, now)?;
            let (ticket, actions) = afk.add(now, goal, scope).map_err(|e| e.to_string())?;
            Ok(json!({"ticket": ticket.0, "actions": actions}).to_string())
        }
        "dispatch_report" => {
            let ticket = args
                .get("ticket")
                .and_then(Json::as_u64)
                .ok_or("'ticket' is required and must be a non-negative integer")?;
            let kind = req_str(args, "kind")?;
            let kind = Kind::parse(kind).ok_or_else(|| {
                format!("kind '{kind}' is not one of ready done ask blocked failed waiting")
            })?;
            let note = args.get("note").and_then(Json::as_str).map(str::to_string);
            let mut afk = open_fed(ctx, now)?;
            let actions = afk
                .report(now, TicketId(ticket), kind, note)
                .map_err(|e| e.to_string())?;
            Ok(json!({"actions": actions}).to_string())
        }
        "dispatch_status" => open_fed(ctx, now)?
            .status(now)
            .map(|v| v.to_string())
            .map_err(|e| e.to_string()),
        "dispatch_close" => {
            let afk = Afk::open(&log_path(ctx)).map_err(|e| e.to_string())?;
            let (mut summary, archive) = afk.end(now).map_err(|e| e.to_string())?;
            summary["log"] = json!(archive);
            Ok(summary.to_string())
        }
        _ => Err(format!("unhandled tool '{name}'")),
    }
}
