use uriel_house::{
    Chart, Fact, Fellow, GoalId, GoalStatus, House, Outcome, TestEvidence, TestStatus,
};
use serde_json::{json, Value as Json};

use crate::tools::{
    no_args, object, opt_strings, req_str, str_list, str_prop, Context, ToolDef, ToolResult,
};

fn fellow_schema() -> Json {
    json!({"type": "string", "enum": ["Foreman", "Chase", "Cameron"],
        "description": "The charting fellow."})
}

fn need_schema() -> Json {
    object(
        json!({
            "project": str_prop("Name of the patient (the project)."),
            "goal": str_prop("The Need: what the patient requires to be complete."),
            "acceptance": str_list("Acceptance criteria of the Need.")
        }),
        &["project", "goal", "acceptance"],
    )
}

fn chart_schema() -> Json {
    object(
        json!({
            "fellow": fellow_schema(),
            "chart": {
                "type": "object",
                "description": "The fellow's chart: {need: \"G0\", fellow, goals: [{id, parent, text, leaf, tier, class, needs, writes, reads, creates, test: {name, cmd, class}, est: {ram_mb, secs, tokens}, guards, alt_of}]}. `need` and `fellow` default from the arguments.",
                "properties": {"goals": {"type": "array", "items": {"type": "object"}}},
                "required": ["goals"],
                "additionalProperties": true
            }
        }),
        &["fellow", "chart"],
    )
}

fn fact_schema() -> Json {
    object(
        json!({"fact": {
            "type": "object",
            "description": "A verified fact. An `id` is assigned when absent; a fact that `refutes` a goal kills it and its subtree.",
            "properties": {
                "id": str_prop("Fact id, e.g. F1."),
                "by": str_prop("Who states it. Default Wilson."),
                "refutes": str_prop("Goal id this fact kills."),
                "source": str_prop("file:line or other provenance."),
                "text": str_prop("The fact.")
            },
            "required": ["source", "text"],
            "additionalProperties": false
        }}),
        &["fact"],
    )
}

fn report_schema() -> Json {
    object(
        json!({
            "goal": str_prop("Goal id."),
            "status": {"type": "string", "enum": ["reached", "blocked", "pending"],
                "description": "`reached` is accepted only if the bound test's first recorded run was RED."},
            "evidence": {
                "type": "object",
                "description": "The test run behind the claim. Required for `reached`.",
                "properties": {
                    "test": str_prop("Bound test name."),
                    "status": {"type": "string", "enum": ["red", "green"]},
                    "output": str_prop("Output excerpt.")
                },
                "required": ["test", "status"],
                "additionalProperties": false
            }
        }),
        &["goal", "status"],
    )
}

fn test_run_schema() -> Json {
    object(
        json!({
            "test": str_prop("Name of a bound test."),
            "status": {"type": "string", "enum": ["red", "green"]},
            "secs": {"type": "number", "minimum": 0, "description": "Wall-clock seconds."},
            "peak_ram_mb": {"type": "integer", "minimum": 0, "description": "Measured peak RSS in MB; feeds the cost model."},
            "covered_files": str_list("Files the test covers (COVERS edges).")
        }),
        &["test", "status"],
    )
}

fn leap_schema() -> Json {
    object(
        json!({
            "goal": str_prop("A leap-candidate goal id (see house_status.leap_candidates)."),
            "cause": str_prop("Cause of death of the theory.")
        }),
        &["goal", "cause"],
    )
}

fn select_schema() -> Json {
    object(
        json!({"files": str_list("Files that changed.")}),
        &["files"],
    )
}

