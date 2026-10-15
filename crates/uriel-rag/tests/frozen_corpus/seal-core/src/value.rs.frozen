//! The property value model.
//!
//! [`Value`] has a **total order** (so it can be sorted, used as a B-tree key and compared
//! without partiality) and an **order-preserving, prefix-free byte encoding**
//! ([`Value::to_key_bytes`]) used for property index keys. Both are defined from the same
//! rules, and the tests check that they agree on every pair of values in a large pool.
//!
//! # Ordering
//!
//! Values of different kinds sort by kind:
//! `Null < Bool < Number < String < Bytes < List < Map < Vector`.
//!
//! * `Int` and `Float` form one *Number* kind and compare by exact mathematical value, so
//!   `Int(1) == Float(1.0)` and `Int(i64::MAX) < Float(2^63)`. `-0.0` equals `0.0`. `NaN`
//!   equals itself and sorts above every other number (a total order needs this; it is
//!   storage equality, not Cypher `=`).
//! * Strings compare by UTF-8 bytes; bytes lexicographically.
//! * Lists compare element-wise, then by length. Maps compare as their sorted
//!   `(key, value)` sequences. Vectors compare element-wise with the same float rules.
//!
//! # JSON
//!
//! [`Value::to_json`] / [`Value::from_json`] (also the `serde` impls) are lossless.
//! `Null`, `Bool`, `Int`, `Float` (finite), `String`, `List` and `Map` use their natural JSON
//! shape. Values JSON cannot express natively use a reserved tag object
//! `{"$seal": "<tag>", "v": ...}`: `bytes` (hex string), `vector` (array of numbers),
//! `float` (`"NaN"`, `"Infinity"`, `"-Infinity"`) and `map` (a user map that itself
//! contains a `"$seal"` key, so it can never be mistaken for a tag).

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::{Error, Result};

/// A property value.
#[derive(Debug, Clone)]
pub enum Value {
    /// Absence of a value. Storing `Null` as a property removes the property.
    Null,
    /// Boolean.
    Bool(bool),
    /// 64-bit signed integer.
    Int(i64),
    /// 64-bit float.
    Float(f64),
    /// UTF-8 string.
    String(String),
    /// Arbitrary bytes.
    Bytes(Vec<u8>),
    /// Heterogeneous list.
    List(Vec<Value>),
    /// String-keyed map, ordered by key.
    Map(BTreeMap<String, Value>),
    /// Dense `f32` embedding vector.
    Vector(Vec<f32>),
}

const TAG_NULL: u8 = 1;
const TAG_BOOL: u8 = 2;
const TAG_NUMBER: u8 = 3;
const TAG_STRING: u8 = 4;
const TAG_BYTES: u8 = 5;
const TAG_LIST: u8 = 6;
const TAG_MAP: u8 = 7;
const TAG_VECTOR: u8 = 8;

/// Reserved JSON key marking a tagged value.
const JSON_TAG: &str = "$seal";

fn canon_f64(f: f64) -> f64 {
    if f.is_nan() {
        f64::NAN
    } else if f == 0.0 {
        0.0
    } else {
        f
    }
}

fn canon_f32(f: f32) -> f32 {
    if f.is_nan() {
        f32::NAN
    } else if f == 0.0 {
        0.0
    } else {
        f
    }
}

/// Splits a number into `(nearest f64, exact remainder)` such that comparing the pairs
/// lexicographically equals comparing the exact mathematical values. Floats and integers
/// with magnitude up to 2^53 have remainder 0.
fn number_parts(v: &Value) -> Option<(f64, i64)> {
    match v {
        Value::Float(f) => Some((canon_f64(*f), 0)),
        Value::Int(i) => {
            let f = *i as f64;
            // |remainder| <= 2^10, so it always fits in an i64
            let rem = (i128::from(*i) - f as i128) as i64;
            Some((f, rem))
        }
        _ => None,
    }
}

fn f64_key(f: f64) -> [u8; 8] {
    let bits = f.to_bits();
    let ordered = if bits >> 63 == 1 {
        !bits
    } else {
        bits | (1 << 63)
    };
    ordered.to_be_bytes()
}

