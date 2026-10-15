use std::collections::BTreeMap;

use uriel_core::Value;

use crate::error::CodecError;

pub const LENGTH_PREFIX: usize = 4;
pub const HEADER_LEN: usize = 5;
pub const DEFAULT_MAX_FRAME: usize = 1 << 20;
pub const MIN_MAX_FRAME: usize = 64;
pub const MAX_DECODE_DEPTH: usize = 64;

pub fn begin_frame(buf: &mut Vec<u8>, kind: u8) {
    buf.clear();
    buf.extend_from_slice(&[0u8; LENGTH_PREFIX]);
    buf.push(kind);
}

pub fn finish_frame(buf: &mut [u8], max_frame: usize) -> Result<(), CodecError> {
    let body = buf.len().saturating_sub(LENGTH_PREFIX);
    if body > max_frame {
        return Err(CodecError::FrameTooLarge {
            len: body,
            max: max_frame,
        });
    }
    let prefix = u32::try_from(body).map_err(|_| CodecError::FrameTooLarge {
        len: body,
        max: max_frame,
    })?;
    buf[..LENGTH_PREFIX].copy_from_slice(&prefix.to_le_bytes());
    Ok(())
}

pub fn body_len_from_prefix(prefix: [u8; LENGTH_PREFIX], max_frame: usize) -> Result<usize, CodecError> {
    let declared = u32::from_le_bytes(prefix) as usize;
    if declared == 0 {
        return Err(CodecError::EmptyFrame);
    }
    if declared > max_frame {
        return Err(CodecError::FrameTooLarge {
            len: declared,
            max: max_frame,
        });
    }
    Ok(declared)
}

pub fn parse_frame(frame: &[u8], max_frame: usize) -> Result<(u8, &[u8]), CodecError> {
    if frame.len() < LENGTH_PREFIX {
        return Err(CodecError::Truncated {
            needed: LENGTH_PREFIX,
            available: frame.len(),
        });
    }
    let mut prefix = [0u8; LENGTH_PREFIX];
    prefix.copy_from_slice(&frame[..LENGTH_PREFIX]);
    let declared = body_len_from_prefix(prefix, max_frame)?;
    let actual = frame.len() - LENGTH_PREFIX;
    if declared != actual {
        return Err(CodecError::LengthMismatch { declared, actual });
    }
    Ok((frame[LENGTH_PREFIX], &frame[HEADER_LEN..]))
}

pub fn put_u8(buf: &mut Vec<u8>, v: u8) {
    buf.push(v);
}

pub fn put_u32(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_le_bytes());
}

pub fn put_u64(buf: &mut Vec<u8>, v: u64) {
    buf.extend_from_slice(&v.to_le_bytes());
}

pub fn put_i64(buf: &mut Vec<u8>, v: i64) {
    buf.extend_from_slice(&v.to_le_bytes());
}

pub fn put_f32(buf: &mut Vec<u8>, v: f32) {
    buf.extend_from_slice(&v.to_bits().to_le_bytes());
}

pub fn put_f64(buf: &mut Vec<u8>, v: f64) {
    buf.extend_from_slice(&v.to_bits().to_le_bytes());
}

pub fn put_len(buf: &mut Vec<u8>, len: usize) {
    put_u32(buf, u32::try_from(len).unwrap_or(u32::MAX));
}

pub fn put_usize(buf: &mut Vec<u8>, v: usize) {
    put_u64(buf, v as u64);
}

pub fn put_bytes(buf: &mut Vec<u8>, bytes: &[u8]) {
    put_len(buf, bytes.len());
    buf.extend_from_slice(bytes);
}

pub fn put_str(buf: &mut Vec<u8>, s: &str) {
    put_bytes(buf, s.as_bytes());
}

pub fn put_map(
    buf: &mut Vec<u8>,
    map: &BTreeMap<String, Value>,
    depth: usize,
) -> Result<(), CodecError> {
    put_len(buf, map.len());
    for (k, v) in map {
        put_str(buf, k);
        put_value(buf, v, depth)?;
    }
    Ok(())
}

pub fn put_value(buf: &mut Vec<u8>, value: &Value, depth: usize) -> Result<(), CodecError> {
    if depth > MAX_DECODE_DEPTH {
        return Err(CodecError::TooDeep(MAX_DECODE_DEPTH));
    }
    match value {
        Value::Null => put_u8(buf, 0),
        Value::Bool(b) => {
            put_u8(buf, 1);
            put_u8(buf, u8::from(*b));
        }
        Value::Int(i) => {
            put_u8(buf, 2);
            put_i64(buf, *i);
        }
        Value::Float(f) => {
            put_u8(buf, 3);
            put_f64(buf, *f);
        }
        Value::String(s) => {
            put_u8(buf, 4);
            put_str(buf, s);
        }
        Value::Bytes(b) => {
            put_u8(buf, 5);
            put_bytes(buf, b);
        }
        Value::List(items) => {
            put_u8(buf, 6);
            put_len(buf, items.len());
            for item in items {
                put_value(buf, item, depth + 1)?;
            }
        }
        Value::Map(map) => {
            put_u8(buf, 7);
            put_map(buf, map, depth + 1)?;
        }
        Value::Vector(items) => {
            put_u8(buf, 8);
            put_len(buf, items.len());
            for x in items {
                put_f32(buf, *x);
            }
        }
    }
    Ok(())
}