pub const TOOLS: &[ToolDef] = &[
    ToolDef {
        name: "house_need",
        title: "State the Need",
        description: "Create a Need: what the patient (project) requires, with acceptance criteria. The first Need is G0; a later Need is N2.G0, N3.G0 and so on, and every goal id of that Need carries the same prefix (N2.G1, N2.G1.1). Several Needs share one database and one resource governor.",
        writes: true,
        schema: need_schema,
    },
    ToolDef {
        name: "house_chart",
        title: "Submit a fellow's chart",
        description: "Validate and store one fellow's chart (Foreman: structure, Chase: failure modes as guards, Cameron: alternative routes). Violations come back by rule so only that fellow re-charts.",
        writes: true,
        schema: chart_schema,
    },
    ToolDef {
        name: "house_merge",
        title: "Merge the charts",
        description: "Merge the stored charts into the one goal tree (backbone, guards, ALT groups, refutations) and validate it.",
        writes: true,
        schema: no_args,
    },
    ToolDef {
        name: "house_fact",
        title: "Record a verified fact",
        description: "Record a fact stated by Wilson. A fact that refutes a goal kills that goal and its subtree at once.",
        writes: true,
        schema: fact_schema,
    },
    ToolDef {
        name: "house_next",
        title: "Next batch",
        description: "The next batch of leaves to dispatch: ready, within the resource governor and free of file conflicts. Dispatched goals become active. Each entry carries its model, prompt pack, test and limits.",
        writes: true,
        schema: no_args,
    },
    ToolDef {
        name: "house_report",
        title: "Report a goal",
        description: "Report a goal's status. `reached` is accepted only when its bound test's first recorded run was RED and the latest is GREEN; otherwise the call errors and the goal stays active.",
        writes: true,
        schema: report_schema,
    },
    ToolDef {
        name: "house_test_run",
        title: "Record a test run",
        description: "Record one run of a bound test (red or green) with its time and peak RAM. Feeds RED-first proof, the result cache, the flake guard and the cost model.",
        writes: true,
        schema: test_run_schema,
    },
    ToolDef {
        name: "house_leap",
        title: "Dr House leap",
        description: "The one leap for a dead subtree. Returns the dispatch (model, fallback_model, effort, timeout) when the goal is a leap candidate; errors on a second leap for the same goal.",
        writes: true,
        schema: leap_schema,
    },
    ToolDef {
        name: "house_status",
        title: "House status",
        description: "A JSON snapshot: the Need, goal counts and list, retirements, pending re-charts, leap candidates, facts.",
        writes: false,
        schema: no_args,
    },
    ToolDef {
        name: "house_select_tests",
        title: "Select tests",
        description: "The tests to run after the given files changed, in order: affected leaf tests, then parent integration tests, most recently failed and cheapest first.",
        writes: false,
        schema: select_schema,
    },
    ToolDef {
        name: "house_config",
        title: "House configuration",
        description: "The model slots and governor settings in effect (config file, then SEAL_DR_HOUSE_MODEL), the project root and the event log.",
        writes: false,
        schema: no_args,
    },
];

fn house(ctx: &Context) -> Result<&House, String> {
    ctx.services.house(ctx.engine.graph())
}

fn to_json<T: serde::Serialize>(v: &T) -> Result<Json, String> {
    serde_json::to_value(v).map_err(|e| e.to_string())
}

fn parse_fellow(s: &str) -> Result<Fellow, String> {
    match s.trim().to_ascii_lowercase().as_str() {
        "foreman" => Ok(Fellow::Foreman),
        "chase" => Ok(Fellow::Chase),
        "cameron" => Ok(Fellow::Cameron),
        other => Err(format!(
            "unknown fellow '{other}': expected Foreman, Chase or Cameron"
        )),
    }
}

fn violations_error(vs: &[uriel_house::Violation]) -> String {
    json!({
        "rejected": true,
        "count": vs.len(),
        "violations": vs,
    })
    .to_string()
}

fn object_arg(args: &Json, key: &str) -> Result<serde_json::Map<String, Json>, String> {
    match args.get(key) {
        Some(Json::Object(m)) => Ok(m.clone()),
        Some(Json::String(s)) => match serde_json::from_str::<Json>(s) {
            Ok(Json::Object(m)) => Ok(m),
            _ => Err(format!("'{key}' must be a JSON object")),
        },
        _ => Err(format!("'{key}' is required and must be an object")),
    }
}

