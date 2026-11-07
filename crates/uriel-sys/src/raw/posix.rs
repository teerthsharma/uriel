use std::mem::MaybeUninit;

use crate::error::SysError;
use crate::rss::{MemoryStatus, ProcessMemory};

#[cfg(target_os = "linux")]
pub(crate) const RSS_SOURCE: &str = "linux:/proc/self/statm, fallback getrusage(ru_maxrss, peak)";
#[cfg(target_os = "macos")]
pub(crate) const RSS_SOURCE: &str = "macos:task_info(MACH_TASK_BASIC_INFO), fallback getrusage(ru_maxrss, peak)";
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) const RSS_SOURCE: &str = "unix:getrusage(ru_maxrss, peak)";

fn os_error(call: &'static str) -> SysError {
    let code = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
    SysError::Os { call, code: code as u32 }
}

fn page_size() -> u64 {
    let n = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if n > 0 {
        n as u64
    } else {
        4096
    }
}

fn peak_rss_from_getrusage() -> Result<ProcessMemory, SysError> {
    let mut usage = MaybeUninit::<libc::rusage>::zeroed();
    let rc = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    if rc != 0 {
        return Err(os_error("getrusage"));
    }
    let maxrss = unsafe { usage.assume_init() }.ru_maxrss.max(0) as u64;
    let bytes = if cfg!(target_os = "macos") { maxrss } else { maxrss.saturating_mul(1024) };
    Ok(ProcessMemory { working_set: bytes, private_bytes: bytes })
}

#[cfg(target_os = "linux")]
pub(crate) fn process_memory() -> Result<ProcessMemory, SysError> {
    let from_statm = std::fs::read_to_string("/proc/self/statm")
        .ok()
        .and_then(|text| crate::rss::parse_statm(&text, page_size()));
    match from_statm {
        Some(m) => Ok(m),
        None => peak_rss_from_getrusage(),
    }
}

#[cfg(target_os = "macos")]
#[allow(deprecated)]
pub(crate) fn process_memory() -> Result<ProcessMemory, SysError> {
    let mut info = unsafe { std::mem::zeroed::<libc::mach_task_basic_info>() };
    let mut count = libc::MACH_TASK_BASIC_INFO_COUNT;
    let kr = unsafe {
        libc::task_info(
            libc::mach_task_self(),
            libc::MACH_TASK_BASIC_INFO,
            (&mut info as *mut libc::mach_task_basic_info).cast(),
            &mut count,
        )
    };
    if kr != libc::KERN_SUCCESS {
        return peak_rss_from_getrusage();
    }
    let resident = info.resident_size;
    let virtual_size = info.virtual_size;
    Ok(ProcessMemory { working_set: resident, private_bytes: virtual_size })
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) fn process_memory() -> Result<ProcessMemory, SysError> {
    peak_rss_from_getrusage()
}

#[cfg(target_os = "linux")]
pub(crate) fn memory_status() -> Result<MemoryStatus, SysError> {
    let text = std::fs::read_to_string("/proc/meminfo").map_err(|e| SysError::Io(e.kind()))?;
    let (total, available) = crate::rss::parse_meminfo(&text).ok_or(SysError::Unsupported)?;
    let read = |p: &str| std::fs::read_to_string(p).ok();
    let limited = read("/sys/fs/cgroup/memory.max").map(|max| {
        let current = read("/sys/fs/cgroup/memory.current").unwrap_or_default();
        crate::rss::apply_cgroup_limit(total, available, &max, &current)
    });
    Ok(match limited {
        Some((t, a, true)) => MemoryStatus {
            total: t,
            available: a,
            source: "linux:/proc/meminfo MemAvailable, capped by cgroup v2 memory.max",
        },
        _ => MemoryStatus { total, available, source: "linux:/proc/meminfo MemAvailable" },
    })
}

#[cfg(target_os = "macos")]
#[allow(deprecated)]
pub(crate) fn memory_status() -> Result<MemoryStatus, SysError> {
    let mut total: u64 = 0;
    let mut len = size_of::<u64>();
    let rc = unsafe {
        libc::sysctlbyname(
            c"hw.memsize".as_ptr(),
            (&mut total as *mut u64).cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc != 0 {
        return Err(os_error("sysctlbyname(hw.memsize)"));
    }
    let mut stats = unsafe { std::mem::zeroed::<libc::vm_statistics64>() };
    let mut count = libc::HOST_VM_INFO64_COUNT;
    let kr = unsafe {
        libc::host_statistics64(
            libc::mach_host_self(),
            libc::HOST_VM_INFO64,
            (&mut stats as *mut libc::vm_statistics64).cast(),
            &mut count,
        )
    };
    if kr != libc::KERN_SUCCESS {
        return Err(SysError::Os { call: "host_statistics64", code: kr as u32 });
    }
    let pages = u64::from(stats.free_count)
        + u64::from(stats.inactive_count)
        + u64::from(stats.speculative_count)
        + u64::from(stats.purgeable_count);
    let available = pages.saturating_mul(page_size()).min(total);
    Ok(MemoryStatus {
        total,
        available,
        source: "macos:host_statistics64 free+inactive+speculative+purgeable",
    })
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) fn memory_status() -> Result<MemoryStatus, SysError> {
    let pages = unsafe { libc::sysconf(libc::_SC_PHYS_PAGES) };
    if pages <= 0 {
        return Err(os_error("sysconf(_SC_PHYS_PAGES)"));
    }
    let total = (pages as u64).saturating_mul(page_size());
    Ok(MemoryStatus {
        total,
        available: total,
        source: "unix:sysconf(_SC_PHYS_PAGES), total only, free RAM unknown",
    })
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
pub(crate) fn trim_heap() -> bool {
    unsafe { libc::malloc_trim(0) != 0 }
}

#[cfg(target_os = "macos")]
pub(crate) fn trim_heap() -> bool {
    extern "C" {
        fn malloc_zone_pressure_relief(zone: *mut libc::c_void, goal: libc::size_t) -> libc::size_t;
    }
    unsafe { malloc_zone_pressure_relief(std::ptr::null_mut(), 0) > 0 }
}

#[cfg(not(any(all(target_os = "linux", target_env = "gnu"), target_os = "macos")))]
pub(crate) fn trim_heap() -> bool {
    false
}
