use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use sysinfo::System;
use uriel_dispatch::afk::{parse_duration, Action, Afk, Kind};
use uriel_dispatch::{Ms, TicketId};

pub const COMMAND_FILE: &str = include_str!("../assets/afk.md");
pub const DEFAULT_WORKERS: u32 = 3;

pub fn default_file() -> PathBuf {
    PathBuf::from(".uriel").join("afk").join("session.jsonl")
}

pub fn now_ms() -> Ms {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

fn owner() -> String {
    ["USER", "USERNAME"]
        .iter()
        .find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))
        .unwrap_or_else(|| "owner".into())
}

fn memory_mb() -> (u64, u64) {
    let mut sys = System::new();
    sys.refresh_memory();
    (
        sys.available_memory() / 1_048_576,
        sys.total_memory() / 1_048_576,
    )
}

fn emit<W: Write>(out: &mut W, v: &Value) -> Result<(), String> {
    writeln!(out, "{v}").map_err(|e| e.to_string())
}

fn acts(a: &[Action]) -> Value {
    serde_json::to_value(a).unwrap_or(Value::Null)
}

fn open_fed(file: &Path, now: Ms, free_total: (u64, u64)) -> Result<Afk, String> {
    let mut afk = Afk::open(file).map_err(|e| e.to_string())?;
    afk.pressure(now, free_total.0, free_total.1)
        .map_err(|e| e.to_string())?;
    Ok(afk)
}

pub fn start<W: Write>(
    out: &mut W,
    file: &Path,
    duration: &str,
    workers: u32,
    now: Ms,
    owner_name: Option<&str>,
) -> Result<(), String> {
    let ms = parse_duration(duration).map_err(|e| e.to_string())?;
    let who = owner_name.map_or_else(owner, str::to_string);
    let afk = Afk::start(file, &who, ms, workers, now).map_err(|e| e.to_string())?;
    emit(
        out,
        &json!({ "started": true, "owner": who, "minutes": ms / 60_000, "workers": workers.clamp(1, 16), "closes": afk.closes(), "file": file }),
    )
}

pub fn add<W: Write>(
    out: &mut W,
    file: &Path,
    goal: Option<&str>,
    text: &str,
    now: Ms,
    mem: (u64, u64),
) -> Result<(), String> {
    let mut afk = open_fed(file, now, mem)?;
    let (ticket, actions) = afk.add(now, goal, text).map_err(|e| e.to_string())?;
    emit(
        out,
        &json!({ "ticket": ticket.0, "actions": acts(&actions) }),
    )
}

pub fn report<W: Write>(
    out: &mut W,
    file: &Path,
    ticket: u64,
    kind: &str,
    note: Option<String>,
    now: Ms,
    mem: (u64, u64),
) -> Result<(), String> {
    let kind = Kind::parse(kind).ok_or_else(|| {
        format!("kind {kind:?} is not one of ready done ask blocked failed waiting")
    })?;
    let mut afk = open_fed(file, now, mem)?;
    let actions = afk
        .report(now, TicketId(ticket), kind, note)
        .map_err(|e| e.to_string())?;
    emit(out, &json!({ "actions": acts(&actions) }))
}

pub fn status<W: Write>(out: &mut W, file: &Path, now: Ms, mem: (u64, u64)) -> Result<(), String> {
    let mut afk = open_fed(file, now, mem)?;
    let v = afk.status(now).map_err(|e| e.to_string())?;
    emit(out, &v)
}

pub fn end<W: Write>(out: &mut W, file: &Path, now: Ms) -> Result<(), String> {
    let afk = Afk::open(file).map_err(|e| e.to_string())?;
    let (summary, archive) = afk.end(now).map_err(|e| e.to_string())?;
    let mut v = summary;
    v["log"] = json!(archive);
    emit(out, &v)
}

pub fn init<W: Write>(out: &mut W, project: bool, home: Option<PathBuf>) -> Result<(), String> {
    let dir = if project {
        PathBuf::from(".claude").join("commands")
    } else {
        home.ok_or("cannot find your home directory; use --project")?
            .join(".claude")
            .join("commands")
    };
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let target = dir.join("afk.md");
    std::fs::write(&target, COMMAND_FILE).map_err(|e| e.to_string())?;
    emit(
        out,
        &json!({ "installed": target, "use": "/afk 2h <goal>" }),
    )
}

