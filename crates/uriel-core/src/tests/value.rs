//! Value model: total order, key encoding, JSON round trip.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use serde_json::json;

use crate::Value;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

const TWO_53: i64 = 1 << 53;

fn s(x: &str) -> Value {
    Value::String(x.to_string())
}

fn map(entries: &[(&str, Value)]) -> Value {
    Value::Map(
        entries
            .iter()
            .map(|(k, v)| ((*k).to_string(), v.clone()))
            .collect::<BTreeMap<_, _>>(),
    )
}

fn random_string(rng: &mut Rng) -> String {
    let alphabet = ['a', 'b', 'z', '\0', '\u{1}', 'é', '\u{ff}', '€', '😀'];
    let n = rng.below(5);
    (0..n)
        .map(|_| alphabet[rng.below(alphabet.len() as u64) as usize])
        .collect()
}

fn random_float(rng: &mut Rng) -> f64 {
    match rng.below(6) {
        0 => f64::from_bits(rng.next()), // any bit pattern, including NaN and subnormals
        1 => (rng.below(2001) as f64 - 1000.0) / 4.0,
        2 => (rng.next() as i64) as f64,
        3 => [0.0, -0.0, f64::INFINITY, f64::NEG_INFINITY, f64::NAN][rng.below(5) as usize],
        4 => ((1_i64 << 53) + rng.below(5) as i64 - 2) as f64,
        _ => rng.below(7) as f64 - 3.0,
    }
}

fn random_int(rng: &mut Rng) -> i64 {
    match rng.below(5) {
        0 => rng.next() as i64,
        1 => rng.below(9) as i64 - 4,
        2 => TWO_53 + rng.below(7) as i64 - 3,
        3 => -TWO_53 + rng.below(7) as i64 - 3,
        _ => [i64::MIN, i64::MAX, i64::MIN + 1, i64::MAX - 1][rng.below(4) as usize],
    }
}

fn random_value(rng: &mut Rng, depth: u32) -> Value {
    let kinds = if depth == 0 { 7 } else { 9 };
    match rng.below(kinds) {
        0 => Value::Null,
        1 => Value::Bool(rng.below(2) == 1),
        2 => Value::Int(random_int(rng)),
        3 => Value::Float(random_float(rng)),
        4 => Value::String(random_string(rng)),
        5 => Value::Bytes(
            (0..rng.below(4))
                .map(|_| [0u8, 1, 255][rng.below(3) as usize])
                .collect(),
        ),
        6 => Value::Vector(
            (0..rng.below(4))
                .map(|_| random_float(rng) as f32)
                .collect(),
        ),
        7 => Value::List(
            (0..rng.below(4))
                .map(|_| random_value(rng, depth - 1))
                .collect(),
        ),
        _ => Value::Map(
            (0..rng.below(4))
                .map(|_| (random_string(rng), random_value(rng, depth - 1)))
                .collect(),
        ),
    }
}

fn curated() -> Vec<Value> {
    let mut v = vec![
        Value::Null,
        Value::Bool(false),
        Value::Bool(true),
        Value::Int(0),
        Value::Int(1),
        Value::Int(-1),
        Value::Int(i64::MIN),
        Value::Int(i64::MAX),
        Value::Int(i64::MAX - 1),
        Value::Int(TWO_53),
        Value::Int(TWO_53 + 1),
        Value::Int(TWO_53 - 1),
        Value::Int(-TWO_53 - 1),
        Value::Float(0.0),
        Value::Float(-0.0),
        Value::Float(1.0),
        Value::Float(-1.0),
        Value::Float(0.5),
        Value::Float(1.5),
        Value::Float(1e300),
        Value::Float(-1e300),
        Value::Float(f64::INFINITY),
        Value::Float(f64::NEG_INFINITY),
        Value::Float(f64::NAN),
        Value::Float(TWO_53 as f64),
        Value::Float(9_223_372_036_854_775_808.0),
        Value::Float(-9_223_372_036_854_775_808.0),
        Value::Float(5e-324),
        Value::Float(f64::MIN_POSITIVE),
        s(""),
        s("a"),
        s("a\0"),
        s("a\0b"),
        s("ab"),
        s("b"),
        s("é"),
        s("\0"),
        Value::Bytes(vec![]),
        Value::Bytes(vec![0]),
        Value::Bytes(vec![0, 0]),
        Value::Bytes(vec![0, 1]),
        Value::Bytes(vec![1]),
        Value::Bytes(vec![255]),
        Value::List(vec![]),
        Value::List(vec![Value::Null]),
        Value::List(vec![Value::Int(1)]),
        Value::List(vec![Value::Int(1), Value::Int(2)]),
        Value::List(vec![Value::Float(1.0)]),
        Value::List(vec![Value::Int(2)]),
        Value::List(vec![Value::List(vec![])]),
        map(&[]),
        map(&[("a", Value::Int(1))]),
        map(&[("a", Value::Int(1)), ("b", Value::Int(2))]),
        map(&[("b", Value::Int(0))]),
        map(&[("a", Value::Float(1.0))]),
        map(&[("a\0", Value::Int(1))]),
        Value::Vector(vec![]),
        Value::Vector(vec![0.0]),
        Value::Vector(vec![-0.0]),
        Value::Vector(vec![1.0]),
        Value::Vector(vec![1.0, 2.0]),
        Value::Vector(vec![f32::NAN]),
        Value::Vector(vec![f32::INFINITY]),
    ];
    v.sort_by(|a, b| a.type_name().cmp(b.type_name()));
    v
}

