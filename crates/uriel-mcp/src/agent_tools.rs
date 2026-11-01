use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, PoisonError};

use serde_json::{json, Value as Json};
use uriel_fleet::{Fleet, Kind, Limits, Obs, Status, Want};
use uriel_genius::{Book, Mode, Options, Plan};

use crate::tools::{object, opt_usize, req_str, str_list, str_prop, Context, ToolDef, ToolResult};

pub const FLEET_ENV: &str = "URIEL_FLEET_MAX";
pub const DEFAULT_FLEET_MAX: usize = 8;
pub const MAX_FLEET_MAX: usize = 256;

pub struct Hub {
    fleet: Mutex<Fleet>,
    book: Mutex<Book>,
    ledgers: Mutex<BTreeMap<String, uriel_ledger::Ledger>>,
    dir: Option<PathBuf>,
}

const MAX_LEDGERS: usize = 16;
const LEDGER_DIR: &str = "uriel-ledger";

impl Hub {
    pub fn new(max_concurrent: usize) -> Hub {
        Hub::with_dir(max_concurrent, None)
    }

    pub fn with_dir(max_concurrent: usize, dir: Option<PathBuf>) -> Hub {
        let limits = Limits {
            max_concurrent: max_concurrent.clamp(1, MAX_FLEET_MAX),
            ..Limits::default()
        };
        Hub {
            fleet: Mutex::new(Fleet::new(limits)),
            book: Mutex::new(Book::new()),
            ledgers: Mutex::new(BTreeMap::new()),
            dir,
        }
    }

    pub fn from_env_in(dir: Option<PathBuf>) -> Hub {
        let max = std::env::var(FLEET_ENV)
            .ok()
            .and_then(|v| v.trim().parse::<usize>().ok())
            .unwrap_or(DEFAULT_FLEET_MAX);
        Hub::with_dir(max, dir)
    }

    pub fn ledger_file(&self, name: &str) -> Option<PathBuf> {
        self.dir.as_ref().map(|d| d.join(LEDGER_DIR).join(format!("{name}.json")))
    }

    pub(crate) fn ledgers(&self) -> MutexGuard<'_, BTreeMap<String, uriel_ledger::Ledger>> {
        self.ledgers.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn load_ledger(
        &self,
        map: &mut BTreeMap<String, uriel_ledger::Ledger>,
        name: &str,
        create: bool,
    ) -> Result<(), String> {
        if map.contains_key(name) {
            return Ok(());
        }
        let fresh = uriel_ledger::Ledger::new(name).map_err(|e| fail(e.code(), e))?;
        let from_disk = match self.ledger_file(name) {
            Some(path) if path.is_file() => {
                let bytes = std::fs::read(&path).map_err(|e| fail("io", format!("cannot read ledger: {e}")))?;
                if bytes.len() > 1 << 20 {
                    return Err(fail("malformed", "ledger file is larger than 1 MiB"));
                }
                let raw: Json = serde_json::from_slice(&bytes).map_err(|e| fail("malformed", e))?;
                let ledger = uriel_ledger::Ledger::from_json(&raw).map_err(|e| fail(e.code(), e))?;
                if ledger.name() != name {
                    return Err(fail("malformed", "ledger file name does not match its content"));
                }
                Some(ledger)
            }
            _ => None,
        };
        let ledger = match from_disk {
            Some(l) => l,
            None if create => fresh,
            None => return Err(fail("unknown_ledger", format!("no ledger '{name}': call ledger_add first"))),
        };
        if map.len() >= MAX_LEDGERS {
            return Err(fail("too_many_ledgers", format!("at most {MAX_LEDGERS} ledgers are open")));
        }
        map.insert(name.to_string(), ledger);
        Ok(())
    }