pub struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    pub fn new(data: &'a [u8]) -> Cursor<'a> {
        Cursor { data, pos: 0 }
    }

    pub fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    pub fn finish(self) -> Result<(), CodecError> {
        match self.remaining() {
            0 => Ok(()),
            n => Err(CodecError::Trailing(n)),
        }
    }

    pub fn take(&mut self, n: usize) -> Result<&'a [u8], CodecError> {
        let available = self.remaining();
        if n > available {
            return Err(CodecError::Truncated {
                needed: n,
                available,
            });
        }
        let slice = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(slice)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], CodecError> {
        let slice = self.take(N)?;
        let mut out = [0u8; N];
        out.copy_from_slice(slice);
        Ok(out)
    }

    pub fn u8(&mut self) -> Result<u8, CodecError> {
        Ok(self.array::<1>()?[0])
    }

    pub fn u32(&mut self) -> Result<u32, CodecError> {
        Ok(u32::from_le_bytes(self.array::<4>()?))
    }

    pub fn u64(&mut self) -> Result<u64, CodecError> {
        Ok(u64::from_le_bytes(self.array::<8>()?))
    }

    pub fn i64(&mut self) -> Result<i64, CodecError> {
        Ok(i64::from_le_bytes(self.array::<8>()?))
    }

    pub fn f32(&mut self) -> Result<f32, CodecError> {
        Ok(f32::from_bits(u32::from_le_bytes(self.array::<4>()?)))
    }

    pub fn f64(&mut self) -> Result<f64, CodecError> {
        Ok(f64::from_bits(u64::from_le_bytes(self.array::<8>()?)))
    }

    pub fn usize(&mut self) -> Result<usize, CodecError> {
        let v = self.u64()?;
        usize::try_from(v).map_err(|_| CodecError::IntegerOverflow(v))
    }

    pub fn bool(&mut self) -> Result<bool, CodecError> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            other => Err(CodecError::UnknownTag(other)),
        }
    }

    pub fn count(&mut self, min_item_bytes: usize) -> Result<usize, CodecError> {
        let count = self.u32()? as usize;
        let available = self.remaining();
        let fits = count
            .checked_mul(min_item_bytes.max(1))
            .is_some_and(|need| need <= available);
        if fits {
            Ok(count)
        } else {
            Err(CodecError::CountOverflow { count, available })
        }
    }

    pub fn bytes(&mut self) -> Result<Vec<u8>, CodecError> {
        let n = self.count(1)?;
        Ok(self.take(n)?.to_vec())
    }

    pub fn string(&mut self) -> Result<String, CodecError> {
        let n = self.count(1)?;
        let raw = self.take(n)?;
        std::str::from_utf8(raw)
            .map(str::to_owned)
            .map_err(|_| CodecError::InvalidUtf8)
    }

    pub fn map(&mut self, depth: usize) -> Result<BTreeMap<String, Value>, CodecError> {
        let n = self.count(5)?;
        let mut out = BTreeMap::new();
        for _ in 0..n {
            let key = self.string()?;
            let value = self.value(depth)?;
            out.insert(key, value);
        }
        Ok(out)
    }

    pub fn value(&mut self, depth: usize) -> Result<Value, CodecError> {
        if depth > MAX_DECODE_DEPTH {
            return Err(CodecError::TooDeep(MAX_DECODE_DEPTH));
        }
        match self.u8()? {
            0 => Ok(Value::Null),
            1 => Ok(Value::Bool(self.bool()?)),
            2 => Ok(Value::Int(self.i64()?)),
            3 => Ok(Value::Float(self.f64()?)),
            4 => Ok(Value::String(self.string()?)),
            5 => Ok(Value::Bytes(self.bytes()?)),
            6 => {
                let n = self.count(1)?;
                let mut items = Vec::with_capacity(n);
                for _ in 0..n {
                    items.push(self.value(depth + 1)?);
                }
                Ok(Value::List(items))
            }
            7 => Ok(Value::Map(self.map(depth + 1)?)),
            8 => {
                let n = self.count(4)?;
                let mut items = Vec::with_capacity(n);
                for _ in 0..n {
                    items.push(self.f32()?);
                }
                Ok(Value::Vector(items))
            }
            other => Err(CodecError::UnknownTag(other)),
        }
    }
}
