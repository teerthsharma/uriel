//! System probes: how much RAM and CPU the machine has right now.

use std::sync::{Mutex, PoisonError};

/// A point-in-time reading of the machine.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SystemSnapshot {
    /// Available RAM in MB.
    pub free_ram_mb: u64,
    /// Installed RAM in MB.
    pub total_ram_mb: u64,
    /// Logical cores.
    pub logical_cores: u32,
    /// Global CPU load, 0 to 100.
    pub load_pct: f32,
}

/// Source of [`SystemSnapshot`]s. Read on every `next()`, never cached.
pub trait SystemProbe: Send + Sync {
    /// Takes a reading.
    fn snapshot(&self) -> SystemSnapshot;
}

/// The real probe, backed by `sysinfo`.
pub struct RealProbe {
    sys: Mutex<sysinfo::System>,
}

impl Default for RealProbe {
    fn default() -> Self {
        RealProbe::new()
    }
}

impl RealProbe {
    /// Creates a probe.
    pub fn new() -> RealProbe {
        RealProbe {
            sys: Mutex::new(sysinfo::System::new()),
        }
    }
}

impl SystemProbe for RealProbe {
    fn snapshot(&self) -> SystemSnapshot {
        let mut sys = self.sys.lock().unwrap_or_else(PoisonError::into_inner);
        sys.refresh_memory();
        sys.refresh_cpu_usage();
        let mb = |bytes: u64| bytes / (1024 * 1024);
        SystemSnapshot {
            free_ram_mb: mb(sys.available_memory()),
            total_ram_mb: mb(sys.total_memory()),
            logical_cores: u32::try_from(sys.cpus().len()).unwrap_or(u32::MAX),
            load_pct: sys.global_cpu_usage(),
        }
    }
}

/// A scriptable probe for tests: `set` changes what the next `snapshot` returns.
#[derive(Debug)]
pub struct FakeProbe {
    inner: Mutex<SystemSnapshot>,
}

impl FakeProbe {
    /// A probe with `free_mb` of `total_mb` free, 8 cores, 0% load.
    pub fn new(free_mb: u64, total_mb: u64) -> FakeProbe {
        FakeProbe {
            inner: Mutex::new(SystemSnapshot {
                free_ram_mb: free_mb,
                total_ram_mb: total_mb,
                logical_cores: 8,
                load_pct: 0.0,
            }),
        }
    }

    /// Changes the free RAM that later snapshots report.
    pub fn set_free(&self, free_mb: u64) {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .free_ram_mb = free_mb;
    }
}

impl SystemProbe for FakeProbe {
    fn snapshot(&self) -> SystemSnapshot {
        *self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl<T: SystemProbe + ?Sized> SystemProbe for std::sync::Arc<T> {
    fn snapshot(&self) -> SystemSnapshot {
        (**self).snapshot()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_probe_changes_over_time() {
        let p = FakeProbe::new(6000, 16000);
        assert_eq!(p.snapshot().free_ram_mb, 6000);
        p.set_free(2000);
        assert_eq!(p.snapshot().free_ram_mb, 2000);
        assert_eq!(p.snapshot().total_ram_mb, 16000);
    }

    #[test]
    fn real_probe_reports_a_machine() {
        let s = RealProbe::new().snapshot();
        assert!(s.total_ram_mb > 0);
        assert!(s.logical_cores > 0);
        assert!(s.free_ram_mb <= s.total_ram_mb);
    }
}
