#![deny(unsafe_code)]

pub mod compact;
pub mod config;
pub mod db;
pub mod house_tools;
pub mod memory;
pub mod place_tools;
pub mod placement;
pub mod prefix;
pub mod prefix_tree;
pub mod rag_tools;
pub mod research_tools;
mod render;
pub mod server;
pub mod services;
pub mod session;
pub mod soul;
pub mod terse;
pub mod tools;
pub mod view;

use std::io::{BufRead, Write};
use std::time::Duration;

use config::Config;
use db::Db;
use server::Server;
use services::Services;
use tools::Context;

fn persist_dir(cfg: &Config) -> Option<std::path::PathBuf> {
    if cfg.in_memory || cfg.read_only {
        return None;
    }
    Some(match cfg.db.parent().filter(|d| !d.as_os_str().is_empty()) {
        Some(dir) => dir.to_path_buf(),
        None => std::path::PathBuf::from("."),
    })
}

pub fn build_server(cfg: &Config) -> Result<Server, String> {
    let ctx = Context {
        db: Db::new(cfg)?,
        read_only: cfg.read_only,
        mode: cfg.mode,
        terse: cfg.terse,
        meter: terse::Meter::default(),
        services: Services::from_config(cfg)?,
        format: cfg.format,
        session: session::Session::for_budget(cfg.memory),
        placement: placement::PlaceState::new(cfg.placement, persist_dir(cfg)),
    };
    let idle = (cfg.idle_release_secs > 0).then(|| Duration::from_secs(cfg.idle_release_secs));
    Ok(Server::new(ctx, cfg.token_limit, idle))
}

pub fn run(cfg: &Config, input: impl BufRead + Send + 'static, output: impl Write) -> Result<(), String> {
    let server = build_server(cfg)?;
    server::serve(&server, input, output).map_err(|e| format!("io error: {e}"))
}