fn pool() -> Vec<Value> {
    let mut v = curated();
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for _ in 0..160 {
        v.push(random_value(&mut rng, 2));
    }
    v
}

#[test]
fn types_are_ordered_null_bool_number_string_bytes_list_map_vector() {
    let ladder = [
        Value::Null,
        Value::Bool(true),
        Value::Float(f64::NAN),
        s(""),
        Value::Bytes(vec![]),
        Value::List(vec![]),
        map(&[]),
        Value::Vector(vec![]),
    ];
    for w in ladder.windows(2) {
        assert!(w[0] < w[1], "{:?} should sort before {:?}", w[0], w[1]);
    }
    assert!(Value::Bool(false) < Value::Bool(true));
}

#[test]
fn numbers_compare_by_exact_mathematical_value() {
    assert_eq!(Value::Int(1), Value::Float(1.0));
    assert_eq!(Value::Int(0), Value::Float(-0.0));
    assert_eq!(Value::Float(0.0), Value::Float(-0.0));
    assert_eq!(
        Value::Int(i64::MIN),
        Value::Float(-9_223_372_036_854_775_808.0)
    );
    assert!(Value::Int(2) > Value::Float(1.5));
    assert!(Value::Int(1) < Value::Float(1.5));
    // 2^53 + 1 is not representable as f64: comparison must not round it away
    assert!(Value::Int(TWO_53 + 1) > Value::Float(TWO_53 as f64));
    assert!(Value::Int(TWO_53 + 1) > Value::Int(TWO_53));
    assert_eq!(Value::Int(TWO_53), Value::Float(TWO_53 as f64));
    // i64::MAX is strictly below 2^63 even though it rounds to it as f64
    assert!(Value::Int(i64::MAX) < Value::Float(9_223_372_036_854_775_808.0));
    assert!(Value::Int(i64::MAX) > Value::Float(9_223_372_036_854_774_784.0));
    // NaN is a member of the order: above every other number, equal to itself
    assert!(Value::Float(f64::NAN) > Value::Float(f64::INFINITY));
    assert_eq!(Value::Float(f64::NAN), Value::Float(f64::NAN));
    assert!(Value::Float(f64::NEG_INFINITY) < Value::Int(i64::MIN));
}

#[test]
fn collections_compare_lexicographically() {
    assert!(Value::List(vec![]) < Value::List(vec![Value::Null]));
    assert!(Value::List(vec![Value::Int(1)]) < Value::List(vec![Value::Int(1), Value::Int(0)]));
    assert!(Value::List(vec![Value::Int(1), Value::Int(9)]) < Value::List(vec![Value::Int(2)]));
    assert_eq!(
        Value::List(vec![Value::Int(1)]),
        Value::List(vec![Value::Float(1.0)])
    );
    assert!(map(&[("a", Value::Int(1))]) < map(&[("a", Value::Int(2))]));
    assert!(map(&[("a", Value::Int(1))]) < map(&[("b", Value::Int(0))]));
    assert!(map(&[("a", Value::Int(1))]) < map(&[("a", Value::Int(1)), ("b", Value::Int(0))]));
    assert!(Value::Vector(vec![1.0]) < Value::Vector(vec![1.0, 0.0]));
    assert_eq!(Value::Vector(vec![0.0]), Value::Vector(vec![-0.0]));
}

