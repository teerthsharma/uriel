use std::io;
use std::sync::{Mutex, MutexGuard, PoisonError};

use redb::StorageBackend;

#[derive(Debug)]
struct State {
    len: u64,
    base_cut: u64,
    patches: Vec<(u64, Vec<u8>)>,
}

#[derive(Debug)]
pub(crate) struct Overlay<B> {
    base: B,
    state: Mutex<State>,
}

impl<B: StorageBackend> Overlay<B> {
    pub(crate) fn new(base: B) -> io::Result<Self> {
        let len = base.len()?;
        Ok(Overlay {
            base,
            state: Mutex::new(State {
                len,
                base_cut: len,
                patches: Vec::new(),
            }),
        })
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl<B: StorageBackend> StorageBackend for Overlay<B> {
    fn len(&self) -> io::Result<u64> {
        Ok(self.state().len)
    }

    fn read(&self, offset: u64, out: &mut [u8]) -> io::Result<()> {
        let st = self.state();
        let end = offset + out.len() as u64;
        if end > st.len {
            return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
        }
        out.fill(0);
        let from_base = end.min(st.base_cut);
        if offset < from_base {
            let n = (from_base - offset) as usize;
            self.base.read(offset, &mut out[..n])?;
        }
        for (at, data) in &st.patches {
            let lo = offset.max(*at);
            let hi = end.min(*at + data.len() as u64);
            if lo < hi {
                let src = &data[(lo - at) as usize..(hi - at) as usize];
                out[(lo - offset) as usize..(hi - offset) as usize].copy_from_slice(src);
            }
        }
        Ok(())
    }

    fn set_len(&self, len: u64) -> io::Result<()> {
        let mut st = self.state();
        st.len = len;
        st.base_cut = st.base_cut.min(len);
        st.patches.retain_mut(|(at, data)| {
            if *at >= len {
                return false;
            }
            let keep = (len - *at).min(data.len() as u64) as usize;
            data.truncate(keep);
            true
        });
        Ok(())
    }

    fn sync_data(&self) -> io::Result<()> {
        Ok(())
    }

    fn write(&self, offset: u64, data: &[u8]) -> io::Result<()> {
        let mut st = self.state();
        let end = offset + data.len() as u64;
        st.len = st.len.max(end);
        st.patches.push((offset, data.to_vec()));
        Ok(())
    }

    fn close(&self) -> io::Result<()> {
        self.base.close()
    }
}
