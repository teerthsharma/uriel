#![deny(unsafe_code)]

pub mod compact;
pub mod config;
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

use config::Config;
use uriel_query::Engine;
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
    let engine = if cfg.in_memory {
        Engine::open_in_memory(cfg.memory)
    } else if cfg.read_only {
        if cfg.db.is_file() {
            Engine::open_read_only(&cfg.db, cfg.memory)
        } else {
            eprintln!(
                "uriel-mcp: {} does not exist; serving an empty read-only graph",
                cfg.db.display()
            );
            Engine::open_in_memory(cfg.memory)
        }
    } else {
        if let Some(dir) = cfg.db.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        }
        Engine::open(&cfg.db, cfg.memory)
    }
    .map_err(|e| format!("cannot open database: {e}"))?;
    let ctx = Context {
        engine,
        read_only: cfg.read_only,
        mode: cfg.mode,
        terse: cfg.terse,
        meter: terse::Meter::default(),
        services: Services::from_config(cfg)?,
        format: cfg.format,
        session: session::Session::for_budget(cfg.memory),
        placement: placement::PlaceState::new(cfg.placement, persist_dir(cfg)),
    };
    if !cfg.read_only {
        if let Err(e) = tools::write_query(&ctx, memory::CREATE_CONSTRAINT, &Default::default()) {
            eprintln!("uriel-mcp: could not create memory constraint: {e}");
        }
    }
    Ok(Server::new(ctx, cfg.token_limit))
}

pub fn run(cfg: &Config, input: impl BufRead, output: impl Write) -> Result<(), String> {
    let server = build_server(cfg)?;
    server::serve(&server, input, output).map_err(|e| format!("io error: {e}"))
}