fn f32_key(f: f32) -> [u8; 4] {
    let bits = f.to_bits();
    let ordered = if bits >> 31 == 1 {
        !bits
    } else {
        bits | (1 << 31)
    };
    ordered.to_be_bytes()
}

/// Writes bytes so the encoding is prefix-free and order-preserving: every `0x00` becomes
/// `0x00 0xFF`, and the value ends with `0x00 0x00`.
fn put_escaped(out: &mut Vec<u8>, bytes: &[u8]) {
    for &b in bytes {
        out.push(b);
        if b == 0 {
            out.push(0xFF);
        }
    }
    out.extend_from_slice(&[0, 0]);
}

impl Value {
    /// Human-facing name of the value's kind, as used by schema introspection:
    /// `Null`, `Boolean`, `Integer`, `Float`, `String`, `Bytes`, `List`, `Map`, `Vector`.
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Null => "Null",
            Value::Bool(_) => "Boolean",
            Value::Int(_) => "Integer",
            Value::Float(_) => "Float",
            Value::String(_) => "String",
            Value::Bytes(_) => "Bytes",
            Value::List(_) => "List",
            Value::Map(_) => "Map",
            Value::Vector(_) => "Vector",
        }
    }

    /// `true` for [`Value::Null`].
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    /// Nesting depth: 0 for scalars and vectors, `1 + deepest child` for lists and maps
    /// (an empty collection has depth 1).
    pub fn depth(&self) -> usize {
        match self {
            Value::List(items) => 1 + items.iter().map(Value::depth).max().unwrap_or(0),
            Value::Map(m) => 1 + m.values().map(Value::depth).max().unwrap_or(0),
            _ => 0,
        }
    }

    pub(crate) fn rank(&self) -> u8 {
        match self {
            Value::Null => TAG_NULL,
            Value::Bool(_) => TAG_BOOL,
            Value::Int(_) | Value::Float(_) => TAG_NUMBER,
            Value::String(_) => TAG_STRING,
            Value::Bytes(_) => TAG_BYTES,
            Value::List(_) => TAG_LIST,
            Value::Map(_) => TAG_MAP,
            Value::Vector(_) => TAG_VECTOR,
        }
    }

    /// Stable, order-preserving byte encoding for use as an index key.
    ///
    /// For all values `a`, `b`: `a.cmp(b) == a.to_key_bytes().cmp(&b.to_key_bytes())`, and
    /// no encoding is a proper prefix of another, so `index_prefix || key || node_id`
    /// entries never collide or interleave. The format is part of the on-disk layout.
    pub fn to_key_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(16);
        self.write_key(&mut out);
        out
    }

    /// Appends [`Value::to_key_bytes`] to `out`.
    pub fn write_key(&self, out: &mut Vec<u8>) {
        match self {
            Value::Null => out.push(TAG_NULL),
            Value::Bool(b) => out.extend_from_slice(&[TAG_BOOL, u8::from(*b)]),
            Value::Int(_) | Value::Float(_) => {
                let (f, rem) = number_parts(self).expect("number");
                out.push(TAG_NUMBER);
                out.extend_from_slice(&f64_key(f));
                out.extend_from_slice(&((rem as u64) ^ (1 << 63)).to_be_bytes());
            }
            Value::String(s) => {
                out.push(TAG_STRING);
                put_escaped(out, s.as_bytes());
            }
            Value::Bytes(b) => {
                out.push(TAG_BYTES);
                put_escaped(out, b);
            }
            Value::List(items) => {
                out.push(TAG_LIST);
                for item in items {
                    item.write_key(out);
                }
                out.push(0);
            }
            Value::Map(m) => {
                out.push(TAG_MAP);
                for (k, v) in m {
                    out.push(1);
                    put_escaped(out, k.as_bytes());
                    v.write_key(out);
                }
                out.push(0);
            }
            Value::Vector(xs) => {
                out.push(TAG_VECTOR);
                for x in xs {
                    out.push(1);
                    out.extend_from_slice(&f32_key(canon_f32(*x)));
                }
                out.push(0);
            }
        }
    }

    /// Converts to JSON. See the [module docs](self) for the exact (lossless) mapping.
    pub fn to_json(&self) -> serde_json::Value {
        use serde_json::Value as J;
        match self {
            Value::Null => J::Null,
            Value::Bool(b) => J::Bool(*b),
            Value::Int(i) => J::from(*i),
            Value::Float(f) => match serde_json::Number::from_f64(*f) {
                Some(n) => J::Number(n),
                None => tagged("float", J::String(nonfinite_name(*f).to_string())),
            },
            Value::String(s) => J::String(s.clone()),
            Value::Bytes(b) => tagged("bytes", J::String(hex_encode(b))),
            Value::List(items) => J::Array(items.iter().map(Value::to_json).collect()),
            Value::Map(m) => {
                let obj: serde_json::Map<String, J> =
                    m.iter().map(|(k, v)| (k.clone(), v.to_json())).collect();
                if m.contains_key(JSON_TAG) {
                    tagged("map", J::Object(obj))
                } else {
                    J::Object(obj)
                }
            }
            Value::Vector(xs) => tagged(
                "vector",
                J::Array(
                    xs.iter()
                        .map(|x| {
                            let f = f64::from(*x);
                            serde_json::Number::from_f64(f)
                                .map_or_else(|| J::String(nonfinite_name(f).to_string()), J::Number)
                        })
                        .collect(),
                ),
            ),
        }
    }

    /// Parses the JSON produced by [`Value::to_json`]. Any other JSON is accepted too:
    /// integers that fit `i64` become `Int`, other numbers `Float`, objects `Map`.
    pub fn from_json(json: &serde_json::Value) -> Result<Value> {
        use serde_json::Value as J;
        Ok(match json {
            J::Null => Value::Null,
            J::Bool(b) => Value::Bool(*b),
            J::Number(n) => match n.as_i64() {
                Some(i) => Value::Int(i),
                None => Value::Float(
                    n.as_f64()
                        .ok_or_else(|| Error::Json(format!("number {n} is not representable")))?,
                ),
            },
            J::String(s) => Value::String(s.clone()),
            J::Array(items) => Value::List(
                items
                    .iter()
                    .map(Value::from_json)
                    .collect::<Result<Vec<_>>>()?,
            ),
            J::Object(obj) => match obj.get(JSON_TAG) {
                None => plain_map(obj)?,
                Some(tag) => decode_tagged(tag, obj)?,
            },
        })
    }
}

