use crate::error::SysError;
use crate::raw;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessMemory {
    pub working_set: u64,
    pub private_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryStatus {
    pub total: u64,
    pub available: u64,
    pub source: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LargePageStatus {
    Ready { minimum: usize },
    PrivilegeMissing,
    Unsupported,
}

pub fn process_memory() -> Result<ProcessMemory, SysError> {
    raw::process_memory()
}

pub fn memory_status() -> Result<MemoryStatus, SysError> {
    raw::memory_status()
}

pub fn trim_heap() -> bool {
    raw::trim_heap()
}

pub fn rss_source() -> &'static str {
    raw::RSS_SOURCE
}

pub fn large_page_status() -> LargePageStatus {
    raw::large_page_status()
}

pub fn large_page_privilege_held() -> bool {
    matches!(large_page_status(), LargePageStatus::Ready { .. })
}

#[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
pub(crate) fn parse_statm(text: &str, page: u64) -> Option<ProcessMemory> {
    let mut fields = text.split_whitespace().map(|f| f.parse::<u64>().ok());
    let virtual_pages = fields.next().flatten()?;
    let resident_pages = fields.next().flatten()?;
    Some(ProcessMemory {
        working_set: resident_pages.checked_mul(page)?,
        private_bytes: virtual_pages.checked_mul(page)?,
    })
}

#[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
pub(crate) fn parse_meminfo(text: &str) -> Option<(u64, u64)> {
    let kib = |key: &str| -> Option<u64> {
        let line = text.lines().find(|l| l.starts_with(key))?;
        let n = line[key.len()..].split_whitespace().next()?.parse::<u64>().ok()?;
        n.checked_mul(1024)
    };
    let total = kib("MemTotal:")?;
    let available = kib("MemAvailable:").or_else(|| {
        let free = kib("MemFree:")?;
        let cached = kib("Cached:").unwrap_or(0);
        free.checked_add(cached)
    })?;
    Some((total, available.min(total)))
}

#[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
pub(crate) fn apply_cgroup_limit(
    total: u64,
    available: u64,
    max: &str,
    current: &str,
) -> (u64, u64, bool) {
    let Ok(limit) = max.trim().parse::<u64>() else {
        return (total, available, false);
    };
    let used = current.trim().parse::<u64>().unwrap_or(0);
    if limit >= total {
        return (total, available, false);
    }
    (limit, available.min(limit.saturating_sub(used)), true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statm_scales_pages_by_the_page_size() {
        let m = parse_statm("1000 250 10 1 0 5 0", 16384).unwrap();
        assert_eq!(m.working_set, 250 * 16384);
        assert_eq!(m.private_bytes, 1000 * 16384);
    }

    #[test]
    fn statm_rejects_garbage_without_panicking() {
        for bad in ["", "1", "a b", "18446744073709551615 18446744073709551615"] {
            assert!(parse_statm(bad, 4096).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn meminfo_prefers_mem_available() {
        let t = "MemTotal:       16384 kB\nMemFree:  1024 kB\nMemAvailable:    8192 kB\n";
        assert_eq!(parse_meminfo(t), Some((16384 * 1024, 8192 * 1024)));
    }

    #[test]
    fn meminfo_falls_back_to_free_plus_cached() {
        let t = "MemTotal: 4096 kB\nMemFree: 1000 kB\nCached: 500 kB\n";
        assert_eq!(parse_meminfo(t), Some((4096 * 1024, 1500 * 1024)));
        assert_eq!(parse_meminfo("MemFree: 1 kB"), None);
    }

    #[test]
    fn cgroup_limit_shrinks_total_and_available() {
        let (t, a, hit) = apply_cgroup_limit(8 << 30, 6 << 30, "1073741824\n", "900000000\n");
        assert!(hit);
        assert_eq!(t, 1 << 30);
        assert_eq!(a, (1u64 << 30) - 900_000_000);
    }

    #[test]
    fn cgroup_unlimited_or_larger_than_ram_changes_nothing() {
        assert_eq!(apply_cgroup_limit(8 << 30, 6 << 30, "max\n", "5"), (8 << 30, 6 << 30, false));
        assert_eq!(apply_cgroup_limit(1 << 30, 1 << 29, "9999999999999", "5"), (1 << 30, 1 << 29, false));
    }

    #[test]
    fn this_process_reports_a_resident_set_on_every_os() {
        let m = process_memory().unwrap();
        assert!(m.working_set > 0, "{m:?}");
        assert!(!rss_source().is_empty());
    }

    #[test]
    fn this_machine_reports_free_ram_on_every_os() {
        let s = memory_status().unwrap();
        assert!(s.total > 0 && s.available > 0 && s.available <= s.total, "{s:?}");
    }

    #[test]
    fn trimming_the_heap_never_fails_and_keeps_the_process_alive() {
        let ballast = vec![7u8; 8 << 20];
        drop(ballast);
        let _ = trim_heap();
        assert!(process_memory().unwrap().working_set > 0);
    }
}
