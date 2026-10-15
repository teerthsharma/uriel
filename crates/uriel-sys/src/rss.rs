use crate::error::SysError;
use crate::raw;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessMemory {
    pub working_set: u64,
    pub private_bytes: u64,
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

pub fn large_page_status() -> LargePageStatus {
    raw::large_page_status()
}

pub fn large_page_privilege_held() -> bool {
    matches!(large_page_status(), LargePageStatus::Ready { .. })
}