fn tagged(tag: &str, payload: serde_json::Value) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    obj.insert(
        JSON_TAG.to_string(),
        serde_json::Value::String(tag.to_string()),
    );
    obj.insert("v".to_string(), payload);
    serde_json::Value::Object(obj)
}

fn plain_map(obj: &serde_json::Map<String, serde_json::Value>) -> Result<Value> {
    Ok(Value::Map(
        obj.iter()
            .map(|(k, v)| Ok((k.clone(), Value::from_json(v)?)))
            .collect::<Result<BTreeMap<_, _>>>()?,
    ))
}

fn nonfinite_name(f: f64) -> &'static str {
    if f.is_nan() {
        "NaN"
    } else if f > 0.0 {
        "Infinity"
    } else {
        "-Infinity"
    }
}

fn parse_nonfinite(s: &str) -> Option<f64> {
    match s {
        "NaN" => Some(f64::NAN),
        "Infinity" => Some(f64::INFINITY),
        "-Infinity" => Some(f64::NEG_INFINITY),
        _ => None,
    }
}

fn decode_tagged(
    tag: &serde_json::Value,
    obj: &serde_json::Map<String, serde_json::Value>,
) -> Result<Value> {
    use serde_json::Value as J;
    let bad = |msg: &str| Error::Json(format!("{msg}: {}", J::Object(obj.clone())));
    let tag = tag.as_str().ok_or_else(|| bad("tag must be a string"))?;
    let payload = match (obj.len(), obj.get("v")) {
        (2, Some(p)) => p,
        _ => {
            return Err(bad(
                "tagged value must have exactly the keys \"$seal\" and \"v\"",
            ))
        }
    };
    match tag {
        "bytes" => {
            let s = payload
                .as_str()
                .ok_or_else(|| bad("bytes payload must be a string"))?;
            hex_decode(s)
                .map(Value::Bytes)
                .ok_or_else(|| bad("invalid hex"))
        }
        "float" => payload
            .as_str()
            .and_then(parse_nonfinite)
            .map(Value::Float)
            .ok_or_else(|| bad("float payload must be NaN, Infinity or -Infinity")),
        "vector" => {
            let items = payload
                .as_array()
                .ok_or_else(|| bad("vector payload must be an array"))?;
            items
                .iter()
                .map(|x| match x {
                    J::Number(n) => n.as_f64().map(|f| f as f32),
                    J::String(s) => parse_nonfinite(s).map(|f| f as f32),
                    _ => None,
                })
                .collect::<Option<Vec<f32>>>()
                .map(Value::Vector)
                .ok_or_else(|| bad("vector elements must be numbers"))
        }
        "map" => match payload {
            J::Object(inner) => plain_map(inner),
            _ => Err(bad("map payload must be an object")),
        },
        _ => Err(bad("unknown tag")),
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(DIGITS[(b >> 4) as usize] as char);
        s.push(DIGITS[(b & 15) as usize] as char);
    }
    s
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    let raw = s.as_bytes();
    if !raw.len().is_multiple_of(2) {
        return None;
    }
    let nibble = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    };
    raw.chunks(2)
        .map(|pair| Some(nibble(pair[0])? << 4 | nibble(pair[1])?))
        .collect()
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Value {}

