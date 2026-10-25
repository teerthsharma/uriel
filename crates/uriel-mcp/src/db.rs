use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use uriel_core::MemoryBudget;
use uriel_house::House;
use uriel_query::{AccessMode, Engine, QueryError};
use uriel_rag::Rag;

use crate::config::Config;
use crate::memory;

pub const LOCK_WAIT: Duration = Duration::from_secs(3);

pub struct Open {
    pub engine: Engine,
    pub(crate) rag: OnceLock<Arc<Rag>>,
    pub(crate) house: OnceLock<Arc<House>>,
}

impl Open {
    fn new(engine: Engine) -> Open {
        Open {
            engine,
            rag: OnceLock::new(),
            house: OnceLock::new(),
        }
    }
}

enum Source {
    InMemory,
    ReadOnly(PathBuf),
    ReadWrite(PathBuf),
}

pub struct Db {
    source: Source,
    memory: MemoryBudget,
    slot: Mutex<Option<Arc<Open>>>,
}

fn locked(e: &QueryError) -> bool {
    matches!(e, QueryError::Core(c) if c.is_already_open())
}

impl Db {
    pub fn new(cfg: &Config) -> Result<Db, String> {
        let source = if cfg.in_memory {
            Source::InMemory
        } else if cfg.read_only {
            Source::ReadOnly(cfg.db.clone())
        } else {
            Source::ReadWrite(cfg.db.clone())
        };
        let db = Db {
            source,
            memory: cfg.memory,
            slot: Mutex::new(None),
        };
        if matches!(db.source, Source::InMemory) {
            db.get()?;
        }
        Ok(db)
    }

    fn slot(&self) -> MutexGuard<'_, Option<Arc<Open>>> {
        self.slot.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn get(&self) -> Result<Arc<Open>, String> {
        let mut slot = self.slot();
        if let Some(open) = slot.as_ref() {
            return Ok(Arc::clone(open));
        }
        let open = Arc::new(Open::new(self.open_waiting()?));
        *slot = Some(Arc::clone(&open));
        Ok(open)
    }

    pub fn is_open(&self) -> bool {
        self.slot().is_some()
    }

    pub fn releasable(&self) -> bool {
        !matches!(self.source, Source::InMemory)
    }

    pub fn release(&self) -> bool {
        self.releasable() && self.slot().take().is_some()
    }

    fn open_waiting(&self) -> Result<Engine, String> {
        if let Source::ReadWrite(path) = &self.source {
            if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
                std::fs::create_dir_all(dir)
                    .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
            }
        }
        let start = Instant::now();
        let mut pause = Duration::from_millis(25);
        loop {
            match self.open_once() {
                Ok(engine) => return Ok(engine),
                Err(e) if locked(&e) => {
                    let waited = start.elapsed();
                    if waited >= LOCK_WAIT {
                        return Err(format!(
                            "the graph {} is locked by another uriel process (waited {:.1} s); retry the call in a few seconds",
                            self.path_name(),
                            waited.as_secs_f64()
                        ));
                    }
                    std::thread::sleep(pause.min(LOCK_WAIT - waited));
                    pause = (pause * 2).min(Duration::from_millis(400));
                }
                Err(e) => return Err(format!("cannot open database: {e}")),
            }
        }
    }

    fn path_name(&self) -> String {
        match &self.source {
            Source::InMemory => ":memory:".into(),
            Source::ReadOnly(p) | Source::ReadWrite(p) => p.display().to_string(),
        }
    }

    fn open_once(&self) -> Result<Engine, QueryError> {
        match &self.source {
            Source::InMemory => Engine::open_in_memory(self.memory),
            Source::ReadOnly(path) if path.is_file() => Engine::open_read_only(path, self.memory),
            Source::ReadOnly(path) => {
                eprintln!(
                    "uriel-mcp: {} does not exist; serving an empty read-only graph",
                    path.display()
                );
                Engine::open_in_memory(self.memory)
            }
            Source::ReadWrite(path) => {
                let engine = Engine::open(path, self.memory)?;
                if let Err(e) =
                    engine.execute(memory::CREATE_CONSTRAINT, &Default::default(), AccessMode::Write)
                {
                    eprintln!("uriel-mcp: could not create memory constraint: {e}");
                }
                Ok(engine)
            }
        }
    }
}