    pub(crate) fn persist_ledger(&self, ledger: &uriel_ledger::Ledger) -> Result<Option<PathBuf>, String> {
        let Some(path) = self.ledger_file(ledger.name()) else { return Ok(None) };
        let dir = path.parent().map(PathBuf::from).unwrap_or_default();
        std::fs::create_dir_all(&dir).map_err(|e| fail("io", format!("cannot create {}: {e}", dir.display())))?;
        let tmp = dir.join(format!("{}.json.tmp{}", ledger.name(), std::process::id()));
        std::fs::write(&tmp, ledger.to_json().to_string()).map_err(|e| fail("io", e))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
        }
        std::fs::rename(&tmp, &path).map_err(|e| fail("io", e))?;
        Ok(Some(path))
    }

    pub fn from_env() -> Hub {
        let max = std::env::var(FLEET_ENV)
            .ok()
            .and_then(|v| v.trim().parse::<usize>().ok())
            .unwrap_or(DEFAULT_FLEET_MAX);
        Hub::new(max)
    }

    pub(crate) fn fleet(&self) -> MutexGuard<'_, Fleet> {
        self.fleet.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn book(&self) -> MutexGuard<'_, Book> {
        self.book.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn plan_schema() -> Json {
    object(
        json!({
            "idea": str_prop("The idea, question or surprising fact to run the hexagon over."),
            "mode": {"type": "string", "enum": ["tier", "fork"],
                "description": "tier (default): one full brief per lens for a smaller model. fork: the shared text once plus six short lens blocks, for forks of this context that share its KV cache."},
            "warm": {"type": "boolean",
                "description": "Run the first lens alone before the rest, so a provider prompt cache holds the shared prefix before the fan-out. Costs one sequential step."}
        }),
        &["idea"],
    )
}

fn brief_schema() -> Json {
    object(
        json!({
            "plan": str_prop("Plan id from genius_plan."),
            "vertex": str_prop("abduce, analogize, question, refute, eliminate or monitor. Omit in fork mode, round 1, for the shared text."),
            "round": {"type": "integer", "enum": [1, 2], "description": "Default 1. Round 2 opens after the sixth round-1 submit."}
        }),
        &["plan"],
    )
}

fn submit_schema() -> Json {
    object(
        json!({
            "plan": str_prop("Plan id from genius_plan."),
            "round": {"type": "integer", "enum": [1, 2], "description": "Optional; must match the open round."},
            "vertex": str_prop("abduce, analogize, question, refute, eliminate or monitor."),
            "claims": {"type": "array", "items": {"type": "object"},
                "description": "The lens's claims, exactly as its brief asks."},
            "unknowns": str_list("What the lens could not determine."),
            "experiment": {"type": "array", "items": {"type": "object"},
                "description": "Results of tests from the verdict's test_queue: {claim, outcome: survived|killed, evidence: [..]}. Send instead of claims, once both rounds are complete."},
            "agent": str_prop("Fleet agent id (from fleet_admit) that produced this; its slot is released and the submission is traced.")
        }),
        &["plan"],
    )
}

fn admit_schema() -> Json {
    object(
        json!({
            "run": str_prop("Run id; use the genius plan id so the trace and the plan line up."),
            "count": {"type": "integer", "minimum": 0, "description": "How many agents you want to launch. Default 1."},
            "max": {"type": "boolean", "description": "Ask for as many as the limits allow."},
            "parent": str_prop("Fleet agent id of the spawning agent, for nested agents."),
            "role": str_prop("What the agents do, e.g. lens."),
            "tier": str_prop("Model tier you will run them on, e.g. small.")
        }),
        &["run"],
    )
}

fn event_schema() -> Json {
    object(
        json!({
            "run": str_prop("Run id."),
            "agent": str_prop("Fleet agent id. Optional only for kind reap."),
            "kind": {"type": "string", "enum": ["llm", "tool", "mark", "end", "reap"],
                "description": "end releases the slot; reap cancels every agent of the run that is still running."},
            "name": str_prop("Model or tool name, or the mark."),
            "tokens_in": {"type": "integer", "minimum": 0},
            "tokens_out": {"type": "integer", "minimum": 0},
            "cached_in": {"type": "integer", "minimum": 0, "description": "Input tokens served from a prompt cache."},
            "t_ms": {"type": "integer", "minimum": 0, "description": "Caller timestamp in milliseconds; stored, never used for decisions."},
            "attrs": {"type": "object", "additionalProperties": {"type": "string"},
                "description": "Up to 8 short string attributes. Secrets and control bytes are removed before storing."},
            "status": {"type": "string", "enum": ["done", "failed", "cancelled"], "description": "For kind end. Default done."},
            "note": str_prop("For kind end.")
        }),
        &["run", "kind"],
    )
}

