use std::path::PathBuf;
use std::sync::OnceLock;

use uriel_core::Graph;
use uriel_house::{FakeProbe, House, HouseConfig, JsonlSink, RealProbe, SystemProbe};
use uriel_rag::{Rag, RagConfig};

use crate::config::{default_config_path, Config};

pub const FAKE_PROBE_ENV: &str = "SEAL_MCP_FAKE_PROBE";

pub struct Services {
    pub root: PathBuf,
    pub events: PathBuf,
    pub config_source: Option<PathBuf>,
    pub house_cfg: HouseConfig,
    rag: OnceLock<Rag>,
    house: OnceLock<House>,
}

fn probe_from_env() -> Box<dyn SystemProbe> {
    let fake = std::env::var(FAKE_PROBE_ENV).ok().and_then(|v| {
        let (free, total) = v.split_once(',')?;
        Some(FakeProbe::new(
            free.trim().parse().ok()?,
            total.trim().parse().ok()?,
        ))
    });
    match fake {
        Some(p) => Box::new(p),
        None => Box::new(RealProbe::new()),
    }
}

impl Services {
    pub fn from_config(cfg: &Config) -> Result<Services, String> {
        if !cfg.root.is_dir() {
            return Err(format!("--root {} is not a directory", cfg.root.display()));
        }
        let (mut house_cfg, config_source) = match &cfg.config {
            Some(path) => (
                HouseConfig::from_file(path)
                    .map_err(|e| format!("cannot load config {}: {e}", path.display()))?,
                Some(path.clone()),
            ),
            None => {
                let path = default_config_path();
                if path.is_file() {
                    (
                        HouseConfig::from_file(&path)
                            .map_err(|e| format!("cannot load config {}: {e}", path.display()))?,
                        Some(path),
                    )
                } else {
                    (HouseConfig::default(), None)
                }
            }
        };
        house_cfg.project_root = cfg.root.clone();
        house_cfg.apply_env();
        Ok(Services {
            root: cfg.root.clone(),
            events: cfg.events_path(),
            config_source,
            house_cfg,
            rag: OnceLock::new(),
            house: OnceLock::new(),
        })
    }

    pub fn rag(&self, graph: &Graph) -> Result<&Rag, String> {
        if let Some(r) = self.rag.get() {
            return Ok(r);
        }
        let rag = Rag::open(graph.clone(), RagConfig::default())
            .map_err(|e| format!("cannot open the RAG index: {e}"))?;
        Ok(self.rag.get_or_init(|| rag))
    }

    pub fn house(&self, graph: &Graph) -> Result<&House, String> {
        if let Some(h) = self.house.get() {
            return Ok(h);
        }
        if let Some(dir) = self.events.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        }
        let house = House::open(
            graph.clone(),
            self.house_cfg.clone(),
            probe_from_env(),
            Box::new(JsonlSink::new(&self.events)),
        )
        .map_err(|e| format!("cannot open the House: {e}"))?;
        Ok(self.house.get_or_init(|| house))
    }
}
