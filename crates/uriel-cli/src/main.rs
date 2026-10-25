#![forbid(unsafe_code)]

mod banner;
mod capture;
mod commands;
mod recall;

use std::io::{BufRead, BufReader, BufWriter, IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand, ValueEnum};
use uriel_core::MemoryBudget;
use uriel_mcp::config::{
    default_db_path, Config, Format, Mode as ServerMode, PlacementMode, DEFAULT_IDLE_RELEASE_SECS, DEFAULT_MEMORY_MIB,
    DEFAULT_TOKEN_LIMIT,
    Terse, MODE_ENV, TERSE_ENV,
};

use banner::{mode_from_env, render_banner, Mode};

const HOOKS_HELP: &str = "Claude Code hooks (never block a session, always exit 0):
  uriel capture --db <path>
      PreCompact and SessionEnd: reads the hook payload on stdin, streams the
      transcript and stores it in the graph as ~600-token fractions
      (ctx/<session>/<n>.md), so it stays searchable after compaction.
  uriel recall-context --db <path> [--budget 1200] [--capture]
      SessionStart (matcher compact): prints the hook JSON with the gists most
      relevant to the session; --capture also stores the compaction summary
      first, under one database open.
Settings block: docs/claude-code-hooks.md

Searching captured context from a shell (read-only, never a hook):
  uriel recall --db <path> [--budget 1200] [--k 8] <query words...>
      Prints `// file:start-end #chunk` citations with the resolution ladder
      (top hit at L1, then L2, then one-line L3 gists) under the token budget.
  uriel recall --db <path> --expand <chunk_id>
      Prints the whole stored text of one cited chunk.";

#[derive(Debug, Parser, PartialEq)]
#[command(name = "uriel", disable_version_flag = true, about = "uriel: fewest tokens in, same information out", after_help = HOOKS_HELP)]
struct Cli {
    #[arg(short = 'V', long = "version")]
    version: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand, PartialEq)]