#[test]
fn order_is_a_total_order() {
    let pool = pool();
    for a in &pool {
        assert_eq!(a.cmp(a), Ordering::Equal, "reflexivity for {a:?}");
        for b in &pool {
            assert_eq!(a.cmp(b), b.cmp(a).reverse(), "antisymmetry {a:?} vs {b:?}");
        }
    }
    // transitivity on every triple of the first 90 values (729k triples)
    for a in pool.iter().take(90) {
        for b in pool.iter().take(90) {
            if a > b {
                continue;
            }
            for c in pool.iter().take(90) {
                if b <= c {
                    assert!(a <= c, "transitivity: {a:?} <= {b:?} <= {c:?}");
                }
            }
        }
    }
}

#[test]
fn key_encoding_preserves_order_and_equality() {
    let pool = pool();
    let keys: Vec<Vec<u8>> = pool.iter().map(Value::to_key_bytes).collect();
    for (i, a) in pool.iter().enumerate() {
        for (j, b) in pool.iter().enumerate() {
            assert_eq!(
                a.cmp(b),
                keys[i].cmp(&keys[j]),
                "order mismatch for {a:?} vs {b:?}: {:?} vs {:?}",
                keys[i],
                keys[j]
            );
        }
    }
}

#[test]
fn key_encoding_is_prefix_free() {
    // Needed so `prefix || key || node_id` index entries never collide or interleave.
    let pool = pool();
    let keys: Vec<Vec<u8>> = pool.iter().map(Value::to_key_bytes).collect();
    for (i, a) in keys.iter().enumerate() {
        for (j, b) in keys.iter().enumerate() {
            if a != b {
                assert!(
                    !b.starts_with(a),
                    "{:?} key is a prefix of {:?} key",
                    pool[i],
                    pool[j]
                );
            }
        }
    }
}

#[test]
fn integer_and_float_keys_sort_numerically_across_a_dense_sweep() {
    let mut vals: Vec<Value> = Vec::new();
    for i in -300..300_i64 {
        vals.push(Value::Int(i));
        vals.push(Value::Float(i as f64 / 2.0));
    }
    for base in [i64::MIN, -TWO_53, TWO_53, i64::MAX - 600] {
        for d in 0..600_i64 {
            vals.push(Value::Int(base.saturating_add(d)));
        }
    }
    vals.sort();
    for w in vals.windows(2) {
        assert!(
            w[0].to_key_bytes() <= w[1].to_key_bytes(),
            "{:?} / {:?}",
            w[0],
            w[1]
        );
        assert!(w[0] <= w[1]);
    }
}

#[test]
fn string_keys_sort_like_utf8_bytes_including_nul() {
    let mut rng = Rng(42);
    let mut strings: Vec<String> = (0..400).map(|_| random_string(&mut rng)).collect();
    strings.sort();
    for w in strings.windows(2) {
        let a = Value::String(w[0].clone()).to_key_bytes();
        let b = Value::String(w[1].clone()).to_key_bytes();
        assert!(a <= b, "{:?} / {:?}", w[0], w[1]);
        assert_eq!(a == b, w[0] == w[1]);
    }
}

#[test]
fn type_names_are_stable() {
    assert_eq!(Value::Null.type_name(), "Null");
    assert_eq!(Value::Bool(true).type_name(), "Boolean");
    assert_eq!(Value::Int(1).type_name(), "Integer");
    assert_eq!(Value::Float(1.0).type_name(), "Float");
    assert_eq!(s("x").type_name(), "String");
    assert_eq!(Value::Bytes(vec![]).type_name(), "Bytes");
    assert_eq!(Value::List(vec![]).type_name(), "List");
    assert_eq!(map(&[]).type_name(), "Map");
    assert_eq!(Value::Vector(vec![]).type_name(), "Vector");
}

#[test]
fn conversions_build_the_expected_variants() {
    assert_eq!(Value::from(true), Value::Bool(true));
    assert!(matches!(Value::from(7_i64), Value::Int(7)));
    assert!(matches!(Value::from(7_i32), Value::Int(7)));
    assert!(matches!(Value::from(1.5_f64), Value::Float(_)));
    assert!(matches!(Value::from("x"), Value::String(_)));
    assert!(matches!(Value::from(String::from("x")), Value::String(_)));
    assert!(matches!(Value::from(vec![1.0_f32]), Value::Vector(_)));
    assert!(matches!(Value::from(vec![Value::Null]), Value::List(_)));
}