fn trace_schema() -> Json {
    object(
        json!({
            "run": str_prop("Run id."),
            "format": {"type": "string", "enum": ["summary", "jsonl", "deck"],
                "description": "summary (default): counts, tokens, limits, the chain head and the deck. jsonl: every event, hash-chained. deck: only the observation deck, how many agents to launch now."}
        }),
        &["run"],
    )
}

pub const TOOLS: &[ToolDef] = &[
    ToolDef {
        name: "genius_plan",
        title: "Plan a genius run",
        description: "Compile an idea into the deterministic hexagonal abduction plan: six lenses (Peirce, Koestler and Gentner, Socrates, Popper, Holmes and Bayes, Flavell and Nelson and Narens), the waves to run them in under the agent cap, and the briefs. The same idea always returns the same plan id and bytes.",
        writes: false,
        schema: plan_schema,
    },
    ToolDef {
        name: "genius_brief",
        title: "Get a lens brief",
        description: "Return one brief as plain text, to be used verbatim as an agent's prompt. One brief per call keeps every response under the output limit. In fork mode, round 1, omit vertex for the shared text once and pass vertex for each short lens block.",
        writes: false,
        schema: brief_schema,
    },
    ToolDef {
        name: "genius_submit",
        title: "Submit a lens result",
        description: "Validate and record one lens's claims (round 1 or 2) or a batch of experiment results. Strange ideas are accepted as typed wild claims with a falsifier and a cheap test, never as fact. Returns the round-2 briefs after the sixth round-1 submit, and the verdict (ranked claims, refuted, test_queue, axes, unknowns) after the sixth round-2 submit. Everything is sanitised before it is stored.",
        writes: false,
        schema: submit_schema,
    },
    ToolDef {
        name: "fleet_admit",
        title: "Admit agents",
        description: "Ask to launch agents. Grants as many as the concurrency, depth, lifetime and token limits allow (max=true for all that fit), returns their ids, how many were deferred and which limit bound. The server does not start agents; you do, then report with fleet_event.",
        writes: false,
        schema: admit_schema,
    },
    ToolDef {
        name: "fleet_event",
        title: "Observe an agent",
        description: "Record an agent's model call, tool call, mark or end in the run's hash-chained trace. Control bytes and secrets in names and attributes are removed first; the response says what was removed. kind end frees the slot; reap cancels agents that never reported.",
        writes: false,
        schema: event_schema,
    },
    ToolDef {
        name: "fleet_trace",
        title: "Read a fleet trace",
        description: "The run summary (agents by state, tokens, limits, whether the chain verifies, its head hash) or every event as JSON lines, shaped for translation to an agent-trajectory or OpenTelemetry exporter.",
        writes: false,
        schema: trace_schema,
    },
];

fn fail(code: &str, message: impl std::fmt::Display) -> String {
    json!({"error": code, "message": message.to_string()}).to_string()
}

pub(crate) fn deck_line(fleet: &Fleet, run: &str) -> String {
    fleet.deck(run).map_or_else(|_| "deck: no such run".to_string(), |d| d.line())
}

pub fn call(ctx: &Context, name: &str, args: &Json) -> ToolResult {
    let hub = &ctx.agents;
    match name {
        "genius_plan" => plan(hub, args),
        "genius_brief" => brief(hub, args),
        "genius_submit" => submit(hub, args),
        "fleet_admit" => admit(hub, args),
        "fleet_event" => event(hub, args),
        "fleet_trace" => trace(hub, args),
        other => Err(fail("unknown_tool", other)),
    }
}