fn goal_arg(args: &Json) -> Result<GoalId, String> {
    req_str(args, "goal").map(|s| GoalId::new(s.trim()))
}

pub fn call(ctx: &Context, name: &str, args: &Json) -> ToolResult {
    match name {
        "house_config" => config(ctx),
        "house_need" => need(ctx, args),
        "house_chart" => chart(ctx, args),
        "house_merge" => merge(ctx),
        "house_fact" => fact(ctx, args),
        "house_next" => next(ctx),
        "house_report" => report(ctx, args),
        "house_test_run" => test_run(ctx, args),
        "house_leap" => leap(ctx, args),
        "house_status" => house(ctx)?
            .status()
            .map(|v| v.to_string())
            .map_err(|e| e.to_string()),
        "house_select_tests" => select_tests(ctx, args),
        other => Err(format!("unknown tool '{other}'")),
    }
}

fn config(ctx: &Context) -> ToolResult {
    let s = &ctx.services;
    let mut cfg = to_json(&s.house_cfg)?;
    if let Some(obj) = cfg.as_object_mut() {
        obj.insert("project_root".into(), json!(s.root.display().to_string()));
    }
    Ok(json!({
        "models": to_json(&s.house_cfg.models)?,
        "governor": {
            "reserve_floor_mb": s.house_cfg.reserve_floor_mb,
            "reserve_pct": s.house_cfg.reserve_pct,
            "heavy_cap": s.house_cfg.heavy_cap,
            "max_agents": s.house_cfg.max_agents,
            "cargo_jobs": s.house_cfg.cargo_jobs,
            "max_rechart": s.house_cfg.max_rechart,
            "cost_window": s.house_cfg.cost_window,
            "alt_secs_weight": s.house_cfg.alt_secs_weight,
            "allow_full_suite": s.house_cfg.allow_full_suite,
        },
        "settings": cfg,
        "root": s.root.display().to_string(),
        "events": s.events.display().to_string(),
        "config_file": s.config_source.as_ref().map(|p| p.display().to_string()),
        "dr_house_env": uriel_house::config::DR_HOUSE_ENV,
    })
    .to_string())
}

fn need(ctx: &Context, args: &Json) -> ToolResult {
    let project = req_str(args, "project")?;
    let goal = req_str(args, "goal")?;
    let acceptance = opt_strings(args, "acceptance")?;
    let id = house(ctx)?
        .need(project, goal, &acceptance)
        .map_err(|e| e.to_string())?;
    Ok(
        json!({"goal": id.as_str(), "project": project, "acceptance": acceptance.len()})
            .to_string(),
    )
}

fn chart(ctx: &Context, args: &Json) -> ToolResult {
    let fellow = parse_fellow(req_str(args, "fellow")?)?;
    let mut obj = object_arg(args, "chart")?;
    obj.entry("need").or_insert_with(|| json!("G0"));
    let signed = obj
        .get("fellow")
        .and_then(Json::as_str)
        .and_then(|s| parse_fellow(s).ok())
        .unwrap_or(fellow);
    obj.insert("fellow".into(), json!(signed.name()));
    let chart: Chart =
        serde_json::from_value(Json::Object(obj)).map_err(|e| format!("invalid chart: {e}"))?;
    let goals = chart.goals.len();
    house(ctx)?
        .submit_chart(fellow, chart)
        .map_err(|vs| violations_error(&vs))?;
    Ok(json!({"accepted": true, "fellow": fellow.name(), "goals": goals}).to_string())
}

fn merge(ctx: &Context) -> ToolResult {
    let report = house(ctx)?.merge().map_err(|vs| violations_error(&vs))?;
    Ok(to_json(&report)?.to_string())
}

