use std::ffi::OsStr;
use std::fmt::Display;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::time::Duration;

const MIN_PROBE_BYTES: u64 = 4 << 10;
const MAX_PROBE_BYTES: u64 = 1 << 28;
const MIN_CALIBRATE_SECS: f64 = 0.05;
const MAX_CALIBRATE_SECS: f64 = 20.0;
const LOW_RAM_BYTES: u64 = 1 << 30;
const LOCK_MEMORY_PRIVILEGE: &str = "SeLockMemoryPrivilege";

pub fn say<W: Write>(out: &mut W, text: impl Display) -> Result<(), String> {
    writeln!(out, "{text}").map_err(|e| format!("write failed: {e}"))
}

pub fn fmt_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn ensure_parent(db: &Path) -> Result<(), String> {
    match db.parent().filter(|d| !d.as_os_str().is_empty()) {
        Some(dir) => std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display())),
        None => Ok(()),
    }
}

pub fn index<W: Write>(out: &mut W, path: &Path, db: &Path) -> Result<(), String> {
    if !path.is_dir() {
        return Err(format!("{} is not a directory", path.display()));
    }
    ensure_parent(db)?;
    let graph = uriel_core::Graph::open(db, uriel_core::MemoryBudget::mib(uriel_mcp::config::DEFAULT_MEMORY_MIB))
        .map_err(|e| format!("cannot open database {}: {e}", db.display()))?;
    let rag = uriel_rag::Rag::open(graph, uriel_rag::RagConfig::default()).map_err(|e| e.to_string())?;
    let report = rag
        .index_dir(path, &uriel_rag::IndexOptions::default())
        .map_err(|e| e.to_string())?;
    let stats = rag.stats().map_err(|e| e.to_string())?;
    say(out, format!("database: {}", db.display()))?;
    say(out, format!("files scanned: {}", report.files_scanned))?;
    say(out, format!("files indexed: {}", report.files_indexed))?;
    say(out, format!("files unchanged: {}", report.files_unchanged))?;
    say(out, format!("files skipped: {}", report.files_skipped))?;
    say(out, format!("files removed: {}", report.files_removed))?;
    say(out, format!("chunks added: {}", report.chunks_added))?;
    say(out, format!("chunks removed: {}", report.chunks_removed))?;
    say(out, format!("total: {} chunks in {} files", stats.chunks, stats.files))
}

pub fn calibrate<W: Write>(out: &mut W, secs: f64) -> Result<(), String> {
    let secs = if secs.is_finite() {
        secs.clamp(MIN_CALIBRATE_SECS, MAX_CALIBRATE_SECS)
    } else {
        MIN_CALIBRATE_SECS
    };
    say(out, format!("calibrating for {secs:.2} s"))?;
    out.flush().map_err(|e| format!("write failed: {e}"))?;
    let points = uriel_place::calibrate::measure(
        Duration::from_secs_f64(secs),
        MIN_PROBE_BYTES,
        MAX_PROBE_BYTES,
    );
    let cal = uriel_place::calibrate::Calibration::from_points(
        points,
        uriel_place::calibrate::machine_info(),
        uriel_place::calibrate::today(),
    );
    say(out, format!("points measured: {}", cal.points.len()))?;
    if cal.knees.is_empty() {
        say(out, "knees: none detected")?;
    }
    for k in &cal.knees {
        say(
            out,
            format!(
                "knee: {} -> {} (x{}.{:02})",
                fmt_bytes(k.from_bytes),
                fmt_bytes(k.to_bytes),
                k.ratio_x100 / 100,
                k.ratio_x100 % 100
            ),
        )?;
    }
    let hot = cal.suggest_hot_bytes(None);
    say(out, format!("suggested hot size: {} ({hot} bytes)", fmt_bytes(hot)))
}

fn verdict<W: Write>(out: &mut W, pass: bool, label: &str, detail: &str) -> Result<(), String> {
    let tag = if pass { "PASS" } else { "WARN" };
    say(out, format!("{tag}  {label}: {detail}"))
}

