//! Binary codec for node and relationship records stored in `redb`.
//!
//! This is *not* the order-preserving index key encoding (see
//! [`Value::to_key_bytes`]); it is compact, bit-exact (it keeps `-0.0` and NaN payloads)
//! and defensive on decode: every length is bounds-checked, nesting is limited, and
//! truncated or trailing bytes produce [`Error::Corrupt`] instead of a panic.

use std::collections::{BTreeMap, BTreeSet};

use crate::error::{Error, Result};
use crate::ids::{Node, NodeId, Props, Rel, RelId};
use crate::value::Value;

/// Record layout version byte.
const RECORD_VERSION: u8 = 1;
/// Maximum nesting accepted by the decoder (writers are limited to a smaller depth).
const MAX_DECODE_DEPTH: usize = 128;

const T_NULL: u8 = 0;
const T_FALSE: u8 = 1;
const T_TRUE: u8 = 2;
const T_INT: u8 = 3;
const T_FLOAT: u8 = 4;
const T_STRING: u8 = 5;
const T_BYTES: u8 = 6;
const T_LIST: u8 = 7;
const T_MAP: u8 = 8;
const T_VECTOR: u8 = 9;

pub(crate) fn put_u32(out: &mut Vec<u8>, n: usize) {
    let n = u32::try_from(n).expect("collection larger than u32::MAX elements");
    out.extend_from_slice(&n.to_le_bytes());
}

pub(crate) fn put_str(out: &mut Vec<u8>, s: &str) {
    put_u32(out, s.len());
    out.extend_from_slice(s.as_bytes());
}

/// Appends the binary encoding of `v` to `out`.
pub(crate) fn encode_value(v: &Value, out: &mut Vec<u8>) {
    match v {
        Value::Null => out.push(T_NULL),
        Value::Bool(false) => out.push(T_FALSE),
        Value::Bool(true) => out.push(T_TRUE),
        Value::Int(i) => {
            out.push(T_INT);
            out.extend_from_slice(&i.to_le_bytes());
        }
        Value::Float(f) => {
            out.push(T_FLOAT);
            out.extend_from_slice(&f.to_bits().to_le_bytes());
        }
        Value::String(s) => {
            out.push(T_STRING);
            put_str(out, s);
        }
        Value::Bytes(b) => {
            out.push(T_BYTES);
            put_u32(out, b.len());
            out.extend_from_slice(b);
        }
        Value::List(items) => {
            out.push(T_LIST);
            put_u32(out, items.len());
            for item in items {
                encode_value(item, out);
            }
        }
        Value::Map(m) => {
            out.push(T_MAP);
            put_u32(out, m.len());
            for (k, val) in m {
                put_str(out, k);
                encode_value(val, out);
            }
        }
        Value::Vector(xs) => {
            out.push(T_VECTOR);
            put_u32(out, xs.len());
            for x in xs {
                out.extend_from_slice(&x.to_bits().to_le_bytes());
            }
        }
    }
}

pub(crate) fn put_props(out: &mut Vec<u8>, props: &Props) {
    put_u32(out, props.len());
    for (k, v) in props {
        put_str(out, k);
        encode_value(v, out);
    }
}

/// Encodes a node record.
pub(crate) fn encode_node(labels: &BTreeSet<String>, props: &Props) -> Vec<u8> {
    let mut out = vec![RECORD_VERSION];
    put_u32(&mut out, labels.len());
    for l in labels {
        put_str(&mut out, l);
    }
    put_props(&mut out, props);
    out
}

/// Encodes a relationship record.
pub(crate) fn encode_rel(src: NodeId, dst: NodeId, ty: &str, props: &Props) -> Vec<u8> {
    let mut out = vec![RECORD_VERSION];
    out.extend_from_slice(&src.0.to_le_bytes());
    out.extend_from_slice(&dst.0.to_le_bytes());
    put_str(&mut out, ty);
    put_props(&mut out, props);
    out
}

pub(crate) struct Reader<'a> {
    buf: &'a [u8],
}

fn corrupt(msg: &str) -> Error {
    Error::Corrupt(msg.to_string())
}

impl<'a> Reader<'a> {
    pub(crate) fn new(buf: &'a [u8]) -> Reader<'a> {
        Reader { buf }
    }

    pub(crate) fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if n > self.buf.len() {
            return Err(corrupt("record truncated"));
        }
        let (head, tail) = self.buf.split_at(n);
        self.buf = tail;
        Ok(head)
    }

