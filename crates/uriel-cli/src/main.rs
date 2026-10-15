#![forbid(unsafe_code)]

mod banner;
mod commands;

use std::io::{BufRead, BufWriter, IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand, ValueEnum};
use uriel_mcp::config::{default_db_path, Config, Format, PlacementMode, DEFAULT_TOKEN_LIMIT};

use banner::{mode_from_env, render_banner, Mode};

#[derive(Debug, Parser, PartialEq)]
#[command(name = "uriel", disable_version_flag = true, about = "uriel: fewest tokens in, same information out")]
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum FormatArg {
    Compact,
    Json,
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

impl McpArgs {
    fn to_config(&self) -> Config {
        Config {
            db: self.db.clone().unwrap_or_else(default_db_path),
            read_only: self.read_only,
            in_memory: false,
            token_limit: DEFAULT_TOKEN_LIMIT,
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
        }
    }
}

fn dispatch<R: BufRead, W: Write>(
    cli: Cli,
    mode: Mode,
    input: R,
    out: &mut W,
) -> Result<(), String> {
    if let Some(Command::Mcp(args)) = &cli.command {
        return uriel_mcp::run(&args.to_config(), input, &mut *out);
    }
    if cli.version {
        commands::say(out, render_banner(mode))?;
        return commands::say(out, format!("uriel {}", env!("CARGO_PKG_VERSION")));
    }
    match cli.command {
        None | Some(Command::Banner) | Some(Command::Mcp(_)) => {
            commands::say(out, render_banner(mode))
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

fn main() -> ExitCode {
    let cli = Cli::parse();
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
    let result = dispatch(cli, mode, stdin.lock(), &mut out);
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
            "--placement", "planned",
        ]);
        assert_eq!(
            cmd,
            Some(Command::Mcp(McpArgs {
                db: Some(PathBuf::from("g.seal")),
                root: Some(PathBuf::from("proj")),
                read_only: true,
                format: Some(FormatArg::Json),
                placement: Some(PlacementArg::Planned),
            }))
        );
    }

    #[test]
    fn mcp_defaults_map_to_the_server_defaults() {
        let Some(Command::Mcp(args)) = command(&["mcp"]) else {
            panic!("expected mcp");
        };
        let cfg = args.to_config();
        assert_eq!(cfg.db, default_db_path());
        assert!(!cfg.read_only && !cfg.in_memory);
        assert_eq!(cfg.format, Format::Compact);
        assert_eq!(cfg.placement, PlacementMode::Local);
        assert_eq!(cfg.token_limit, DEFAULT_TOKEN_LIMIT);
    }

    #[test]
    fn mcp_flags_map_into_the_server_config() {
        let Some(Command::Mcp(args)) =
            command(&["mcp", "--db", "g.seal", "--read-only", "--format", "json", "--placement", "planned"])
        else {
            panic!("expected mcp");
        };
        let cfg = args.to_config();
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