pub fn check_db_writable(db: &Path) -> Result<(), String> {
    ensure_parent(db)?;
    let dir = match db.parent().filter(|d| !d.as_os_str().is_empty()) {
        Some(d) => d.to_path_buf(),
        None => PathBuf::from("."),
    };
    let probe = dir.join(format!(".uriel-doctor-{}", std::process::id()));
    std::fs::write(&probe, b"ok").map_err(|e| format!("cannot write in {}: {e}", dir.display()))?;
    std::fs::remove_file(&probe).map_err(|e| format!("cannot remove {}: {e}", probe.display()))
}

pub fn lock_memory_state(whoami_priv_output: &str) -> Option<bool> {
    whoami_priv_output
        .lines()
        .find(|l| l.contains(LOCK_MEMORY_PRIVILEGE))
        .map(|l| l.contains("Enabled"))
}

fn lock_memory_report() -> (bool, String) {
    match Process::new("whoami").arg("/priv").output() {
        Ok(o) if o.status.success() => {
            match lock_memory_state(&String::from_utf8_lossy(&o.stdout)) {
                Some(true) => (true, format!("{LOCK_MEMORY_PRIVILEGE} present and enabled")),
                Some(false) => (
                    true,
                    format!("{LOCK_MEMORY_PRIVILEGE} present, disabled in this token"),
                ),
                None => (false, format!("{LOCK_MEMORY_PRIVILEGE} not granted to this user")),
            }
        }
        _ => (false, "whoami /priv unavailable".to_string()),
    }
}

pub fn doctor<W: Write>(out: &mut W, db: &Path) -> Result<(), String> {
    match check_db_writable(db) {
        Ok(()) => verdict(out, true, "db path", &format!("{} is writable", db.display()))?,
        Err(e) => verdict(out, false, "db path", &e)?,
    }
    let mut system = sysinfo::System::new();
    system.refresh_memory();
    let free = system.available_memory();
    verdict(
        out,
        free >= LOW_RAM_BYTES,
        "free RAM",
        &format!("{} available of {}", fmt_bytes(free), fmt_bytes(system.total_memory())),
    )?;
    let (ok, detail) = lock_memory_report();
    verdict(out, ok, "lock memory privilege", &detail)
}

pub fn install_args(exe: &Path, db: &Path) -> Vec<String> {
    ["mcp", "add", "uriel", "-s", "user", "--"]
        .iter()
        .map(|s| (*s).to_string())
        .chain([
            exe.display().to_string(),
            "mcp".to_string(),
            "--db".to_string(),
            db.display().to_string(),
        ])
        .collect()
}

pub fn install_command_line(exe: &Path, db: &Path) -> String {
    let parts: Vec<String> = install_args(exe, db)
        .into_iter()
        .map(|a| {
            if a.contains(char::is_whitespace) {
                format!("\"{a}\"")
            } else {
                a
            }
        })
        .collect();
    format!("claude {}", parts.join(" "))
}

pub fn find_on_path(name: &str, path_var: &OsStr, exts: &[&str]) -> Option<PathBuf> {
    std::env::split_paths(path_var).find_map(|dir| {
        exts.iter()
            .map(|ext| dir.join(format!("{name}{ext}")))
            .find(|candidate| candidate.is_file())
    })
}

fn executable_extensions() -> &'static [&'static str] {
    if cfg!(windows) {
        &[".exe", ".cmd", ".bat"]
    } else {
        &[""]
    }
}

