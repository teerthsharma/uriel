use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_HANDLE_EOF, ERROR_IO_PENDING, GENERIC_READ, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, GetDiskFreeSpaceW, GetFileSizeEx, GetVolumePathNameW, ReadFile, FILE_FLAG_NO_BUFFERING,
    FILE_FLAG_OVERLAPPED, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::Threading::INFINITE;
use windows_sys::Win32::System::IO::{
    CancelIoEx, CreateIoCompletionPort, GetOverlappedResult, GetQueuedCompletionStatus, OVERLAPPED,
};

use super::win::last_os_error_code;
use super::Block;
use crate::error::SysError;

const DEFAULT_UNIT: usize = 4096;
const VOLUME_PATH_CHARS: usize = 1024;

pub(crate) struct RawJob {
    pub offset: usize,
    pub len: usize,
    pub file_offset: u64,
}

pub(crate) struct UnbufferedFile {
    file: HANDLE,
    port: HANDLE,
    poisoned: bool,
}

unsafe impl Send for UnbufferedFile {}

enum Submit {
    Pending,
    Failed(u32),
}

enum Completion {
    Packet {
        overlapped: *mut OVERLAPPED,
        ok: bool,
        code: u32,
        bytes: u32,
    },
    WaitFailed(u32),
}

fn wide_path(path: &Path) -> Result<Vec<u16>, SysError> {
    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    if wide.is_empty() || wide.contains(&0) {
        return Err(SysError::InvalidPath);
    }
    wide.push(0);
    Ok(wide)
}

fn open_file_unbuffered_overlapped(wide_with_nul: &[u16]) -> HANDLE {
    unsafe {
        CreateFileW(
            wide_with_nul.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            null(),
            OPEN_EXISTING,
            FILE_FLAG_NO_BUFFERING | FILE_FLAG_OVERLAPPED,
            null_mut(),
        )
    }
}

fn create_port_associated_with_file(file: HANDLE) -> HANDLE {
    unsafe { CreateIoCompletionPort(file, null_mut(), 0, 0) }
}

fn close_handle_owned_by_caller(handle: HANDLE) {
    unsafe {
        CloseHandle(handle);
    }
}

fn file_size_of_open_handle(file: HANDLE) -> Option<i64> {
    let mut size = 0i64;
    (unsafe { GetFileSizeEx(file, &mut size) } != 0).then_some(size)
}

fn set_overlapped_file_offset(overlapped: &mut OVERLAPPED, file_offset: u64) {
    overlapped.Anonymous.Anonymous.Offset = file_offset as u32;
    overlapped.Anonymous.Anonymous.OffsetHigh = (file_offset >> 32) as u32;
}

fn submit_read_into_block_range_kept_alive_by_caller(
    file: HANDLE,
    buffer: *mut u8,
    len: u32,
    overlapped: *mut OVERLAPPED,
) -> Submit {
    let ok = unsafe { ReadFile(file, buffer.cast(), len, null_mut(), overlapped) };
    if ok != 0 {
        return Submit::Pending;
    }
    let code = last_os_error_code();
    if code == ERROR_IO_PENDING {
        Submit::Pending
    } else {
        Submit::Failed(code)
    }
}

fn wait_for_next_completion(port: HANDLE) -> Completion {
    let mut bytes = 0u32;
    let mut key = 0usize;
    let mut overlapped: *mut OVERLAPPED = null_mut();
    let ok = unsafe { GetQueuedCompletionStatus(port, &mut bytes, &mut key, &mut overlapped, INFINITE) };
    let code = if ok != 0 { 0 } else { last_os_error_code() };
    if ok == 0 && overlapped.is_null() {
        return Completion::WaitFailed(code);
    }
    Completion::Packet {
        overlapped,
        ok: ok != 0,
        code,
        bytes,
    }
}

fn cancel_then_wait_for_overlapped_still_owned_by_caller(file: HANDLE, overlapped: *mut OVERLAPPED) {
    let mut transferred = 0u32;
    unsafe {
        CancelIoEx(file, overlapped);
        GetOverlappedResult(file, overlapped, &mut transferred, 1);
    }
}

fn index_of_overlapped(base: *mut OVERLAPPED, count: usize, packet: *mut OVERLAPPED) -> Option<usize> {
    let start = base as usize;
    let at = packet as usize;
    let size = size_of::<OVERLAPPED>();
    let distance = at.checked_sub(start)?;
    (distance % size == 0 && distance / size < count).then_some(distance / size)
}

