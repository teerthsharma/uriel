#![deny(unsafe_code)]
#![deny(unsafe_op_in_unsafe_fn)]

mod arena;
mod cap;
mod csr;
mod error;
#[allow(unsafe_code)]
mod raw;
mod reader;
mod region;
mod rss;

pub use arena::{Arena, Slab, SLAB_ALIGN};
pub use cap::{Cap, Read, Width, Word, Write};
pub use csr::HotCsr;
pub use error::SysError;
pub use reader::{
    AsyncReader, ReadJob, ReaderBackend, ReaderMode, MAX_BATCH, MAX_JOB_BYTES, MIN_ALIGNMENT_UNIT,
};
pub use region::{Backend, FallbackReason, Region, Request};
pub use rss::{large_page_privilege_held, large_page_status, process_memory, LargePageStatus, ProcessMemory};