enum Command {
    Banner,
    Mcp(McpArgs),
    Index(IndexArgs),
    Calibrate(CalibrateArgs),
    Doctor(DoctorArgs),
    Install,
    Capture(CaptureArgs),
    RecallContext(RecallArgs),
    Recall(SearchArgs),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum FormatArg {
    Compact,
    Json,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ModeArg {
    Normal,
    House,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum TerseArg {
    Off,
    Data,
    Prose,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum PlacementArg {
    Local,
    Planned,
}

#[derive(Debug, Args, PartialEq)]
struct McpArgs {
    #[arg(long)]
    db: Option<PathBuf>,
    #[arg(long)]
    root: Option<PathBuf>,
    #[arg(long)]
    read_only: bool,
    #[arg(long, value_enum)]
    format: Option<FormatArg>,
    #[arg(long, value_enum)]
    placement: Option<PlacementArg>,
    #[arg(long, value_enum)]
    mode: Option<ModeArg>,
    #[arg(long, value_enum)]
    terse: Option<TerseArg>,
    #[arg(long, value_parser = clap::value_parser!(u64).range(16..))]
    memory_mib: Option<u64>,
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    token_limit: Option<u64>,
    #[arg(long)]
    idle_release_secs: Option<u64>,
}

#[derive(Debug, Args, PartialEq)]
struct IndexArgs {
    path: PathBuf,
    #[arg(long)]
    db: Option<PathBuf>,
}

#[derive(Debug, Args, PartialEq)]
struct CalibrateArgs {
    #[arg(long, default_value_t = 4.0)]
    secs: f64,
}

#[derive(Debug, Args, PartialEq)]
struct DoctorArgs {
    #[arg(long)]
    db: Option<PathBuf>,
}

#[derive(Debug, Args, PartialEq)]
struct CaptureArgs {
    #[arg(long)]
    db: Option<PathBuf>,
    #[arg(long)]
    session: Option<String>,
    #[arg(long)]
    transcript: Option<PathBuf>,
}

#[derive(Debug, Args, PartialEq)]
struct RecallArgs {
    #[arg(long)]
    db: Option<PathBuf>,
    #[arg(long, default_value_t = recall::DEFAULT_BUDGET)]
    budget: usize,
    #[arg(long)]
    capture: bool,
}

#[derive(Debug, Args, PartialEq)]
struct SearchArgs {
    #[arg(long)]
    db: Option<PathBuf>,
    #[arg(long, default_value_t = recall::DEFAULT_BUDGET)]
    budget: usize,
    #[arg(long, default_value_t = recall::DEFAULT_K)]
    k: usize,
    #[arg(long)]
    expand: Option<u64>,
    #[arg(num_args = 1.., required_unless_present = "expand", conflicts_with = "expand")]
    query: Vec<String>,
}

impl McpArgs {
    fn to_config(&self) -> Result<Config, String> {
        self.to_config_envs(
            std::env::var(MODE_ENV).ok().as_deref(),
            std::env::var(TERSE_ENV).ok().as_deref(),
        )
    }

    #[cfg(test)]
    fn to_config_env(&self, mode_env: Option<&str>) -> Result<Config, String> {
        self.to_config_envs(mode_env, None)
    }

    fn to_config_envs(&self, mode_env: Option<&str>, terse_env: Option<&str>) -> Result<Config, String> {
        let terse_flag = self.terse.map(|t| match t {
            TerseArg::Off => Terse::Off,
            TerseArg::Data => Terse::Data,
            TerseArg::Prose => Terse::Prose,
        });
        let mode_flag = self.mode.map(|m| match m {
            ModeArg::Normal => ServerMode::Normal,
            ModeArg::House => ServerMode::House,
        });
        Ok(Config {
            db: self.db.clone().unwrap_or_else(default_db_path),
            read_only: self.read_only,
            in_memory: false,
            token_limit: self
                .token_limit
                .map_or(DEFAULT_TOKEN_LIMIT, |n| usize::try_from(n).unwrap_or(usize::MAX)),
            root: self
                .root
                .clone()
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))),
            events: None,
            config: None,
            format: match self.format {
                Some(FormatArg::Json) => Format::Json,
                Some(FormatArg::Compact) | None => Format::Compact,
            },
            placement: match self.placement {
                Some(PlacementArg::Planned) => PlacementMode::Planned,
                Some(PlacementArg::Local) | None => PlacementMode::Local,
            },
            memory: MemoryBudget::mib(
                self.memory_mib
                    .map_or(DEFAULT_MEMORY_MIB, |m| usize::try_from(m).unwrap_or(DEFAULT_MEMORY_MIB)),
            ),
            mode: ServerMode::resolve(mode_flag, mode_env)?,
            terse: Terse::resolve(terse_flag, terse_env)?,
            idle_release_secs: self.idle_release_secs.unwrap_or(DEFAULT_IDLE_RELEASE_SECS),
        })
    }
}

fn dispatch<R: BufRead + Send + 'static, W: Write>(
    cli: Cli,
    mode: Mode,
    input: R,
    out: &mut W,
) -> Result<(), String> {
    if let Some(Command::Mcp(args)) = &cli.command {
        return uriel_mcp::run(&args.to_config()?, input, &mut *out);
    }
    if cli.version {
        commands::say(out, render_banner(mode))?;
        return commands::say(out, format!("uriel {}", env!("CARGO_PKG_VERSION")));
    }
    match cli.command {
        None | Some(Command::Banner) | Some(Command::Mcp(_)) => {
            commands::say(out, render_banner(mode))
        }
        Some(Command::Capture(args)) => {
            let db = args.db.unwrap_or_else(default_db_path);
            capture::run(&db, args.session, args.transcript, input);
            Ok(())
        }
        Some(Command::RecallContext(args)) => {
            let db = args.db.unwrap_or_else(default_db_path);
            recall::run(&db, args.budget, args.capture, input, out);
            Ok(())
        }
        Some(Command::Recall(args)) => {
            let db = args.db.unwrap_or_else(default_db_path);
            recall::run_search(&db, args.budget, args.k, &args.query.join(" "), args.expand, out);
            Ok(())
        }
        Some(Command::Index(args)) => {
            let db = args.db.unwrap_or_else(default_db_path);
            commands::index(out, &args.path, &db)
        }
        Some(Command::Calibrate(args)) => commands::calibrate(out, args.secs),
        Some(Command::Doctor(args)) => {
            let db = args.db.unwrap_or_else(default_db_path);
            commands::doctor(out, &db)
        }
        Some(Command::Install) => {
            commands::say(out, render_banner(mode))?;
            let exe = std::env::current_exe().map_err(|e| format!("cannot locate this exe: {e}"))?;
            commands::install(out, &exe, &default_db_path())
        }
    }
}

