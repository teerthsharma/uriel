use std::io::{BufReader, BufWriter};
use std::process::ExitCode;

use uriel_mcp::config::{parse_args, Cli, USAGE};

fn main() -> ExitCode {
    let cfg = match parse_args(std::env::args().skip(1)) {
        Ok(Cli::Run(cfg)) => cfg,
        Ok(Cli::Help) => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Ok(Cli::Version) => {
            println!("uriel-mcp {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("uriel-mcp: {e}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    eprintln!(
        "uriel-mcp {} starting (db: {}, read-only: {})",
        env!("CARGO_PKG_VERSION"),
        if cfg.in_memory {
            ":memory:".to_string()
        } else {
            cfg.db.display().to_string()
        },
        cfg.read_only
    );
    let stdin = std::io::stdin();
    let stdout = BufWriter::new(std::io::stdout().lock());
    match uriel_mcp::run(&cfg, BufReader::new(stdin.lock()), stdout) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("uriel-mcp: {e}");
            ExitCode::FAILURE
        }
    }
}