#[test]
fn json_uses_natural_shapes_for_plain_values() {
    assert_eq!(Value::Null.to_json(), json!(null));
    assert_eq!(Value::Bool(true).to_json(), json!(true));
    assert_eq!(Value::Int(-5).to_json(), json!(-5));
    assert_eq!(s("hi").to_json(), json!("hi"));
    assert_eq!(
        Value::List(vec![Value::Int(1), s("a")]).to_json(),
        json!([1, "a"])
    );
    assert_eq!(
        map(&[("k", Value::Bool(false))]).to_json(),
        json!({"k": false})
    );
    // a float stays a float on the wire
    let f = Value::Float(1.0).to_json();
    assert!(f.is_f64(), "{f}");
    assert_eq!(serde_json::to_string(&Value::Float(1.0)).unwrap(), "1.0");
    assert_eq!(serde_json::to_string(&Value::Int(1)).unwrap(), "1");
}

#[test]
fn json_round_trips_every_value_exactly() {
    for v in pool() {
        let j = v.to_json();
        let back = Value::from_json(&j).unwrap_or_else(|e| panic!("{v:?} -> {j} failed: {e}"));
        assert_eq!(v.type_name(), back.type_name(), "{v:?} -> {j}");
        assert_eq!(v.to_key_bytes(), back.to_key_bytes(), "{v:?} -> {j}");
        // through text as well: this is what leaves the process
        let text = serde_json::to_string(&v).unwrap();
        let back2: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v.type_name(), back2.type_name(), "{v:?} -> {text}");
        assert_eq!(v, back2, "{v:?} -> {text}");
    }
}

#[test]
fn json_preserves_distinct_variants_that_json_cannot_express_natively() {
    let bytes = Value::Bytes(vec![0, 1, 254, 255]);
    assert_eq!(
        Value::from_json(&bytes.to_json()).unwrap().type_name(),
        "Bytes"
    );
    let vec = Value::Vector(vec![0.1, 2.5, f32::INFINITY]);
    match Value::from_json(&vec.to_json()).unwrap() {
        Value::Vector(x) => assert_eq!(x, vec![0.1_f32, 2.5, f32::INFINITY]),
        other => panic!("{other:?}"),
    }
    let nan = Value::from_json(&Value::Float(f64::NAN).to_json()).unwrap();
    assert!(matches!(nan, Value::Float(x) if x.is_nan()));
    let inf = Value::from_json(&Value::Float(f64::NEG_INFINITY).to_json()).unwrap();
    assert!(matches!(inf, Value::Float(x) if x == f64::NEG_INFINITY));
    // a user map that collides with the reserved tag key is escaped, not misread
    let tricky = map(&[("$seal", s("bytes")), ("v", s("00"))]);
    let back = Value::from_json(&tricky.to_json()).unwrap();
    assert_eq!(back.type_name(), "Map");
    assert_eq!(back, tricky);
}

#[test]
fn json_edge_cases_on_input() {
    // integers beyond i64 degrade to Float instead of failing
    let big = Value::from_json(&json!(18_446_744_073_709_551_615_u64)).unwrap();
    assert_eq!(big.type_name(), "Float");
    // unknown reserved tag is an error, not a silent Map
    let err = Value::from_json(&json!({"$seal": "nonsense", "v": 1})).unwrap_err();
    assert!(matches!(err, crate::error::Error::Json(_)), "{err:?}");
    // malformed payloads are errors
    assert!(Value::from_json(&json!({"$seal": "bytes", "v": "zz"})).is_err());
    assert!(Value::from_json(&json!({"$seal": "bytes", "v": "abc"})).is_err());
    assert!(Value::from_json(&json!({"$seal": "vector", "v": ["x"]})).is_err());
}

#[test]
fn depth_counts_nesting() {
    assert_eq!(Value::Int(1).depth(), 0);
    assert_eq!(Value::List(vec![]).depth(), 1);
    assert_eq!(
        Value::List(vec![Value::List(vec![Value::Int(1)])]).depth(),
        2
    );
    assert_eq!(map(&[("a", Value::List(vec![]))]).depth(), 2);
}