fn env_or_empty(key: &str) -> String {
    std::env::var(key).unwrap_or_default()
}

fn color_forced(value: Option<&std::ffi::OsStr>) -> bool {
    value.is_some_and(|v| !v.is_empty() && v != "0")
}

fn effective_colorterm(colorterm: String, wt_session: bool) -> String {
    if colorterm.trim().is_empty() && wt_session {
        "truecolor".to_string()
    } else {
        colorterm
    }
}

fn is_hook_invocation(args: &[std::ffi::OsString]) -> bool {
    args.get(1)
        .is_some_and(|a| a == "capture" || a == "recall-context")
}

fn parse_cli() -> Cli {
    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    match Cli::try_parse_from(&args) {
        Ok(cli) => cli,
        Err(e) if e.use_stderr() && is_hook_invocation(&args) => {
            eprintln!("uriel: {}", e.to_string().lines().next().unwrap_or("bad arguments"));
            std::process::exit(0)
        }
        Err(e) => e.exit(),
    }
}

fn main() -> ExitCode {
    let cli = parse_cli();
    let stdout = std::io::stdout();
    let tty = stdout.is_terminal();
    let forced = color_forced(std::env::var_os("FORCE_COLOR").as_deref());
    let mut mode = mode_from_env(
        std::env::var_os("NO_COLOR").is_some(),
        &effective_colorterm(env_or_empty("COLORTERM"), std::env::var_os("WT_SESSION").is_some()),
        &env_or_empty("TERM"),
        tty || forced,
    );
    if mode != Mode::Plain && tty && enable_ansi_support::enable_ansi_support().is_err() {
        mode = Mode::Plain;
    }
    let stdin = std::io::stdin();
    let mut out = BufWriter::new(stdout.lock());
    let hook = matches!(cli.command, Some(Command::Capture(_) | Command::RecallContext(_)));
    let input: Box<dyn BufRead + Send> = if hook && stdin.is_terminal() {
        Box::new(std::io::empty())
    } else {
        Box::new(BufReader::new(stdin))
    };
    let result = dispatch(cli, mode, input, &mut out);
    let flushed = out.flush().map_err(|e| e.to_string());
    match result.and(flushed) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("uriel: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    struct Recorder {
        bytes: Vec<u8>,
        writes: usize,
    }

    impl Write for Recorder {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.writes += 1;
            self.bytes.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("uriel").chain(args.iter().copied()))
    }

    fn command(args: &[&str]) -> Option<Command> {
        parse(args).expect("parses").command
    }

    #[test]
    fn no_args_parses_to_no_command() {
        let cli = parse(&[]).expect("parses");
        assert_eq!(cli.command, None);
        assert!(!cli.version);
    }

    #[test]
    fn banner_subcommand_parses() {
        assert_eq!(command(&["banner"]), Some(Command::Banner));
    }

    #[test]
    fn version_flag_parses_short_and_long() {
        assert!(parse(&["--version"]).expect("parses").version);
        assert!(parse(&["-V"]).expect("parses").version);
    }

    #[test]
    fn mcp_parses_every_flag() {
        let cmd = command(&[
            "mcp", "--db", "g.seal", "--root", "proj", "--read-only", "--format", "json",
            "--placement", "planned", "--memory-mib", "64",
        ]);
        assert_eq!(
            cmd,
            Some(Command::Mcp(McpArgs {
                db: Some(PathBuf::from("g.seal")),
                root: Some(PathBuf::from("proj")),
                read_only: true,
                format: Some(FormatArg::Json),
                placement: Some(PlacementArg::Planned),
                mode: None,
                terse: None,
                memory_mib: Some(64),
                token_limit: None,
                idle_release_secs: None,
            }))
        );
    }

    #[test]
    fn mcp_defaults_map_to_the_server_defaults() {
        let Some(Command::Mcp(args)) = command(&["mcp"]) else {
            panic!("expected mcp");
        };
        let cfg = args.to_config_env(None).expect("config");
        assert_eq!(cfg.db, default_db_path());
        assert!(!cfg.read_only && !cfg.in_memory);
        assert_eq!(cfg.format, Format::Compact);
        assert_eq!(cfg.placement, PlacementMode::Local);
        assert_eq!(cfg.token_limit, DEFAULT_TOKEN_LIMIT);
        assert_eq!(cfg.memory, MemoryBudget::mib(50));
    }

    #[test]
    fn mcp_mode_flag_maps_into_the_server_config_and_the_default_is_normal() {
        let cfg_of = |argv: &[&str], env: Option<&str>| {
            let Some(Command::Mcp(args)) = command(argv) else {
                panic!("expected mcp");
            };
            args.to_config_env(env)
        };
        assert_eq!(cfg_of(&["mcp"], None).expect("config").mode, ServerMode::Normal);
        assert_eq!(cfg_of(&["mcp", "--mode", "house"], None).expect("config").mode, ServerMode::House);
        assert_eq!(cfg_of(&["mcp"], Some("house")).expect("config").mode, ServerMode::House);
        assert_eq!(
            cfg_of(&["mcp", "--mode", "normal"], Some("house")).expect("config").mode,
            ServerMode::Normal
        );
        assert!(cfg_of(&["mcp"], Some("warp")).expect_err("bad env").contains(MODE_ENV));
        assert!(parse(&["mcp", "--mode", "warp"]).is_err());
    }

    #[test]
    fn mcp_terse_flag_maps_into_the_server_config_and_the_default_is_off() {
        let cfg_of = |argv: &[&str], env: Option<&str>| {
            let Some(Command::Mcp(args)) = command(argv) else {
                panic!("expected mcp");
            };
            args.to_config_envs(None, env)
        };
        assert_eq!(cfg_of(&["mcp"], None).expect("config").terse, Terse::Off);
        assert_eq!(cfg_of(&["mcp", "--terse", "data"], None).expect("config").terse, Terse::Data);
        assert_eq!(cfg_of(&["mcp", "--terse", "prose"], None).expect("config").terse, Terse::Prose);
        assert_eq!(cfg_of(&["mcp"], Some("prose")).expect("config").terse, Terse::Prose);
        assert_eq!(cfg_of(&["mcp", "--terse", "off"], Some("prose")).expect("config").terse, Terse::Off);
        assert!(cfg_of(&["mcp"], Some("full")).expect_err("bad env").contains(TERSE_ENV));
        assert!(parse(&["mcp", "--terse", "full"]).is_err());
    }

    #[test]
    fn mcp_idle_release_and_token_limit_flags_map_into_the_server_config() {
        let Some(Command::Mcp(args)) = command(&["mcp", "--idle-release-secs", "7", "--token-limit", "50000"]) else {
            panic!("expected mcp");
        };
        let cfg = args.to_config_env(None).expect("config");
        assert_eq!(cfg.idle_release_secs, 7);
        assert_eq!(cfg.token_limit, 50_000);
        let Some(Command::Mcp(args)) = command(&["mcp", "--idle-release-secs", "0"]) else {
            panic!("expected mcp");
        };
        assert_eq!(args.to_config_env(None).expect("config").idle_release_secs, 0);
        let Some(Command::Mcp(args)) = command(&["mcp"]) else {
            panic!("expected mcp");
        };
        let cfg = args.to_config_env(None).expect("config");
        assert_eq!(cfg.idle_release_secs, DEFAULT_IDLE_RELEASE_SECS);
        assert_eq!(cfg.token_limit, DEFAULT_TOKEN_LIMIT);
        assert!(parse(&["mcp", "--token-limit", "0"]).is_err());
        assert!(parse(&["mcp", "--idle-release-secs", "-1"]).is_err());
    }

    #[test]
    fn mcp_memory_flag_maps_into_the_budget_and_rejects_less_than_the_floor() {
        let Some(Command::Mcp(args)) = command(&["mcp", "--memory-mib", "96"]) else {
            panic!("expected mcp");
        };
        assert_eq!(args.to_config_env(None).expect("config").memory, MemoryBudget::mib(96));
        assert!(parse(&["mcp", "--memory-mib", "15"]).is_err());
        assert!(parse(&["mcp", "--memory-mib", "0"]).is_err());
    }

    #[test]
    fn mcp_flags_map_into_the_server_config() {
        let Some(Command::Mcp(args)) =
            command(&["mcp", "--db", "g.seal", "--read-only", "--format", "json", "--placement", "planned"])
        else {
            panic!("expected mcp");
        };
        let cfg = args.to_config_env(None).expect("config");
        assert_eq!(cfg.db, PathBuf::from("g.seal"));
        assert!(cfg.read_only);
        assert_eq!(cfg.format, Format::Json);
        assert_eq!(cfg.placement, PlacementMode::Planned);
    }

    #[test]
    fn index_parses_a_path_and_optional_db() {
        assert_eq!(
            command(&["index", "src", "--db", "g.seal"]),
            Some(Command::Index(IndexArgs {
                path: PathBuf::from("src"),
                db: Some(PathBuf::from("g.seal")),
            }))
        );
        assert!(parse(&["index"]).is_err());
    }

    #[test]
    fn calibrate_parses_with_and_without_secs() {
        assert_eq!(
            command(&["calibrate"]),
            Some(Command::Calibrate(CalibrateArgs { secs: 4.0 }))
        );
        assert_eq!(
            command(&["calibrate", "--secs", "1.5"]),
            Some(Command::Calibrate(CalibrateArgs { secs: 1.5 }))
        );
    }

    #[test]
    fn doctor_and_install_parse() {
        assert_eq!(command(&["doctor"]), Some(Command::Doctor(DoctorArgs { db: None })));
        assert_eq!(command(&["install"]), Some(Command::Install));
    }

    #[test]
    fn capture_parses_its_flags_and_defaults() {
        assert_eq!(
            command(&["capture"]),
            Some(Command::Capture(CaptureArgs { db: None, session: None, transcript: None }))
        );
        assert_eq!(
            command(&["capture", "--db", "g.uriel", "--session", "s", "--transcript", "t.jsonl"]),
            Some(Command::Capture(CaptureArgs {
                db: Some(PathBuf::from("g.uriel")),
                session: Some("s".into()),
                transcript: Some(PathBuf::from("t.jsonl")),
            }))
        );
    }

    #[test]
    fn recall_context_parses_its_flags_and_defaults() {
        assert_eq!(
            command(&["recall-context"]),
            Some(Command::RecallContext(RecallArgs { db: None, budget: 1200, capture: false }))
        );
        assert_eq!(
            command(&["recall-context", "--db", "g", "--budget", "300", "--capture"]),
            Some(Command::RecallContext(RecallArgs {
                db: Some(PathBuf::from("g")),
                budget: 300,
                capture: true,
            }))
        );
    }

    #[test]
    fn recall_parses_its_flags_and_query_words() {
        assert_eq!(
            command(&["recall", "tcp", "drain"]),
            Some(Command::Recall(SearchArgs {
                db: None,
                budget: 1200,
                k: 8,
                expand: None,
                query: vec!["tcp".into(), "drain".into()],
            }))
        );
        assert_eq!(
            command(&["recall", "--db", "g", "--budget", "300", "--k", "5", "release", "v0.2.0"]),
            Some(Command::Recall(SearchArgs {
                db: Some(PathBuf::from("g")),
                budget: 300,
                k: 5,
                expand: None,
                query: vec!["release".into(), "v0.2.0".into()],
            }))
        );
        assert_eq!(
            command(&["recall", "--db", "g", "--expand", "42"]),
            Some(Command::Recall(SearchArgs {
                db: Some(PathBuf::from("g")),
                budget: 1200,
                k: 8,
                expand: Some(42),
                query: Vec::new(),
            }))
        );
        assert!(parse(&["recall", "--expand", "42", "extra", "words"]).is_err());
        assert!(parse(&["recall"]).is_err());
        assert!(parse(&["recall", "--bogus", "x"]).is_err());
        assert!(parse(&["recall", "--k", "many", "x"]).is_err());
    }

    #[test]
    fn hook_invocations_are_recognised_by_their_first_argument() {
        let args = |a: &[&str]| -> Vec<std::ffi::OsString> { a.iter().map(Into::into).collect() };
        assert!(is_hook_invocation(&args(&["uriel", "capture", "--bogus"])));
        assert!(is_hook_invocation(&args(&["uriel", "recall-context"])));
        assert!(!is_hook_invocation(&args(&["uriel", "index"])));
        assert!(!is_hook_invocation(&args(&["uriel", "recall", "x"])));
        assert!(!is_hook_invocation(&args(&["uriel"])));
    }

    #[test]
    fn unknown_flags_and_subcommands_are_rejected() {
        for bad in [
            &["--bogus"][..],
            &["bogus"],
            &["banner", "--bogus"],
            &["mcp", "--bogus"],
            &["mcp", "--format", "xml"],
            &["mcp", "--placement", "warp"],
            &["index", "src", "--bogus"],
            &["calibrate", "--bogus"],
            &["doctor", "--bogus"],
            &["install", "--bogus"],
            &["capture", "--bogus"],
            &["recall-context", "--bogus"],
            &["recall-context", "--budget", "many"],
        ] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn banner_dispatch_writes_the_banner() {
        let mut rec = Recorder { bytes: Vec::new(), writes: 0 };
        let cli = parse(&[]).expect("parses");
        dispatch(cli, Mode::Plain, Cursor::new(Vec::new()), &mut rec).expect("dispatch");
        let text = String::from_utf8(rec.bytes).expect("utf8");
        assert_eq!(text, format!("{}\n", render_banner(Mode::Plain)));
    }

    #[test]
    fn version_dispatch_prints_banner_then_version() {
        let mut rec = Recorder { bytes: Vec::new(), writes: 0 };
        let cli = parse(&["--version"]).expect("parses");
        dispatch(cli, Mode::Plain, Cursor::new(Vec::new()), &mut rec).expect("dispatch");
        let text = String::from_utf8(rec.bytes).expect("utf8");
        let banner = render_banner(Mode::Plain);
        assert!(text.starts_with(&banner));
        assert!(text.trim_end().ends_with(&format!("uriel {}", env!("CARGO_PKG_VERSION"))));
    }

    #[test]
    fn mcp_dispatch_never_writes_the_banner() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("graph.seal");
        for mode in [Mode::TrueColor, Mode::Ansi256, Mode::Plain] {
            let mut rec = Recorder { bytes: Vec::new(), writes: 0 };
            let cli = Cli {
                version: true,
                command: Some(Command::Mcp(McpArgs {
                    db: Some(db.clone()),
                    root: Some(dir.path().to_path_buf()),
                    read_only: false,
                    format: None,
                    placement: None,
                    mode: None,
                    terse: None,
                    memory_mib: None,
                    token_limit: None,
                    idle_release_secs: None,
                })),
            };
            let result = dispatch(cli, mode, Cursor::new(Vec::new()), &mut rec);
            assert!(result.is_ok(), "{result:?}");
            assert_eq!(rec.writes, 0, "{mode:?}");
            assert!(rec.bytes.is_empty(), "{mode:?}");
        }
    }

    #[test]
    fn mcp_dispatch_output_is_only_json_rpc() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cli = Cli {
            version: false,
            command: Some(Command::Mcp(McpArgs {
                db: Some(dir.path().join("graph.seal")),
                root: Some(dir.path().to_path_buf()),
                read_only: true,
                format: None,
                placement: None,
                mode: None,
                terse: None,
                memory_mib: None,
                token_limit: None,
                idle_release_secs: None,
            })),
        };
        let request = "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n";
        let mut rec = Recorder { bytes: Vec::new(), writes: 0 };
        dispatch(cli, Mode::TrueColor, Cursor::new(request.as_bytes().to_vec()), &mut rec)
            .expect("dispatch");
        let text = String::from_utf8(rec.bytes).expect("utf8");
        assert!(!text.contains('\x1b'));
        assert!(!text.contains("URIEL"));
        assert!(text.trim_start().starts_with('{'));
    }

    #[test]
    fn force_color_counts_only_when_set_non_empty_and_not_zero() {
        use std::ffi::OsStr;
        assert!(!color_forced(None));
        assert!(!color_forced(Some(OsStr::new(""))));
        assert!(!color_forced(Some(OsStr::new("0"))));
        assert!(color_forced(Some(OsStr::new("1"))));
        assert!(color_forced(Some(OsStr::new("true"))));
    }

    #[test]
    fn windows_terminal_without_colorterm_gets_truecolor() {
        assert_eq!(effective_colorterm(String::new(), true), "truecolor");
        assert_eq!(effective_colorterm(String::new(), false), "");
        assert_eq!(effective_colorterm("24bit".into(), true), "24bit");
        assert_eq!(
            mode_from_env(false, &effective_colorterm(String::new(), true), "", true),
            Mode::TrueColor
        );
        assert_eq!(mode_from_env(true, &effective_colorterm(String::new(), true), "", true), Mode::Plain);
    }
}