fn fact(ctx: &Context, args: &Json) -> ToolResult {
    let mut obj = object_arg(args, "fact")?;
    let house = house(ctx)?;
    if !obj.contains_key("id") {
        let known = house
            .status()
            .map_err(|e| e.to_string())?
            .get("facts")
            .and_then(Json::as_array)
            .map_or(0, Vec::len);
        obj.insert("id".into(), json!(format!("F{}", known + 1)));
    }
    obj.entry("by").or_insert_with(|| json!("Wilson"));
    let fact: Fact =
        serde_json::from_value(Json::Object(obj)).map_err(|e| format!("invalid fact: {e}"))?;
    let id = fact.id.clone();
    house.add_fact(fact).map_err(|e| e.to_string())?;
    Ok(json!({"recorded": true, "id": id}).to_string())
}

fn next(ctx: &Context) -> ToolResult {
    let batch = house(ctx)?.next().map_err(|e| e.to_string())?;
    Ok(to_json(&batch)?.to_string())
}

fn parse_status<T: serde::de::DeserializeOwned>(args: &Json, key: &str) -> Result<T, String> {
    let s = req_str(args, key)?.trim().to_ascii_lowercase();
    serde_json::from_value(Json::String(s.clone()))
        .map_err(|_| format!("'{key}' has an unknown value '{s}'"))
}

fn report(ctx: &Context, args: &Json) -> ToolResult {
    let goal = goal_arg(args)?;
    let status: GoalStatus = parse_status(args, "status")?;
    let evidence = match args.get("evidence") {
        None | Some(Json::Null) if status == GoalStatus::Reached => {
            return Err("'evidence' is required to report a goal as reached".into())
        }
        None | Some(Json::Null) => TestEvidence {
            test: String::new(),
            status: TestStatus::Green,
            output: String::new(),
        },
        Some(v) => {
            let mut obj = object_arg(&json!({"evidence": v}), "evidence")?;
            if let Some(s) = obj.get("status").and_then(Json::as_str) {
                let lower = s.to_ascii_lowercase();
                obj.insert("status".into(), json!(lower));
            }
            serde_json::from_value(Json::Object(obj))
                .map_err(|e| format!("invalid evidence: {e}"))?
        }
    };
    let out = house(ctx)?
        .report(&goal, status, evidence)
        .map_err(|e| e.to_string())?;
    let body = to_json(&out)?.to_string();
    match out.outcome {
        Outcome::Accepted | Outcome::Recorded => Ok(body),
        Outcome::Unproven | Outcome::Rejected => Err(body),
    }
}

fn test_run(ctx: &Context, args: &Json) -> ToolResult {
    let test = req_str(args, "test")?;
    let status: TestStatus = parse_status(args, "status")?;
    let secs = match args.get("secs") {
        None | Some(Json::Null) => 0.0,
        Some(v) => v
            .as_f64()
            .filter(|s| s.is_finite() && *s >= 0.0)
            .ok_or("'secs' must be a non-negative number")?,
    };
    let ram = match args.get("peak_ram_mb") {
        None | Some(Json::Null) => 0,
        Some(v) => v
            .as_u64()
            .ok_or("'peak_ram_mb' must be a non-negative integer")?,
    };
    let covered = opt_strings(args, "covered_files")?;
    house(ctx)?
        .record_test_run(test, status, secs, ram, &covered)
        .map_err(|e| e.to_string())?;
    Ok(json!({"recorded": true, "test": test, "status": status.name()}).to_string())
}

fn leap(ctx: &Context, args: &Json) -> ToolResult {
    let goal = goal_arg(args)?;
    let cause = req_str(args, "cause")?;
    match house(ctx)?.leap(&goal, cause).map_err(|e| e.to_string())? {
        Some(dispatch) => Ok(to_json(&dispatch)?.to_string()),
        None => Ok(json!({
            "leap": false,
            "goal": goal.as_str(),
            "reason": "the goal is not a leap candidate: only a goal left dead after its re-charts qualifies",
        })
        .to_string()),
    }
}

fn select_tests(ctx: &Context, args: &Json) -> ToolResult {
    let files = opt_strings(args, "files")?;
    if files.is_empty() {
        return Err("'files' must name at least one file".into());
    }
    let picks = house(ctx)?
        .select_tests(&files)
        .map_err(|e| e.to_string())?;
    Ok(to_json(&picks)?.to_string())
}