fn plan(hub: &Hub, args: &Json) -> ToolResult {
    let idea = req_str(args, "idea")?;
    let mode = match args.get("mode") {
        None | Some(Json::Null) => Mode::Tier,
        Some(Json::String(s)) => Mode::parse(s).ok_or_else(|| "'mode' must be \"tier\" or \"fork\"".to_string())?,
        Some(_) => return Err("'mode' must be \"tier\" or \"fork\"".into()),
    };
    let warm = crate::tools::opt_bool(args, "warm")?.unwrap_or(false);
    let cap = hub.fleet().limits().max_concurrent;
    let plan = Plan::compile(idea, Options { mode, warm, max_concurrent: cap })
        .map_err(|e| fail(e.code(), e))?;
    let mut json = plan.to_json();
    json["limits"] = hub.fleet().limits().to_json();
    let id = plan.id.clone();
    hub.book().open(plan);
    let mut fleet = hub.fleet();
    fleet.seed_backlog(&id, 6).map_err(|e| fail("fleet", e))?;
    json["deck"] = Json::String(deck_line(&fleet, &id));
    Ok(json.to_string())
}

fn brief(hub: &Hub, args: &Json) -> ToolResult {
    let plan = req_str(args, "plan")?;
    let vertex = match args.get("vertex") {
        None | Some(Json::Null) => None,
        Some(Json::String(s)) => Some(
            uriel_genius::Vertex::parse(s)
                .ok_or_else(|| fail("bad_vertex", format!("unknown vertex '{s}'")))?,
        ),
        Some(_) => return Err("'vertex' must be a string".into()),
    };
    let round = match args.get("round") {
        None | Some(Json::Null) => 1,
        Some(v) => v
            .as_u64()
            .and_then(|n| u8::try_from(n).ok())
            .ok_or_else(|| "'round' must be 1 or 2".to_string())?,
    };
    hub.book()
        .brief(plan, vertex, round)
        .map_err(|e| fail(e.code(), e))
}

fn submit(hub: &Hub, args: &Json) -> ToolResult {
    let plan = req_str(args, "plan")?;
    let agent = args.get("agent").and_then(Json::as_str).filter(|a| !a.is_empty());
    let is_claims = args.get("experiment").is_none();
    let outcome = hub.book().submit(plan, args);
    match outcome {
        Ok(done) => {
            let mut json = done.to_json();
            if let Some(agent) = agent.filter(|_| is_claims) {
                json["fleet"] = trace_submission(hub, plan, agent, args, &done.report);
            }
            let mut fleet = hub.fleet();
            if done.round2_ready {
                let _ = fleet.enqueue(plan, 6);
            }
            json["deck"] = Json::String(deck_line(&fleet, plan));
            Ok(json.to_string())
        }
        Err(e) => {
            if let Some(agent) = agent {
                let mut attrs = BTreeMap::new();
                attrs.insert("code".to_string(), e.code().to_string());
                let _ = hub.fleet().observe(
                    plan,
                    agent,
                    Obs { kind: Some(Kind::Mark), name: "submit_rejected".into(), attrs, ..Obs::default() },
                );
            }
            Err(fail(e.code(), e))
        }
    }
}

fn trace_submission(hub: &Hub, plan: &str, agent: &str, args: &Json, report: &uriel_fleet::Report) -> Json {
    let mut attrs = BTreeMap::new();
    attrs.insert("vertex".to_string(), args.get("vertex").and_then(Json::as_str).unwrap_or("").to_string());
    attrs.insert("secrets".to_string(), report.secret_total().to_string());
    attrs.insert("suspect".to_string(), report.suspect().to_string());
    let mut fleet = hub.fleet();
    let marked = fleet.observe(
        plan,
        agent,
        Obs { kind: Some(Kind::Mark), name: "genius_submit".into(), attrs, ..Obs::default() },
    );
    match marked.and_then(|_| fleet.end(plan, agent, Status::Done, "submitted")) {
        Ok(_) => json!({"agent": agent, "released": true}),
        Err(e) => json!({"agent": agent, "released": false, "error": e.to_string()}),
    }
}