pub(crate) fn sector_unit(path: &Path) -> usize {
    let Ok(wide) = wide_path(path) else {
        return DEFAULT_UNIT;
    };
    let mut volume = vec![0u16; VOLUME_PATH_CHARS];
    let named = unsafe { GetVolumePathNameW(wide.as_ptr(), volume.as_mut_ptr(), VOLUME_PATH_CHARS as u32) };
    if named == 0 {
        return DEFAULT_UNIT;
    }
    let (mut sectors_per_cluster, mut bytes_per_sector, mut free, mut total) = (0u32, 0u32, 0u32, 0u32);
    let ok = unsafe {
        GetDiskFreeSpaceW(
            volume.as_ptr(),
            &mut sectors_per_cluster,
            &mut bytes_per_sector,
            &mut free,
            &mut total,
        )
    };
    if ok == 0 {
        return DEFAULT_UNIT;
    }
    (bytes_per_sector as usize).max(DEFAULT_UNIT)
}

impl UnbufferedFile {
    pub(crate) fn open(path: &Path) -> Result<UnbufferedFile, SysError> {
        let wide = wide_path(path)?;
        let file = open_file_unbuffered_overlapped(&wide);
        if file == INVALID_HANDLE_VALUE || file.is_null() {
            return Err(SysError::Os {
                call: "CreateFileW",
                code: last_os_error_code(),
            });
        }
        let port = create_port_associated_with_file(file);
        if port.is_null() {
            let code = last_os_error_code();
            close_handle_owned_by_caller(file);
            return Err(SysError::Os {
                call: "CreateIoCompletionPort",
                code,
            });
        }
        Ok(UnbufferedFile {
            file,
            port,
            poisoned: false,
        })
    }

    pub(crate) fn file_len(&self) -> Result<u64, SysError> {
        file_size_of_open_handle(self.file)
            .and_then(|s| u64::try_from(s).ok())
            .ok_or(SysError::Os {
                call: "GetFileSizeEx",
                code: last_os_error_code(),
            })
    }

    pub(crate) fn read_batch(
        &mut self,
        block: &mut Block,
        jobs: &[RawJob],
    ) -> Result<Vec<usize>, SysError> {
        if self.poisoned {
            return Err(SysError::Poisoned);
        }
        let count = jobs.len();
        let mut targets: Vec<*mut u8> = Vec::with_capacity(count);
        for job in jobs {
            if job.len > u32::MAX as usize {
                return Err(SysError::TooLarge);
            }
            job.file_offset
                .checked_add(job.len as u64)
                .ok_or(SysError::OutOfRange)?;
            targets.push(block.span_ptr_checked(job.offset, job.len)?);
        }
        let mut overlapped: Box<[OVERLAPPED]> = (0..count).map(|_| OVERLAPPED::default()).collect();
        let base = overlapped.as_mut_ptr();
        let mut results = vec![0usize; count];
        let mut in_flight = vec![false; count];
        let mut pending = 0usize;
        let mut first_error: Option<SysError> = None;
        for (i, job) in jobs.iter().enumerate() {
            set_overlapped_file_offset(&mut overlapped[i], job.file_offset);
            let packet = base.wrapping_add(i);
            match submit_read_into_block_range_kept_alive_by_caller(self.file, targets[i], job.len as u32, packet) {
                Submit::Pending => {
                    in_flight[i] = true;
                    pending += 1;
                }
                Submit::Failed(code) if code == ERROR_HANDLE_EOF => {}
                Submit::Failed(code) => {
                    first_error.get_or_insert(SysError::Os {
                        call: "ReadFile",
                        code,
                    });
                    break;
                }
            }
        }
        while pending > 0 {
            match wait_for_next_completion(self.port) {
                Completion::Packet {
                    overlapped: packet,
                    ok,
                    code,
                    bytes,
                } => {
                    let Some(i) = index_of_overlapped(base, count, packet) else {
                        continue;
                    };
                    if !in_flight[i] {
                        continue;
                    }
                    in_flight[i] = false;
                    pending -= 1;
                    if ok {
                        results[i] = bytes as usize;
                    } else if code != ERROR_HANDLE_EOF {
                        first_error.get_or_insert(SysError::Os {
                            call: "GetQueuedCompletionStatus",
                            code,
                        });
                    }
                }
                Completion::WaitFailed(code) => {
                    self.poisoned = true;
                    for (i, flag) in in_flight.iter().enumerate() {
                        if *flag {
                            cancel_then_wait_for_overlapped_still_owned_by_caller(self.file, base.wrapping_add(i));
                        }
                    }
                    first_error.get_or_insert(SysError::Os {
                        call: "GetQueuedCompletionStatus",
                        code,
                    });
                    break;
                }
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(results),
        }
    }
}

impl Drop for UnbufferedFile {
    fn drop(&mut self) {
        close_handle_owned_by_caller(self.port);
        close_handle_owned_by_caller(self.file);
    }
}