pub fn run<W: Write>(
    out: &mut W,
    file: Option<PathBuf>,
    cmd: &crate::AfkCmd,
) -> Result<(), String> {
    let file = file.unwrap_or_else(default_file);
    let now = now_ms();
    match cmd {
        crate::AfkCmd::Start { duration, workers } => {
            start(out, &file, duration, *workers, now, None)
        }
        crate::AfkCmd::Add { goal_id, text } => add(
            out,
            &file,
            goal_id.as_deref(),
            &text.join(" "),
            now,
            memory_mb(),
        ),
        crate::AfkCmd::Report { ticket, kind, note } => {
            report(out, &file, *ticket, kind, note.clone(), now, memory_mb())
        }
        crate::AfkCmd::Status => status(out, &file, now, memory_mb()),
        crate::AfkCmd::End => end(out, &file, now),
        crate::AfkCmd::Init { project } => init(
            out,
            *project,
            std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(PathBuf::from),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: Ms = 1_700_000_000_000;
    const PLENTY: (u64, u64) = (60_000, 64_000);

    fn json_of(out: Vec<u8>) -> Value {
        serde_json::from_slice(&out).unwrap()
    }

    #[test]
    fn the_command_file_in_the_repo_is_the_one_the_binary_installs() {
        let repo = include_str!("../../../.claude/commands/afk.md");
        assert_eq!(repo, COMMAND_FILE);
    }

    #[test]
    fn the_command_file_names_every_subcommand_it_tells_the_model_to_run() {
        for needle in [
            "uriel afk start",
            "uriel afk add",
            "uriel afk report",
            "uriel afk status",
            "uriel afk end",
            "$ARGUMENTS",
            "argument-hint",
        ] {
            assert!(COMMAND_FILE.contains(needle), "{needle}");
        }
    }

    #[test]
    fn start_add_report_status_end_round_trip_through_the_binary_layer() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("s.jsonl");
        let mut o = Vec::new();
        start(&mut o, &f, "2h", 2, T0, Some("me")).unwrap();
        assert_eq!(json_of(o)["minutes"], 120);
        let mut o = Vec::new();
        add(&mut o, &f, Some("G1"), "write the parser", T0 + 1, PLENTY).unwrap();
        let v = json_of(o);
        assert_eq!(v["actions"][0]["do"], "start");
        assert_eq!(v["actions"][0]["goal"], "G1");
        let ticket = v["ticket"].as_u64().unwrap();
        let mut o = Vec::new();
        report(
            &mut o,
            &f,
            ticket,
            "ready",
            Some("branch afk/g1; tests: ok".into()),
            T0 + 2,
            PLENTY,
        )
        .unwrap();
        let mut o = Vec::new();
        status(&mut o, &f, T0 + 3, PLENTY).unwrap();
        assert_eq!(json_of(o)["tickets"][0]["stage"], "PersonReview");
        let mut o = Vec::new();
        end(&mut o, &f, T0 + 4).unwrap();
        let v = json_of(o);
        assert_eq!(v["ready"][0]["result"], "branch afk/g1; tests: ok");
        assert!(!f.exists());
    }

    #[test]
    fn low_memory_queues_the_goal_instead_of_starting_it() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("s.jsonl");
        start(&mut Vec::new(), &f, "1h", 3, T0, Some("me")).unwrap();
        let mut o = Vec::new();
        add(&mut o, &f, None, "build everything", T0 + 1, (500, 16_000)).unwrap();
        assert!(json_of(o)["actions"].as_array().unwrap().is_empty());
    }

    #[test]
    fn bad_input_is_an_error_message_not_a_panic() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("s.jsonl");
        assert!(start(&mut Vec::new(), &f, "soon", 3, T0, Some("me")).is_err());
        assert!(status(&mut Vec::new(), &f, T0, PLENTY)
            .unwrap_err()
            .contains("no afk session"));
        start(&mut Vec::new(), &f, "1h", 3, T0, Some("me")).unwrap();
        assert!(report(&mut Vec::new(), &f, 1, "great", None, T0, PLENTY).is_err());
        assert!(report(&mut Vec::new(), &f, 99, "ready", None, T0, PLENTY)
            .unwrap_err()
            .contains("no ticket"));
        assert!(add(&mut Vec::new(), &f, None, "  ", T0, PLENTY).is_err());
    }

    #[test]
    fn init_writes_the_slash_command_where_claude_code_reads_it() {
        let d = tempfile::tempdir().unwrap();
        let mut o = Vec::new();
        init(&mut o, false, Some(d.path().to_path_buf())).unwrap();
        let p = d.path().join(".claude").join("commands").join("afk.md");
        assert_eq!(std::fs::read_to_string(p).unwrap(), COMMAND_FILE);
        assert!(init(&mut Vec::new(), false, None).is_err());
    }
}