impl PartialOrd for Value {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Value {
    fn cmp(&self, other: &Self) -> Ordering {
        let by_kind = self.rank().cmp(&other.rank());
        if by_kind != Ordering::Equal {
            return by_kind;
        }
        match (self, other) {
            (Value::Null, Value::Null) => Ordering::Equal,
            (Value::Bool(a), Value::Bool(b)) => a.cmp(b),
            (Value::String(a), Value::String(b)) => a.cmp(b),
            (Value::Bytes(a), Value::Bytes(b)) => a.cmp(b),
            (Value::List(a), Value::List(b)) => a.cmp(b),
            (Value::Map(a), Value::Map(b)) => a.cmp(b),
            (Value::Vector(a), Value::Vector(b)) => a
                .iter()
                .map(|x| canon_f32(*x))
                .zip(b.iter().map(|x| canon_f32(*x)))
                .map(|(x, y)| x.total_cmp(&y))
                .find(|o| *o != Ordering::Equal)
                .unwrap_or_else(|| a.len().cmp(&b.len())),
            _ => {
                // both are numbers (same rank, not matched above)
                let (fa, ra) = number_parts(self).expect("number");
                let (fb, rb) = number_parts(other).expect("number");
                fa.total_cmp(&fb).then(ra.cmp(&rb))
            }
        }
    }
}

impl fmt::Display for Value {
    /// Renders the value as its JSON form.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_json())
    }
}

impl From<bool> for Value {
    fn from(v: bool) -> Self {
        Value::Bool(v)
    }
}

impl From<i64> for Value {
    fn from(v: i64) -> Self {
        Value::Int(v)
    }
}

impl From<i32> for Value {
    fn from(v: i32) -> Self {
        Value::Int(i64::from(v))
    }
}

impl From<f64> for Value {
    fn from(v: f64) -> Self {
        Value::Float(v)
    }
}

impl From<&str> for Value {
    fn from(v: &str) -> Self {
        Value::String(v.to_string())
    }
}

impl From<String> for Value {
    fn from(v: String) -> Self {
        Value::String(v)
    }
}

impl From<Vec<f32>> for Value {
    fn from(v: Vec<f32>) -> Self {
        Value::Vector(v)
    }
}

impl From<Vec<Value>> for Value {
    fn from(v: Vec<Value>) -> Self {
        Value::List(v)
    }
}

impl Serialize for Value {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        self.to_json().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Value {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let json = serde_json::Value::deserialize(deserializer)?;
        Value::from_json(&json).map_err(serde::de::Error::custom)
    }
}
