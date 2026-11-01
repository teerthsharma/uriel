use serde_json::{json, Value as Json};
use uriel_ledger::NewGoal;

use crate::agent_tools::deck_line;
use crate::tools::{object, opt_usize, req_str, str_prop, Context, ToolDef, ToolResult};

fn fail(code: &str, message: impl std::fmt::Display) -> String {
    json!({"error": code, "message": message.to_string()}).to_string()
}

fn add_schema() -> Json {
    object(
        json!({
            "ledger": str_prop("Ledger name (1 to 64 characters from A-Z a-z 0-9 _ . : -). Also the fleet run id its backlog feeds."),
            "goals": {"type": "array", "maxItems": 64, "items": {"type": "object",
                "properties": {
                    "id": str_prop("Goal id."),
                    "title": str_prop("One line: what to do."),
                    "detail": str_prop("Optional context, up to 600 bytes."),
                    "needs": {"type": "array", "maxItems": 16, "items": {"type": "string"}, "description": "Ids of goals that must be done first."}
                },
                "required": ["id", "title"], "additionalProperties": false},
                "description": "Tiny goals with their dependencies. The whole batch is refused if it has a cycle, an unknown dependency or a changed redefinition."}
        }),
        &["ledger", "goals"],
    )
}

fn next_schema() -> Json {
    object(
        json!({
            "ledger": str_prop("Ledger name."),
            "limit": {"type": "integer", "minimum": 0, "description": "How many ready goals to issue. Default: every free agent slot of the fleet run with this name."}
        }),
        &["ledger"],
    )
}

fn done_schema() -> Json {
    object(
        json!({
            "ledger": str_prop("Ledger name."),
            "id": str_prop("Goal id; it must have been issued by ledger_next."),
            "outcome": {"type": "string", "enum": ["done", "failed"]},
            "result": str_prop("One or two sentences. It is passed, fenced, into the prompts of the goals that depend on this one.")
        }),
        &["ledger", "id", "outcome"],
    )
}

pub const TOOLS: &[ToolDef] = &[
    ToolDef {
        name: "ledger_add",
        title: "Add goals to a ledger",
        description: "Keep a bounded graph of tiny dependent goals (at most 256, 16 dependencies each). Order is a topological sort, ties broken by id, so the same ledger always gives the same layers. The ledger is saved next to the database and read by scripts/ledger_next.py.",
        writes: true,
        schema: add_schema,
    },
    ToolDef {
        name: "ledger_next",
        title: "Issue the next goals",
        description: "Issue the ready frontier: goals whose dependencies are all done, earliest layer first. Issued goals are queued as fleet backlog, so the observation deck says how many agents to launch. Prompts for them come from `python3 scripts/ledger_next.py <file> --issued`, cached by content.",
        writes: true,
        schema: next_schema,
    },
    ToolDef {
        name: "ledger_done",
        title: "Finish a goal",
        description: "Record the outcome of an issued goal. A failed goal blocks everything downstream of it; a done goal may open the next layer.",
        writes: true,
        schema: done_schema,
    },
];

pub fn call(ctx: &Context, name: &str, args: &Json) -> ToolResult {
    match name {
        "ledger_add" => add(ctx, args),
        "ledger_next" => next(ctx, args),
        "ledger_done" => done(ctx, args),
        other => Err(fail("unknown_tool", other)),
    }
}

fn goal_of(raw: &Json) -> Result<NewGoal, String> {
    let o = raw.as_object().ok_or_else(|| "each goal must be an object".to_string())?;
    let s = |k: &str| o.get(k).and_then(Json::as_str).unwrap_or("").to_string();
    let needs = match o.get("needs") {
        None | Some(Json::Null) => Vec::new(),
        Some(Json::Array(a)) => a
            .iter()
            .map(|v| v.as_str().map(str::to_string).ok_or_else(|| "'needs' must be strings".to_string()))
            .collect::<Result<_, _>>()?,
        Some(_) => return Err("'needs' must be an array".into()),
    };
    Ok(NewGoal { id: s("id"), title: s("title"), detail: s("detail"), needs })
}

fn add(ctx: &Context, args: &Json) -> ToolResult {
    let name = req_str(args, "ledger")?;
    let goals: Vec<NewGoal> = match args.get("goals") {
        Some(Json::Array(a)) => a.iter().map(goal_of).collect::<Result<_, _>>()?,
        _ => return Err("'goals' is required and must be an array".into()),
    };
    let hub = &ctx.agents;
    let mut map = hub.ledgers();
    hub.load_ledger(&mut map, name, true)?;
    let ledger = map.get_mut(name).ok_or_else(|| fail("internal", "ledger vanished"))?;
    let report = ledger.add(goals).map_err(|e| fail(e.code(), e))?;
    let file = hub.persist_ledger(ledger)?;
    Ok(json!({
        "added": report.added,
        "unchanged": report.unchanged,
        "sanitised": report.sanitised.to_json(),
        "summary": ledger.summary(),
        "file": file.map(|p| p.display().to_string())
    })
    .to_string())
}

fn next(ctx: &Context, args: &Json) -> ToolResult {
    let name = req_str(args, "ledger")?;
    let hub = &ctx.agents;
    let default_limit = {
        let fleet = hub.fleet();
        fleet.deck(name).map_or(fleet.limits().max_concurrent, |d| d.free)
    };
    let limit = opt_usize(args, "limit", default_limit)?;
    let mut map = hub.ledgers();
    hub.load_ledger(&mut map, name, false)?;
    let ledger = map.get_mut(name).ok_or_else(|| fail("internal", "ledger vanished"))?;
    let ids = ledger.issue(limit);
    let file = hub.persist_ledger(ledger)?;
    let issued: Vec<Json> = ids
        .iter()
        .map(|id| json!({"id": id}))
        .collect();
    let summary = ledger.summary();
    drop(map);
    let mut fleet = hub.fleet();
    fleet.enqueue(name, ids.len()).map_err(|e| fail("fleet", e))?;
    let mut out = json!({
        "issued": issued,
        "summary": summary,
        "deck": deck_line(&fleet, name),
        "file": file.as_ref().map(|p| p.display().to_string())
    });
    if let Some(f) = &file {
        out["prompts"] = Json::String(format!("python3 scripts/ledger_next.py {} --issued", f.display()));
    } else {
        out["prompts"] = Json::String("no file: this server has no writable database directory; use the ids and ledger_done".to_string());
    }
    Ok(out.to_string())
}

fn done(ctx: &Context, args: &Json) -> ToolResult {
    let name = req_str(args, "ledger")?;
    let id = req_str(args, "id")?;
    let ok = match args.get("outcome").and_then(Json::as_str) {
        Some("done") => true,
        Some("failed") => false,
        _ => return Err("'outcome' must be \"done\" or \"failed\"".into()),
    };
    let result = args.get("result").and_then(Json::as_str).unwrap_or("");
    let hub = &ctx.agents;
    let mut map = hub.ledgers();
    hub.load_ledger(&mut map, name, false)?;
    let ledger = map.get_mut(name).ok_or_else(|| fail("internal", "ledger vanished"))?;
    let report = ledger.finish(id, ok, result).map_err(|e| fail(e.code(), e))?;
    hub.persist_ledger(ledger)?;
    Ok(json!({"id": id, "sanitised": report.to_json(), "summary": ledger.summary()}).to_string())
}