    pub(crate) fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    pub(crate) fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().expect("4 bytes"),
        ))
    }

    pub(crate) fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(
            self.take(8)?.try_into().expect("8 bytes"),
        ))
    }

    /// Reads an element count and checks it against the bytes left, given that every
    /// element needs at least `min_elem` bytes. Stops hostile counts from allocating.
    pub(crate) fn count(&mut self, min_elem: usize) -> Result<usize> {
        let n = self.u32()? as usize;
        if n.saturating_mul(min_elem) > self.buf.len() {
            return Err(corrupt("element count exceeds record size"));
        }
        Ok(n)
    }

    pub(crate) fn string(&mut self) -> Result<String> {
        let n = self.u32()? as usize;
        let bytes = self.take(n)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| corrupt("string is not valid UTF-8"))
    }

    fn value(&mut self, depth: usize) -> Result<Value> {
        if depth > MAX_DECODE_DEPTH {
            return Err(corrupt("value nesting too deep"));
        }
        Ok(match self.u8()? {
            T_NULL => Value::Null,
            T_FALSE => Value::Bool(false),
            T_TRUE => Value::Bool(true),
            T_INT => Value::Int(self.u64()? as i64),
            T_FLOAT => Value::Float(f64::from_bits(self.u64()?)),
            T_STRING => Value::String(self.string()?),
            T_BYTES => {
                let n = self.u32()? as usize;
                Value::Bytes(self.take(n)?.to_vec())
            }
            T_LIST => {
                let n = self.count(1)?;
                let mut items = Vec::with_capacity(n);
                for _ in 0..n {
                    items.push(self.value(depth + 1)?);
                }
                Value::List(items)
            }
            T_MAP => {
                let n = self.count(5)?;
                let mut m = BTreeMap::new();
                for _ in 0..n {
                    let k = self.string()?;
                    m.insert(k, self.value(depth + 1)?);
                }
                Value::Map(m)
            }
            T_VECTOR => {
                let n = self.count(4)?;
                let mut xs = Vec::with_capacity(n);
                for _ in 0..n {
                    xs.push(f32::from_bits(self.u32()?));
                }
                Value::Vector(xs)
            }
            other => return Err(Error::Corrupt(format!("unknown value tag {other}"))),
        })
    }

    pub(crate) fn props(&mut self) -> Result<Props> {
        let n = self.count(6)?;
        let mut props = Props::new();
        for _ in 0..n {
            let k = self.string()?;
            props.insert(k, self.value(0)?);
        }
        Ok(props)
    }

    fn header(&mut self) -> Result<()> {
        match self.u8()? {
            RECORD_VERSION => Ok(()),
            v => Err(Error::Corrupt(format!("unknown record version {v}"))),
        }
    }

    pub(crate) fn finish(&self) -> Result<()> {
        if self.buf.is_empty() {
            Ok(())
        } else {
            Err(corrupt("trailing bytes after record"))
        }
    }
}

#[cfg(test)]
pub(crate) fn decode_value(bytes: &[u8]) -> Result<Value> {
    let mut r = Reader { buf: bytes };
    let v = r.value(0)?;
    r.finish()?;
    Ok(v)
}

fn untag(bytes: &[u8]) -> Result<std::borrow::Cow<'_, [u8]>> {
    uriel_tier::decode_value_cow(bytes).map_err(|e| Error::Corrupt(e.to_string()))
}

/// Decodes a node record.
pub(crate) fn decode_node(id: NodeId, bytes: &[u8]) -> Result<Node> {
    let bytes = untag(bytes)?;
    let mut r = Reader { buf: &bytes };
    r.header()?;
    let n = r.count(4)?;
    let mut labels = BTreeSet::new();
    for _ in 0..n {
        labels.insert(r.string()?);
    }
    let props = r.props()?;
    r.finish()?;
    Ok(Node { id, labels, props })
}

/// Decodes a relationship record.
pub(crate) fn decode_rel(id: RelId, bytes: &[u8]) -> Result<Rel> {
    let bytes = untag(bytes)?;
    let mut r = Reader { buf: &bytes };
    r.header()?;
    let src = NodeId(r.u64()?);
    let dst = NodeId(r.u64()?);
    let rel_type = r.string()?;
    let props = r.props()?;
    r.finish()?;
    Ok(Rel {
        id,
        src,
        dst,
        rel_type,
        props,
    })
}
