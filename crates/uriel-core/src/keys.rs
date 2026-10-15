//! Key formats for the byte-keyed tables.
//!
//! Every key starts with a fixed or NUL-terminated component so that a *prefix* selects
//! exactly one group and the group is a contiguous range in the B-tree:
//!
//! | table | key |
//! |---|---|
//! | label index | `label 00 node(8 BE)` |
//! | type index | `type 00 rel(8 BE)` |
//! | adjacency (out: src, in: dst) | `node(8 BE) type 00 rel(8 BE)` |
//! | property index | `label 00 key 00 value_key node(8 BE)` |
//! | counts | `'L' label` or `'R' type` |
//! | index catalog | `label 00 key` |
//! | constraint catalog | `label 00 key 00 kind` |
//!
//! Names never contain NUL ([`check_name`]), and [`Value::to_key_bytes`] is prefix-free,
//! which is what makes these prefixes unambiguous.

use std::ops::Bound;

use crate::catalog::ConstraintKind;
use crate::error::{Error, Result};
use crate::ids::{NodeId, RelId};
use crate::value::Value;

/// Rejects empty names and names containing NUL.
pub(crate) fn check_name(name: &str) -> Result<()> {
    if name.is_empty() || name.contains('\0') {
        Err(Error::InvalidName(name.to_string()))
    } else {
        Ok(())
    }
}

/// Smallest byte string greater than every string starting with `prefix`, or `None` if
/// `prefix` is all `0xFF` (no such bound exists).
pub(crate) fn prefix_end(prefix: &[u8]) -> Option<Vec<u8>> {
    let mut end = prefix.to_vec();
    while let Some(last) = end.pop() {
        if last != 0xFF {
            end.push(last + 1);
            return Some(end);
        }
    }
    None
}

/// Owned `[prefix, prefix_end)` bounds.
pub(crate) fn prefix_bounds(prefix: &[u8]) -> (Bound<Vec<u8>>, Bound<Vec<u8>>) {
    (
        Bound::Included(prefix.to_vec()),
        prefix_end(prefix).map_or(Bound::Unbounded, Bound::Excluded),
    )
}

/// Borrows owned bounds in the shape `redb` range methods accept.
pub(crate) fn as_slices(b: &(Bound<Vec<u8>>, Bound<Vec<u8>>)) -> (Bound<&[u8]>, Bound<&[u8]>) {
    fn one(b: &Bound<Vec<u8>>) -> Bound<&[u8]> {
        match b {
            Bound::Included(v) => Bound::Included(v.as_slice()),
            Bound::Excluded(v) => Bound::Excluded(v.as_slice()),
            Bound::Unbounded => Bound::Unbounded,
        }
    }
    (one(&b.0), one(&b.1))
}

fn with_u64(mut v: Vec<u8>, n: u64) -> Vec<u8> {
    v.extend_from_slice(&n.to_be_bytes());
    v
}

pub(crate) fn tail_u64(key: &[u8]) -> Result<u64> {
    let tail = key
        .len()
        .checked_sub(8)
        .map(|i| &key[i..])
        .ok_or_else(|| Error::Corrupt("key shorter than its id suffix".into()))?;
    Ok(u64::from_be_bytes(tail.try_into().expect("8 bytes")))
}

/// `label 00`
pub(crate) fn name_prefix(name: &str) -> Vec<u8> {
    let mut v = Vec::with_capacity(name.len() + 9);
    v.extend_from_slice(name.as_bytes());
    v.push(0);
    v
}

/// `name 00 id(8)`: used for the label and type indexes.
pub(crate) fn name_id_key(name: &str, id: u64) -> Vec<u8> {
    with_u64(name_prefix(name), id)
}

/// Splits `name 00 id(8)` into its parts.
pub(crate) fn parse_name_id_key(key: &[u8]) -> Result<(&str, u64)> {
    let id = tail_u64(key)?;
    let name = &key[..key.len() - 8];
    let name = name
        .strip_suffix(&[0])
        .and_then(|n| std::str::from_utf8(n).ok())
        .ok_or_else(|| Error::Corrupt("malformed name/id key".into()))?;
    Ok((name, id))
}

/// `node(8)` or `node(8) type 00`
pub(crate) fn adj_prefix(node: NodeId, rel_type: Option<&str>) -> Vec<u8> {
    let mut v = node.0.to_be_bytes().to_vec();
    if let Some(t) = rel_type {
        v.extend_from_slice(t.as_bytes());
        v.push(0);
    }
    v
}

