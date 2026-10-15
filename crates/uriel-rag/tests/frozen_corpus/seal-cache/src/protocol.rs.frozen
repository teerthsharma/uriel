//! TCP cache protocol: `u32` big-endian length prefix + bincode payload.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::error::CacheError;
use crate::local::{CacheEntry, CacheMeta};

/// An entry on the wire; `ttl_ms` is the remaining time to live.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct WireEntry {
    pub value: Vec<u8>,
    pub epoch: u64,
    pub tags: Vec<String>,
    pub ttl_ms: Option<u64>,
}

impl WireEntry {
    pub(crate) fn from_entry(e: &CacheEntry) -> Self {
        WireEntry {
            value: e.value.to_vec(),
            epoch: e.meta.epoch,
            tags: e.meta.tags.clone(),
            ttl_ms: e.meta.ttl.map(ttl_to_ms),
        }
    }

    pub(crate) fn into_parts(self) -> (Vec<u8>, CacheMeta) {
        let meta = CacheMeta {
            epoch: self.epoch,
            tags: self.tags,
            ttl: self.ttl_ms.map(Duration::from_millis),
        };
        (self.value, meta)
    }
}

/// Whole milliseconds, rounded down so a ttl is never extended in transit.
pub(crate) fn ttl_to_ms(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Request {
    Get { key: String },
    Put { key: String, entry: WireEntry },
    Invalidate { tags: Vec<String>, epoch: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Response {
    Value(Option<WireEntry>),
    Stored(bool),
    Invalidated(u64),
}

pub(crate) async fn write_frame<W, T>(
    w: &mut W,
    msg: &T,
    max_frame: usize,
) -> Result<(), CacheError>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let payload = bincode::serialize(msg).map_err(|e| CacheError::Protocol(e.to_string()))?;
    if payload.len() > max_frame {
        return Err(CacheError::Protocol(format!(
            "frame of {} bytes exceeds limit {max_frame}",
            payload.len()
        )));
    }
    let mut buf = Vec::with_capacity(4 + payload.len());
    buf.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    buf.extend_from_slice(&payload);
    w.write_all(&buf).await?;
    w.flush().await?;
    Ok(())
}

pub(crate) async fn read_frame<R, T>(r: &mut R, max_frame: usize) -> Result<T, CacheError>
where
    R: AsyncRead + Unpin,
    T: for<'de> Deserialize<'de>,
{
    let mut header = [0u8; 4];
    r.read_exact(&mut header).await?;
    let len = u32::from_be_bytes(header) as usize;
    if len > max_frame {
        // Reject before allocating: the length comes from the peer.
        return Err(CacheError::Protocol(format!(
            "frame of {len} bytes exceeds limit {max_frame}"
        )));
    }
    let mut payload = vec![0u8; len];
    r.read_exact(&mut payload).await?;
    bincode::deserialize(&payload).map_err(|e| CacheError::Protocol(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::io::duplex;

    #[tokio::test]
    async fn frames_roundtrip_back_to_back() {
        let (mut a, mut b) = duplex(4096);
        let r1 = Request::Get { key: "k".into() };
        let r2 = Request::Invalidate {
            tags: vec!["Person".into(), "KNOWS".into()],
            epoch: 9,
        };
        write_frame(&mut a, &r1, 1 << 20).await.unwrap();
        write_frame(&mut a, &r2, 1 << 20).await.unwrap();
        assert_eq!(read_frame::<_, Request>(&mut b, 1 << 20).await.unwrap(), r1);
        assert_eq!(read_frame::<_, Request>(&mut b, 1 << 20).await.unwrap(), r2);
    }

    #[tokio::test]
    async fn oversize_header_is_rejected_before_allocating() {
        let (mut a, mut b) = duplex(64);
        a.write_all(&u32::MAX.to_be_bytes()).await.unwrap();
        let err = read_frame::<_, Request>(&mut b, 1024).await.unwrap_err();
        assert!(matches!(err, CacheError::Protocol(_)), "{err:?}");
    }

    #[tokio::test]
    async fn oversize_payload_is_refused_on_write() {
        let (mut a, _b) = duplex(64);
        let big = Response::Value(Some(WireEntry {
            value: vec![0; 10_000],
            epoch: 1,
            tags: vec![],
            ttl_ms: None,
        }));
        let err = write_frame(&mut a, &big, 1024).await.unwrap_err();
        assert!(matches!(err, CacheError::Protocol(_)), "{err:?}");
    }

    #[tokio::test]
    async fn truncated_frame_is_an_io_error() {
        let (mut a, mut b) = duplex(64);
        a.write_all(&100u32.to_be_bytes()).await.unwrap();
        a.write_all(&[1, 2, 3]).await.unwrap();
        drop(a);
        let err = read_frame::<_, Request>(&mut b, 1024).await.unwrap_err();
        assert!(matches!(err, CacheError::Io(_)), "{err:?}");
    }

    #[tokio::test]
    async fn garbage_payload_is_a_protocol_error() {
        let (mut a, mut b) = duplex(64);
        a.write_all(&3u32.to_be_bytes()).await.unwrap();
        a.write_all(&[0xFF, 0xFF, 0xFF]).await.unwrap();
        let err = read_frame::<_, Request>(&mut b, 1024).await.unwrap_err();
        assert!(matches!(err, CacheError::Protocol(_)), "{err:?}");
    }

    #[test]
    fn wire_entry_preserves_meta() {
        let e = CacheEntry {
            value: Arc::from(vec![1u8, 2, 3]),
            meta: CacheMeta::new(7)
                .with_tags(["A", "B"])
                .with_ttl(Duration::from_millis(1500)),
        };
        let w = WireEntry::from_entry(&e);
        assert_eq!(w.ttl_ms, Some(1500));
        let (v, m) = w.into_parts();
        assert_eq!(v, vec![1, 2, 3]);
        assert_eq!(m.epoch, 7);
        assert_eq!(m.tags, vec!["A".to_string(), "B".to_string()]);
        assert_eq!(m.ttl, Some(Duration::from_millis(1500)));
    }
}
