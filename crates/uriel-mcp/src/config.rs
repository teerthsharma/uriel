use std::path::PathBuf;

pub const DEFAULT_TOKEN_LIMIT: usize = 20_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Format {
    #[default]
    Compact,
    Json,
}

impl Format {
    pub fn parse(s: &str) -> Option<Format> {
        match s {
            "compact" => Some(Format::Compact),
            "json" => Some(Format::Json),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlacementMode {
    #[default]
    Local,
    Planned,
}

impl PlacementMode {
    pub fn parse(s: &str) -> Option<PlacementMode> {
        match s {
            "local" => Some(PlacementMode::Local),
            "planned" => Some(PlacementMode::Planned),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            PlacementMode::Local => "local",
            PlacementMode::Planned => "planned",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub db: PathBuf,
    pub read_only: bool,
    pub in_memory: bool,
    pub token_limit: usize,
    pub root: PathBuf,
    pub events: Option<PathBuf>,
    pub config: Option<PathBuf>,
    pub format: Format,
    pub placement: PlacementMode,
}

impl Config {
    pub fn events_path(&self) -> PathBuf {
        self.events
            .clone()
            .unwrap_or_else(|| self.root.join("house-events.jsonl"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cli {
    Run(Config),
    Help,
    Version,
}

pub const USAGE: &str = "\
uriel-mcp: stdio MCP server for the seal graph database

USAGE:
    uriel-mcp [OPTIONS]

OPTIONS:
    --db <PATH>          Database file (default: ~/.seal/graph.seal)
    --read-only          Hide and deny all write tools
    --in-memory          Use a throwaway in-memory database
    --token-limit <N>    Truncate tool output to N characters (default: 20000)
    --root <PATH>        Project root for the rag_* and house_* tools (default: current directory)
    --events <PATH>      House event log (default: <root>/house-events.jsonl)
    --config <PATH>      JSON config with model slots (default: ~/.seal/config.json if present)
    --format <F>         Result format of rag_search, seal_recall and read_neo4j_cypher:
                         compact (default; fewer tokens) or json (exact v1 output)
    --placement <M>      local (default) or planned: planned keeps a placement model fed by the
                         nodes the tools read, which why_hot and pin explain and steer
    -h, --help           Print this help
    -V, --version        Print version
";

pub fn default_db_path() -> PathBuf {
    let home = std::env::var_os("USERPROFILE")
        .filter(|v| !v.is_empty())
        .or_else(|| std::env::var_os("HOME").filter(|v| !v.is_empty()))
        .map_or_else(|| PathBuf::from("."), PathBuf::from);
    home.join(".seal").join("graph.seal")
}

pub fn default_config_path() -> PathBuf {
    default_db_path().with_file_name("config.json")
}

pub fn parse_args<I, S>(args: I) -> Result<Cli, String>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut cfg = Config {
        db: default_db_path(),
        read_only: false,
        in_memory: false,
        token_limit: DEFAULT_TOKEN_LIMIT,
        root: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        events: None,
        config: None,
        format: Format::Compact,
        placement: PlacementMode::Local,
    };
    let mut it = args.into_iter().map(Into::into);
    while let Some(arg) = it.next() {
        let (flag, inline) = match arg.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f.to_string(), Some(v.to_string())),
            _ => (arg.clone(), None),
        };
        match flag.as_str() {
            "-h" | "--help" => return Ok(Cli::Help),
            "-V" | "--version" => return Ok(Cli::Version),
            "--read-only" => no_value(&flag, &inline, || cfg.read_only = true)?,
            "--in-memory" => no_value(&flag, &inline, || cfg.in_memory = true)?,
            "--db" => {
                let v = value(&flag, inline, &mut it)?;
                if v.is_empty() {
                    return Err("--db requires a non-empty path".into());
                }
                cfg.db = PathBuf::from(v);
            }
            "--token-limit" => {
                let v = value(&flag, inline, &mut it)?;
                cfg.token_limit = v.parse::<usize>().ok().filter(|n| *n > 0).ok_or_else(|| {
                    format!("--token-limit expects a positive integer, got '{v}'")
                })?;
            }
            "--root" => cfg.root = path_value(&flag, inline, &mut it)?,
            "--events" => cfg.events = Some(path_value(&flag, inline, &mut it)?),
            "--config" => cfg.config = Some(path_value(&flag, inline, &mut it)?),
            "--format" => {
                let v = value(&flag, inline, &mut it)?;
                cfg.format = Format::parse(&v)
                    .ok_or_else(|| format!("--format expects compact or json, got '{v}'"))?;
            }
            "--placement" => {
                let v = value(&flag, inline, &mut it)?;
                cfg.placement = PlacementMode::parse(&v)
                    .ok_or_else(|| format!("--placement expects local or planned, got '{v}'"))?;
            }
            other => return Err(format!("unknown argument '{other}'")),
        }
    }
    Ok(Cli::Run(cfg))
}

fn no_value(flag: &str, inline: &Option<String>, set: impl FnOnce()) -> Result<(), String> {
    if inline.is_some() {
        return Err(format!("{flag} does not take a value"));
    }
    set();
    Ok(())
}

fn path_value(
    flag: &str,
    inline: Option<String>,
    it: &mut impl Iterator<Item = String>,
) -> Result<PathBuf, String> {
    let v = value(flag, inline, it)?;
    if v.is_empty() {
        return Err(format!("{flag} requires a non-empty path"));
    }
    Ok(PathBuf::from(v))
}

fn value(
    flag: &str,
    inline: Option<String>,
    it: &mut impl Iterator<Item = String>,
) -> Result<String, String> {
    inline
        .or_else(|| it.next())
        .ok_or_else(|| format!("{flag} requires a value"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(args: &[&str]) -> Config {
        match parse_args(args.iter().copied()).unwrap() {
            Cli::Run(c) => c,
            other => panic!("expected Run, got {other:?}"),
        }
    }

    #[test]
    fn defaults() {
        let c = run(&[]);
        assert!(!c.read_only && !c.in_memory);
        assert_eq!(c.token_limit, DEFAULT_TOKEN_LIMIT);
        assert!(c.db.ends_with(".seal/graph.seal") || c.db.ends_with(".seal\\graph.seal"));
    }

    #[test]
    fn flags_and_values() {
        let c = run(&[
            "--db",
            "x.seal",
            "--read-only",
            "--in-memory",
            "--token-limit=50",
        ]);
        assert_eq!(c.db, PathBuf::from("x.seal"));
        assert!(c.read_only && c.in_memory);
        assert_eq!(c.token_limit, 50);
        assert_eq!(run(&["--db=y.seal"]).db, PathBuf::from("y.seal"));
    }

    #[test]
    fn root_events_and_config_flags() {
        let c = run(&["--root", "proj", "--events=ev.jsonl", "--config", "c.json"]);
        assert_eq!(c.root, PathBuf::from("proj"));
        assert_eq!(c.events_path(), PathBuf::from("ev.jsonl"));
        assert_eq!(c.config, Some(PathBuf::from("c.json")));
    }

    #[test]
    fn events_default_to_the_root() {
        let c = run(&["--root", "proj"]);
        assert_eq!(
            c.events_path(),
            PathBuf::from("proj").join("house-events.jsonl")
        );
        assert_eq!(c.config, None);
    }

    #[test]
    fn rejects_bad_input() {
        for bad in [
            &["--db"][..],
            &["--token-limit", "0"],
            &["--token-limit", "abc"],
            &["--bogus"],
            &["--read-only=1"],
            &["--root"],
            &["--events", ""],
            &["--format", "xml"],
            &["--format"],
        ] {
            assert!(parse_args(bad.iter().copied()).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn format_flag() {
        assert_eq!(run(&[]).format, Format::Compact);
        assert_eq!(run(&["--format", "json"]).format, Format::Json);
        assert_eq!(run(&["--format=compact"]).format, Format::Compact);
    }

    #[test]
    fn placement_flag() {
        assert_eq!(run(&[]).placement, PlacementMode::Local);
        assert_eq!(run(&["--placement", "planned"]).placement, PlacementMode::Planned);
        assert_eq!(run(&["--placement=local"]).placement, PlacementMode::Local);
        for bad in [&["--placement", "warp"][..], &["--placement"], &["--placement="]] {
            assert!(parse_args(bad.iter().copied()).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn placement_mode_names_round_trip() {
        for m in [PlacementMode::Local, PlacementMode::Planned] {
            assert_eq!(PlacementMode::parse(m.name()), Some(m));
        }
    }

    #[test]
    fn help_and_version() {
        assert_eq!(parse_args(["--help"]).unwrap(), Cli::Help);
        assert_eq!(parse_args(["-V"]).unwrap(), Cli::Version);
    }
}