fn admit(hub: &Hub, args: &Json) -> ToolResult {
    let run = req_str(args, "run")?;
    let want = if crate::tools::opt_bool(args, "max")?.unwrap_or(false) {
        Want::Max
    } else {
        Want::Count(opt_usize(args, "count", 1)?)
    };
    let parent = args.get("parent").and_then(Json::as_str).filter(|p| !p.is_empty());
    let role = args.get("role").and_then(Json::as_str).unwrap_or("agent");
    let tier = args.get("tier").and_then(Json::as_str).unwrap_or("");
    let mut fleet = hub.fleet();
    let admission = fleet
        .admit(run, want, parent, role, tier)
        .map_err(|e| fail("admit_failed", e))?;
    let mut json = admission.to_json();
    json["limits"] = fleet.limits().to_json();
    json["deck"] = Json::String(deck_line(&fleet, run));
    Ok(json.to_string())
}

fn u64_arg(args: &Json, key: &str) -> Result<u64, String> {
    match args.get(key) {
        None | Some(Json::Null) => Ok(0),
        Some(v) => v.as_u64().ok_or_else(|| format!("'{key}' must be a non-negative integer")),
    }
}

fn event(hub: &Hub, args: &Json) -> ToolResult {
    let run = req_str(args, "run")?;
    let kind = req_str(args, "kind")?;
    let agent = args.get("agent").and_then(Json::as_str).filter(|a| !a.is_empty());
    let mut fleet = hub.fleet();
    if kind == "reap" {
        let n = fleet.reap(run).map_err(|e| fail("reap_failed", e))?;
        return Ok(json!({"run": run, "reaped": n, "deck": deck_line(&fleet, run)}).to_string());
    }
    let agent = agent.ok_or_else(|| "'agent' is required unless kind is reap".to_string())?;
    if kind == "end" {
        let status = args
            .get("status")
            .and_then(Json::as_str)
            .map_or(Some(Status::Done), Status::parse_final)
            .ok_or_else(|| "'status' must be done, failed or cancelled".to_string())?;
        let note = args.get("note").and_then(Json::as_str).unwrap_or("");
        let ev = fleet.end(run, agent, status, note).map_err(|e| fail("event_failed", e))?;
        return Ok(json!({"event": ev.to_json(), "deck": deck_line(&fleet, run)}).to_string());
    }
    let kind = Kind::parse(kind)
        .filter(|k| matches!(k, Kind::Llm | Kind::Tool | Kind::Mark))
        .ok_or_else(|| "'kind' must be llm, tool, mark, end or reap".to_string())?;
    let mut attrs = BTreeMap::new();
    match args.get("attrs") {
        None | Some(Json::Null) => {}
        Some(Json::Object(m)) => {
            for (k, v) in m {
                let s = v.as_str().ok_or_else(|| "'attrs' values must be strings".to_string())?;
                attrs.insert(k.clone(), s.to_string());
            }
        }
        Some(_) => return Err("'attrs' must be an object of strings".into()),
    }
    let obs = Obs {
        kind: Some(kind),
        name: args.get("name").and_then(Json::as_str).unwrap_or("").to_string(),
        tokens_in: u64_arg(args, "tokens_in")?,
        tokens_out: u64_arg(args, "tokens_out")?,
        cached_in: u64_arg(args, "cached_in")?,
        t_ms: args.get("t_ms").and_then(Json::as_u64),
        attrs,
    };
    let (ev, report) = fleet.observe(run, agent, obs).map_err(|e| fail("event_failed", e))?;
    Ok(json!({"event": ev.to_json(), "sanitised": report.to_json(), "deck": deck_line(&fleet, run)}).to_string())
}

fn trace(hub: &Hub, args: &Json) -> ToolResult {
    let run = req_str(args, "run")?;
    let fleet = hub.fleet();
    match args.get("format").and_then(Json::as_str).unwrap_or("summary") {
        "summary" => fleet
            .summary(run)
            .map(|s| s.to_string())
            .map_err(|e| fail("unknown_run", e)),
        "jsonl" => fleet.trace_jsonl(run).map_err(|e| fail("unknown_run", e)),
        "deck" => fleet
            .deck(run)
            .map(|d| d.to_json().to_string())
            .map_err(|e| fail("unknown_run", e)),
        _ => Err("'format' must be \"summary\", \"jsonl\" or \"deck\"".into()),
    }
}
