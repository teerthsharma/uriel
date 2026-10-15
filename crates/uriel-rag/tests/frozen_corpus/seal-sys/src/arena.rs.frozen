use crate::cap::{Cap, Read, Width, Write};
use crate::error::SysError;
use crate::region::{Region, Request};

pub const SLAB_ALIGN: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slab {
    read: Cap<Read>,
    write: Cap<Write>,
    padded: usize,
}

impl Slab {
    pub fn read_cap(&self) -> Cap<Read> {
        self.read
    }

    pub fn write_cap(&self) -> Cap<Write> {
        self.write
    }

    pub fn offset(&self) -> usize {
        self.read.offset()
    }

    pub fn len(&self) -> usize {
        self.read.len()
    }

    pub fn is_empty(&self) -> bool {
        self.read.is_empty()
    }

    pub fn padded_len(&self) -> usize {
        self.padded
    }
}

pub struct Arena {
    region: Region,
    cursor: usize,
    cap: usize,
}

pub(crate) fn round_up_slab(bytes: usize) -> Option<usize> {
    bytes.checked_add(SLAB_ALIGN - 1).map(|v| v & !(SLAB_ALIGN - 1))
}

impl Arena {
    pub fn with_cap(bytes: usize) -> Result<Arena, SysError> {
        Arena::with_cap_pages(bytes, false)
    }

    pub fn with_cap_pages(bytes: usize, prefer_huge: bool) -> Result<Arena, SysError> {
        let cap = bytes & !(SLAB_ALIGN - 1);
        if cap == 0 {
            return Err(SysError::CapTooSmall);
        }
        let region = Region::allocate(Request {
            bytes: cap,
            align: SLAB_ALIGN,
            prefer_huge,
        })?;
        Ok(Arena {
            region,
            cursor: 0,
            cap,
        })
    }

    pub fn cap(&self) -> usize {
        self.cap
    }

    pub fn used(&self) -> usize {
        self.cursor
    }

    pub fn remaining(&self) -> usize {
        self.cap - self.cursor
    }

    pub fn region(&self) -> &Region {
        &self.region
    }

    pub fn region_mut(&mut self) -> &mut Region {
        &mut self.region
    }

    pub fn alloc(&mut self, bytes: usize, width: Width) -> Result<Slab, SysError> {
        if bytes == 0 {
            return Err(SysError::ZeroSize);
        }
        let remaining = self.remaining();
        let padded = match round_up_slab(bytes) {
            Some(p) if p <= remaining => p,
            Some(p) => {
                return Err(SysError::CapExceeded {
                    requested: p,
                    remaining,
                })
            }
            None => {
                return Err(SysError::CapExceeded {
                    requested: usize::MAX,
                    remaining,
                })
            }
        };
        let read = self.region.cap_read(self.cursor, bytes, width)?;
        let write = self.region.cap_write(self.cursor, bytes, width)?;
        self.cursor += padded;
        Ok(Slab {
            read,
            write,
            padded,
        })
    }

    pub fn reset(&mut self) -> Result<(), SysError> {
        if self.cursor > 0 {
            let all = self.region.cap_write(0, self.cursor, Width::W8)?;
            self.region.view_mut::<u8>(&all)?.fill(0);
        }
        self.region.revoke_all_caps();
        self.cursor = 0;
        Ok(())
    }
}
