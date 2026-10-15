//! Event sinks. Events are the JSON shapes read by the live board (`house.py`, `gates.py`).

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use serde_json::Value;

/// Receives every board event.
pub trait EventSink: Send + Sync {
    /// Handles one event. Must not panic; failures are the sink's to swallow or log.
    fn emit(&self, event: Value);
}

/// Appends one JSON object per line to a file (`house-events.jsonl`).
#[derive(Debug)]
pub struct JsonlSink {
    path: PathBuf,
    lock: Mutex<()>,
}

impl JsonlSink {
    /// Creates a sink appending to `path` (the file is created on first event).
    pub fn new(path: impl AsRef<Path>) -> JsonlSink {
        JsonlSink {
            path: path.as_ref().to_path_buf(),
            lock: Mutex::new(()),
        }
    }

    /// The file this sink writes.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl EventSink for JsonlSink {
    fn emit(&self, event: Value) {
        let _guard = self.lock.lock().unwrap_or_else(PoisonError::into_inner);
        if let Ok(mut f) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            // a failed board write must never fail the harness
            let _ = writeln!(f, "{event}");
        }
    }
}

/// Collects events in memory (tests).
#[derive(Debug, Default, Clone)]
pub struct VecSink {
    events: Arc<Mutex<Vec<Value>>>,
}

impl VecSink {
    /// An empty sink.
    pub fn new() -> VecSink {
        VecSink::default()
    }

    /// Everything emitted so far.
    pub fn events(&self) -> Vec<Value> {
        self.events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Events whose `t` equals `kind`.
    pub fn of_kind(&self, kind: &str) -> Vec<Value> {
        self.events()
            .into_iter()
            .filter(|e| e.get("t").and_then(Value::as_str) == Some(kind))
            .collect()
    }
}

impl EventSink for VecSink {
    fn emit(&self, event: Value) {
        self.events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(event);
    }
}

impl EventSink for Arc<VecSink> {
    fn emit(&self, event: Value) {
        (**self).emit(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn jsonl_sink_appends_lines() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("house-events.jsonl");
        let sink = JsonlSink::new(&path);
        sink.emit(json!({"t":"done","agent":"House"}));
        sink.emit(json!({"t":"test","agent":"Chase","status":"red","name":"x"}));
        let text = std::fs::read_to_string(&path).expect("read");
        let lines: Vec<_> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        let second: Value = serde_json::from_str(lines[1]).expect("json");
        assert_eq!(second["status"], "red");
    }

    #[test]
    fn vec_sink_filters_by_kind() {
        let s = VecSink::new();
        s.emit(json!({"t":"goal","id":"G1"}));
        s.emit(json!({"t":"test","name":"a"}));
        assert_eq!(s.of_kind("goal").len(), 1);
        assert_eq!(s.events().len(), 2);
    }
}
