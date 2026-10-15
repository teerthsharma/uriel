use std::fs::File;
use std::path::Path;

use crate::cap::{Cap, Width, Write};
use crate::error::SysError;
use crate::raw;
use crate::region::Region;

pub const MAX_BATCH: usize = 64;
pub const MAX_JOB_BYTES: usize = 1 << 30;
pub const MIN_ALIGNMENT_UNIT: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadJob {
    pub cap: Cap<Write>,
    pub file_offset: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReaderMode {
    Auto,
    ForceBuffered,
    AutoWithFailedUnbufferedOpen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReaderBackend {
    Unbuffered,
    Buffered,
}

enum Inner {
    #[cfg(windows)]
    Unbuffered(raw::UnbufferedFile),
    Buffered(File),
}

pub struct AsyncReader {
    inner: Inner,
    unit: usize,
    file_len: u64,
    fallback: Option<SysError>,
}

fn io_error(error: std::io::Error) -> SysError {
    SysError::Io(error.kind())
}

#[cfg(windows)]
fn read_at_offset(file: &File, buf: &mut [u8], offset: u64) -> std::io::Result<usize> {
    std::os::windows::fs::FileExt::seek_read(file, buf, offset)
}

#[cfg(unix)]
fn read_at_offset(file: &File, buf: &mut [u8], offset: u64) -> std::io::Result<usize> {
    std::os::unix::fs::FileExt::read_at(file, buf, offset)
}

#[cfg(not(any(windows, unix)))]
fn read_at_offset(_file: &File, _buf: &mut [u8], _offset: u64) -> std::io::Result<usize> {
    Err(std::io::Error::from(std::io::ErrorKind::Unsupported))
}

fn read_fully_buffered(file: &File, buf: &mut [u8], offset: u64) -> Result<usize, SysError> {
    let mut done = 0usize;
    while done < buf.len() {
        let at = offset
            .checked_add(done as u64)
            .ok_or(SysError::OutOfRange)?;
        let n = read_at_offset(file, &mut buf[done..], at).map_err(io_error)?;
        if n == 0 {
            break;
        }
        done += n;
    }
    Ok(done)
}

impl AsyncReader {
    pub fn open<P: AsRef<Path>>(path: P) -> Result<AsyncReader, SysError> {
        AsyncReader::open_with(path, ReaderMode::Auto)
    }

    pub fn open_with<P: AsRef<Path>>(path: P, mode: ReaderMode) -> Result<AsyncReader, SysError> {
        let path = path.as_ref();
        let unit = raw::alignment_unit(path).max(MIN_ALIGNMENT_UNIT);
        let fallback = match mode {
            ReaderMode::ForceBuffered => None,
            ReaderMode::AutoWithFailedUnbufferedOpen => Some(SysError::Os {
                call: "CreateFileW(FILE_FLAG_NO_BUFFERING) injected failure",
                code: 0,
            }),
            ReaderMode::Auto => match AsyncReader::open_unbuffered(path, unit) {
                Ok(reader) => return Ok(reader),
                Err(e) => Some(e),
            },
        };
        let file = File::open(path).map_err(io_error)?;
        let file_len = file.metadata().map_err(io_error)?.len();
        Ok(AsyncReader {
            inner: Inner::Buffered(file),
            unit,
            file_len,
            fallback,
        })
    }

    #[cfg(windows)]
    fn open_unbuffered(path: &Path, unit: usize) -> Result<AsyncReader, SysError> {
        let file = raw::UnbufferedFile::open(path)?;
        let file_len = file.file_len()?;
        Ok(AsyncReader {
            inner: Inner::Unbuffered(file),
            unit,
            file_len,
            fallback: None,
        })
    }

    #[cfg(not(windows))]
    fn open_unbuffered(_path: &Path, _unit: usize) -> Result<AsyncReader, SysError> {
        Err(SysError::Unsupported)
    }

    pub fn fallback_reason(&self) -> Option<&SysError> {
        self.fallback.as_ref()
    }

    pub fn backend(&self) -> ReaderBackend {
        match &self.inner {
            #[cfg(windows)]
            Inner::Unbuffered(_) => ReaderBackend::Unbuffered,
            Inner::Buffered(_) => ReaderBackend::Buffered,
        }
    }

    pub fn alignment_unit(&self) -> usize {
        self.unit
    }

    pub fn file_len(&self) -> u64 {
        self.file_len
    }

    pub fn read_at(
        &mut self,
        region: &mut Region,
        cap: &Cap<Write>,
        file_offset: u64,
    ) -> Result<usize, SysError> {
        let jobs = [ReadJob {
            cap: *cap,
            file_offset,
        }];
        let done = self.read_batch(region, &jobs)?;
        done.first().copied().ok_or(SysError::OutOfRange)
    }

    pub fn read_batch(
        &mut self,
        region: &mut Region,
        jobs: &[ReadJob],
    ) -> Result<Vec<usize>, SysError> {
        if jobs.len() > MAX_BATCH {
            return Err(SysError::TooManyJobs);
        }
        let unit = self.unit;
        let mut spans: Vec<(usize, usize)> = Vec::with_capacity(jobs.len());
        for job in jobs {
            region.check_cap(&job.cap)?;
            if job.cap.width() != Width::W8 {
                return Err(SysError::WrongWidth);
            }
            if job.cap.len() > MAX_JOB_BYTES {
                return Err(SysError::TooLarge);
            }
            if job.file_offset % unit as u64 != 0 || job.cap.len() % unit != 0 {
                return Err(SysError::Misaligned);
            }
            if region.alignment_of(&job.cap)? < unit {
                return Err(SysError::Misaligned);
            }
            job.file_offset
                .checked_add(job.cap.len() as u64)
                .ok_or(SysError::OutOfRange)?;
            spans.push((job.cap.offset(), job.cap.offset() + job.cap.len()));
        }
        spans.sort_unstable();
        if spans.windows(2).any(|w| w[0].1 > w[1].0) {
            return Err(SysError::Overlap);
        }
        match &mut self.inner {
            #[cfg(windows)]
            Inner::Unbuffered(file) => {
                let raw_jobs: Vec<raw::RawJob> = jobs
                    .iter()
                    .map(|j| raw::RawJob {
                        offset: j.cap.offset(),
                        len: j.cap.len(),
                        file_offset: j.file_offset,
                    })
                    .collect();
                file.read_batch(region.block_mut(), &raw_jobs)
            }
            Inner::Buffered(file) => {
                let mut done = Vec::with_capacity(jobs.len());
                for job in jobs {
                    let buf = region.view_mut::<u8>(&job.cap)?;
                    done.push(read_fully_buffered(file, buf, job.file_offset)?);
                }
                Ok(done)
            }
        }
    }
}