pub fn install<W: Write>(out: &mut W, exe: &Path, db: &Path) -> Result<(), String> {
    let found = std::env::var_os("PATH")
        .and_then(|p| find_on_path("claude", &p, executable_extensions()));
    let Some(claude) = found else {
        return say(out, install_command_line(exe, db));
    };
    out.flush().map_err(|e| format!("write failed: {e}"))?;
    let status = Process::new(&claude)
        .args(install_args(exe, db))
        .status()
        .map_err(|e| format!("cannot run {}: {e}", claude.display()))?;
    if status.success() {
        say(out, "registered uriel with claude")
    } else {
        Err(format!("claude exited with {status}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_command_line_is_the_exact_claude_invocation() {
        let line = install_command_line(Path::new("uriel"), Path::new("g.seal"));
        assert_eq!(line, "claude mcp add uriel -s user -- uriel mcp --db g.seal");
    }

    #[test]
    fn install_command_line_quotes_paths_with_spaces() {
        let line = install_command_line(Path::new("C:\\Program Files\\uriel.exe"), Path::new("C:\\Users\\a b\\.seal\\graph.seal"));
        assert_eq!(
            line,
            "claude mcp add uriel -s user -- \"C:\\Program Files\\uriel.exe\" mcp --db \"C:\\Users\\a b\\.seal\\graph.seal\""
        );
    }

    #[test]
    fn install_args_end_with_the_exe_mcp_and_the_db() {
        let args = install_args(Path::new("x"), Path::new("y.seal"));
        assert_eq!(args[..6], ["mcp", "add", "uriel", "-s", "user", "--"]);
        assert_eq!(args[6..], ["x", "mcp", "--db", "y.seal"]);
    }

    #[test]
    fn find_on_path_returns_the_first_matching_extension() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("claude.cmd"), "").expect("write");
        let path_var = std::env::join_paths([dir.path()]).expect("join");
        let found = find_on_path("claude", &path_var, &[".exe", ".cmd"]);
        assert_eq!(found, Some(dir.path().join("claude.cmd")));
    }

    #[test]
    fn find_on_path_is_none_when_absent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path_var = std::env::join_paths([dir.path()]).expect("join");
        assert_eq!(find_on_path("claude", &path_var, &[".exe", ".cmd"]), None);
    }

    #[test]
    fn lock_memory_state_reads_the_whoami_table() {
        let enabled = "SeLockMemoryPrivilege   Lock pages in memory   Enabled\r\n";
        let disabled = "SeLockMemoryPrivilege   Lock pages in memory   Disabled\r\n";
        let other = "SeShutdownPrivilege   Shut down the system   Disabled\r\n";
        assert_eq!(lock_memory_state(enabled), Some(true));
        assert_eq!(lock_memory_state(disabled), Some(false));
        assert_eq!(lock_memory_state(other), None);
        assert_eq!(lock_memory_state(""), None);
    }

    #[test]
    fn fmt_bytes_scales_units() {
        assert_eq!(fmt_bytes(512), "512 B");
        assert_eq!(fmt_bytes(2048), "2.0 KiB");
        assert_eq!(fmt_bytes(3 << 20), "3.0 MiB");
        assert_eq!(fmt_bytes(5 << 30), "5.0 GiB");
    }

    #[test]
    fn db_writable_check_passes_in_a_fresh_directory_and_leaves_no_probe() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("nested").join("graph.seal");
        assert!(check_db_writable(&db).is_ok());
        let leftovers = std::fs::read_dir(dir.path().join("nested")).expect("read_dir").count();
        assert_eq!(leftovers, 0);
    }

    #[test]
    fn doctor_prints_one_verdict_line_per_check() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut out = Vec::new();
        doctor(&mut out, &dir.path().join("graph.seal")).expect("doctor");
        let text = String::from_utf8(out).expect("utf8");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines.iter().all(|l| l.starts_with("PASS") || l.starts_with("WARN")));
        assert!(lines[0].starts_with("PASS"));
    }

    #[test]
    fn index_reports_counts_for_a_small_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).expect("mkdir");
        std::fs::write(src.join("a.txt"), "alpha beta gamma delta").expect("write");
        let mut out = Vec::new();
        index(&mut out, &src, &dir.path().join("graph.seal")).expect("index");
        let text = String::from_utf8(out).expect("utf8");
        assert!(text.contains("files indexed: 1"));
        assert!(text.contains("in 1 files"));
    }

    #[test]
    fn index_rejects_a_missing_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut out = Vec::new();
        let result = index(&mut out, &dir.path().join("absent"), &dir.path().join("g.seal"));
        assert!(result.is_err());
    }
}