/// `node(8) type 00 rel(8)`
pub(crate) fn adj_key(node: NodeId, rel_type: &str, rel: RelId) -> Vec<u8> {
    with_u64(adj_prefix(node, Some(rel_type)), rel.0)
}

/// Splits an adjacency key into `(node, type, rel)`.
pub(crate) fn parse_adj_key(key: &[u8]) -> Result<(NodeId, &str, RelId)> {
    if key.len() < 8 + 1 + 8 {
        return Err(Error::Corrupt("adjacency key too short".into()));
    }
    let node = u64::from_be_bytes(key[..8].try_into().expect("8 bytes"));
    let rel = tail_u64(key)?;
    let mid = &key[8..key.len() - 8];
    let ty = mid
        .strip_suffix(&[0])
        .and_then(|t| std::str::from_utf8(t).ok())
        .ok_or_else(|| Error::Corrupt("malformed adjacency key".into()))?;
    Ok((NodeId(node), ty, RelId(rel)))
}

/// `label 00 key 00`
pub(crate) fn prop_prefix(label: &str, key: &str) -> Vec<u8> {
    let mut v = name_prefix(label);
    v.extend_from_slice(key.as_bytes());
    v.push(0);
    v
}

/// Property index entry `label 00 key 00 value_key node(8)`.
pub(crate) fn prop_key(label: &str, key: &str, value: &Value, node: NodeId) -> Vec<u8> {
    let mut v = prop_prefix(label, key);
    value.write_key(&mut v);
    v.extend_from_slice(&node.0.to_be_bytes());
    v
}

/// Splits a property index entry into `(label, key, value_key_bytes, node)`.
pub(crate) fn parse_prop_key(key: &[u8]) -> Result<(&str, &str, &[u8], NodeId)> {
    let bad = || Error::Corrupt("malformed property index key".into());
    let node = tail_u64(key)?;
    let body = &key[..key.len() - 8];
    let l_end = body.iter().position(|b| *b == 0).ok_or_else(bad)?;
    let rest = &body[l_end + 1..];
    let k_end = rest.iter().position(|b| *b == 0).ok_or_else(bad)?;
    let label = std::str::from_utf8(&body[..l_end]).map_err(|_| bad())?;
    let k = std::str::from_utf8(&rest[..k_end]).map_err(|_| bad())?;
    Ok((label, k, &rest[k_end + 1..], NodeId(node)))
}

/// `label 00 key`: index catalog key.
pub(crate) fn index_key(label: &str, key: &str) -> Vec<u8> {
    let mut v = name_prefix(label);
    v.extend_from_slice(key.as_bytes());
    v
}

/// Splits `label 00 key`.
pub(crate) fn parse_index_key(key: &[u8]) -> Result<(&str, &str)> {
    let bad = || Error::Corrupt("malformed index catalog key".into());
    let sep = key.iter().position(|b| *b == 0).ok_or_else(bad)?;
    let label = std::str::from_utf8(&key[..sep]).map_err(|_| bad())?;
    let k = std::str::from_utf8(&key[sep + 1..]).map_err(|_| bad())?;
    Ok((label, k))
}

fn kind_byte(kind: ConstraintKind) -> u8 {
    match kind {
        ConstraintKind::Unique => 1,
        ConstraintKind::Exists => 2,
    }
}

/// `label 00 key 00 kind`: constraint catalog key.
pub(crate) fn constraint_key(kind: ConstraintKind, label: &str, key: &str) -> Vec<u8> {
    let mut v = index_key(label, key);
    v.push(0);
    v.push(kind_byte(kind));
    v
}

/// Splits a constraint catalog key.
pub(crate) fn parse_constraint_key(key: &[u8]) -> Result<(ConstraintKind, &str, &str)> {
    let bad = || Error::Corrupt("malformed constraint catalog key".into());
    let (body, kind) = match key {
        [body @ .., 0, k] => (body, *k),
        _ => return Err(bad()),
    };
    let kind = match kind {
        1 => ConstraintKind::Unique,
        2 => ConstraintKind::Exists,
        _ => return Err(bad()),
    };
    let (label, k) = parse_index_key(body)?;
    Ok((kind, label, k))
}

/// Key in the counts table for a label.
pub(crate) fn label_count_key(label: &str) -> Vec<u8> {
    let mut v = vec![b'L'];
    v.extend_from_slice(label.as_bytes());
    v
}

/// Key in the counts table for a relationship type.
pub(crate) fn type_count_key(rel_type: &str) -> Vec<u8> {
    let mut v = vec![b'R'];
    v.extend_from_slice(rel_type.as_bytes());
    v
}
