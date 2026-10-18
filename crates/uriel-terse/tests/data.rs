mod common;

use common::SplitMix;
use uriel_terse::data::{decode, encode};
use uriel_terse::{Error, Value};

fn v(text: &str) -> Value {
    Value::parse(text).unwrap_or_else(|e| panic!("bad fixture {text:?}: {e}"))
}

fn roundtrip(text: &str) -> String {
    let value = v(text);
    let enc = encode(&value).unwrap();
    let back = decode(&enc).unwrap_or_else(|e| panic!("decode failed for {enc:?}: {e}"));
    assert_eq!(back, value, "round trip changed value; encoded {enc:?}");
    enc
}

#[test]
fn json_text_round_trips_exactly() {
    for text in [
        "null",
        "true",
        "[1,2,{\"a\":[]}]",
        "{\"b\":1,\"a\":2}",
        "\"a\\u0000b\\n\\t\\\"\\\\\"",
        "[1e400,-0,0.1e-7,123456789012345678901234567890,1E+2,-0.0,0]",
        "\"\\ud83d\\ude00\"",
    ] {
        let value = v(text);
        assert_eq!(v(&value.to_json()), value);
    }
    assert_eq!(v("{\"b\":1,\"a\":2}").to_json(), "{\"b\":1,\"a\":2}");
    assert_eq!(v(" { \"a\" : [ 1 , 2 ] } ").to_json(), "{\"a\":[1,2]}");
}

#[test]
fn json_parser_rejects_bad_input() {
    for bad in [
        "", "{", "[1,]", "{\"a\":1,}", "01", "1.", ".5", "+1", "\"\\ud800\"", "\"\\udc00\"",
        "\"a\nb\"", "tru", "[1] x", "{'a':1}", "\"\\x\"", "-", "1e", "nul",
    ] {
        assert!(Value::parse(bad).is_err(), "accepted {bad:?}");
    }
}

#[test]
fn depth_is_bounded() {
    let deep = format!("{}{}", "[".repeat(100_000), "]".repeat(100_000));
    assert_eq!(Value::parse(&deep), Err(Error::TooDeep));
    let mut value = Value::Null;
    for _ in 0..1000 {
        value = Value::Array(vec![value]);
    }
    assert_eq!(encode(&value), Err(Error::TooDeep));
    let deep_enc: String = (0..1000)
        .map(|i| format!("{}k:\n", " ".repeat(i)))
        .collect::<String>()
        + &format!("{}k: 1", " ".repeat(1000));
    assert_eq!(decode(&deep_enc), Err(Error::TooDeep));
}

#[test]
fn invalid_number_literal_is_an_error() {
    assert_eq!(
        encode(&Value::Number("0x10".to_string())),
        Err(Error::InvalidNumber)
    );
}

#[test]
fn uniform_array_becomes_header_plus_rows() {
    let enc = roundtrip("{\"hits\":[{\"a\":1,\"b\":\"x\"},{\"a\":2,\"b\":\"y\"}],\"n\":2}");
    assert_eq!(enc, "hits[2]{a,b}:\n 1,x\n 2,y\nn: 2");
}

#[test]
fn scalar_arrays_are_inline() {
    assert_eq!(roundtrip("{\"k\":[1,\"two\",null,true]}"), "k[4]: 1,two,null,true");
    assert_eq!(roundtrip("{\"k\":[]}"), "k[0]:");
    assert_eq!(roundtrip("{\"k\":{}}"), "k: {}");
}

#[test]
fn nested_objects_and_lists() {
    let enc = roundtrip(
        "{\"alpha\":{\"beta\":{\"gamma\":\"one\",\"delta\":\"two\"},\"epsilon\":\"three\"},\"items\":[{\"identifier\":\"xray\",\"labels\":[\"a\",\"b\"]},{\"identifier\":\"yankee\",\"labels\":[\"c\"]},[\"p\",\"q\"],\"s\",{}]}",
    );
    assert!(enc.contains("\nalpha:\n beta:\n  gamma: one\n"), "{enc}");
    assert!(
        enc.contains("\n - $0: xray\n   labels[2]: a,b\n"),
        "{enc}"
    );
    assert!(enc.contains("\n - [2]: p,q\n"), "{enc}");
    assert!(enc.ends_with("\n - s\n - {}"), "{enc}");
}

#[test]
fn shared_path_prefix_is_factored() {
    let text = "{\"hits\":[{\"file\":\"crates/uriel-rag/src/bm25.rs\",\"line\":1},{\"file\":\"crates/uriel-rag/src/index.rs\",\"line\":2},{\"file\":\"crates/uriel-rag/src/tokenize.rs\",\"line\":3}]}";
    let enc = roundtrip(text);
    assert!(enc.contains("crates/uriel-rag/src/"), "{enc}");
    assert_eq!(enc.matches("crates/uriel-rag/src/").count(), 1, "{enc}");
}

#[test]
fn repeated_long_keys_use_a_dictionary() {
    let text = "{\"l\":[{\"averyveryLongKeyName\":1,\"x\":{\"averyveryLongKeyName\":2}},{\"averyveryLongKeyName\":3,\"y\":[{\"averyveryLongKeyName\":4}]},{\"averyveryLongKeyName\":5,\"z\":{}}]}";
    let enc = roundtrip(text);
    assert_eq!(enc.matches("averyveryLongKeyName").count(), 1, "{enc}");
    assert!(enc.len() < v(text).to_json().len());
}

#[test]
fn key_order_is_preserved() {
    let text = "{\"zeta\":1,\"alpha\":[{\"y\":1,\"b\":2},{\"y\":3,\"b\":4}],\"mid\":{\"q\":1,\"a\":2}}";
    let value = v(text);
    let back = decode(&encode(&value).unwrap()).unwrap();
    assert_eq!(back.to_json(), text);
}

#[test]
fn awkward_strings_survive() {
    let strings = [
        "", " ", " lead", "trail ", "tab\there", "a,b", "a:b", "k: v", "\"q\"", "true", "false",
        "null", "123", "-1", "1e5", "-", "$x", "$", "{}", "[0]:", "- x", "-", "!hi", "!", "[",
        "{", "a^b", "^", "x}", "\u{e9}t\u{e9}", "\u{1f600}", "\u{0}", "\u{2028}", "a\nb", "a\r\nb",
        "https://x.org/a_b?c=d,e", "C:\\a\\b", "#c", "@m", "~t", "=", "a=b", "0x10", "01", "1.",
        "NaN", "Infinity", "\\", "a\\", "\"", "''",
    ];
    for s in strings {
        let value = Value::Object(vec![
            ("k".to_string(), Value::String(s.to_string())),
            (s.to_string(), Value::Number("1".to_string())),
            (
                "arr".to_string(),
                Value::Array(vec![
                    Value::String(s.to_string()),
                    Value::String(s.to_string()),
                ]),
            ),
            (
                "rows".to_string(),
                Value::Array(vec![
                    Value::Object(vec![
                        ("c".to_string(), Value::String(s.to_string())),
                        (s.to_string(), Value::Null),
                    ]),
                    Value::Object(vec![
                        ("c".to_string(), Value::String("z".to_string())),
                        (s.to_string(), Value::Bool(false)),
                    ]),
                ]),
            ),
            (
                "list".to_string(),
                Value::Array(vec![
                    Value::String(s.to_string()),
                    Value::Array(vec![Value::String(s.to_string())]),
                    Value::Object(vec![(s.to_string(), Value::String(s.to_string()))]),
                ]),
            ),
        ]);
        let enc = encode(&value).unwrap();
        assert_eq!(decode(&enc).unwrap(), value, "string {s:?}\nencoded:\n{enc}");
        let root = Value::String(s.to_string());
        let enc = encode(&root).unwrap();
        assert_eq!(decode(&enc).unwrap(), root, "root string {s:?}");
    }
}

#[test]
fn edge_numbers_survive() {
    let enc = roundtrip(
        "{\"n\":[1e400,-0,0.1e-7,123456789012345678901234567890,1E+2,-0.0,0,1.5e300,5e-324],\"m\":1e400}",
    );
    assert!(enc.contains("1e400"));
}

#[test]
fn roots_of_every_shape() {
    for text in [
        "null", "true", "1", "\"s\"", "[]", "{}", "[1,2]", "[{\"a\":1},{\"a\":2}]", "[[1],[2]]",
        "{\"a\":null}", "[\"x\"]", "[{}]",
    ] {
        roundtrip(text);
    }
}

#[test]
fn never_larger_than_minified_json_plus_one() {
    let mut rng = SplitMix(31);
    for _ in 0..500 {
        let value = random_value(&mut rng, 4);
        let enc = encode(&value).unwrap();
        assert!(enc.len() <= value.to_json().len() + 1, "{enc}");
    }
}

#[test]
fn decode_rejects_or_accepts_garbage_without_panicking() {
    let mut rng = SplitMix(404);
    let alphabet: Vec<char> = "ab$[]{}:,^\"\\ -!\n01.e=".chars().collect();
    for _ in 0..20_000 {
        let len = rng.below(40);
        let s: String = (0..len).map(|_| *rng.pick(&alphabet)).collect();
        let _ = decode(&s);
    }
    for _ in 0..2000 {
        let len = rng.below(200);
        let bytes: Vec<u8> = (0..len).map(|_| rng.next_u64() as u8).collect();
        let _ = decode(&String::from_utf8_lossy(&bytes));
    }
}

#[test]
fn mutated_valid_encodings_never_panic() {
    let mut rng = SplitMix(808);
    for _ in 0..1500 {
        let value = random_value(&mut rng, 3);
        let mut chars: Vec<char> = encode(&value).unwrap().chars().collect();
        if chars.is_empty() {
            continue;
        }
        for _ in 0..1 + rng.below(3) {
            let at = rng.below(chars.len());
            match rng.below(3) {
                0 => {
                    chars.remove(at);
                }
                1 => chars.insert(at, *rng.pick(&[',', ':', ' ', '\n', '"', '[', '$'])),
                _ => chars[at] = *rng.pick(&['x', '1', '}', '^', '-']),
            }
            if chars.is_empty() {
                break;
            }
        }
        let s: String = chars.into_iter().collect();
        let _ = decode(&s);
    }
}

const KEYS: &[&str] = &[
    "a", "b", "id", "file", "start_line", "score", "via", "remembered", "k y", "", "$d", "x,y",
    "n:m", "q\"r", "\u{e9}", "averyveryLongKeyName", "path", "name", "[0]", "-", "!",
];

const STRS: &[&str] = &[
    "", "x", "true", "null", "42", "1e5", "-", "a,b", "k: v", "\"q\"", " sp", "sp ", "$ref",
    "{}", "[]", "- item", "!bang", "line\nbreak", "tab\t", "\u{e9}t\u{e9}", "\u{1f600}", "\u{0}",
    "crates/uriel-rag/src/bm25.rs", "crates/uriel-rag/src/index.rs", "crates/uriel-core/x.rs",
    "src/a", "src/b", "src/", "bm25", "vector", "https://x.org/a_b", "C:\\a\\b", "0x1f", "NaN",
    "#hash", "a^b", "x}",
];

const NUMS: &[&str] = &[
    "0", "1", "-1", "42", "3.5", "0.91", "-0", "-0.0", "1e5", "1E+2", "0.1e-7", "1e400",
    "123456789012345678901234567890", "5e-324", "10",
];

fn random_scalar(rng: &mut SplitMix) -> Value {
    match rng.below(5) {
        0 => Value::Null,
        1 => Value::Bool(rng.below(2) == 0),
        2 => Value::Number(rng.pick(NUMS).to_string()),
        _ => Value::String(rng.pick(STRS).to_string()),
    }
}

fn random_value(rng: &mut SplitMix, depth: usize) -> Value {
    if depth == 0 || rng.below(4) == 0 {
        return random_scalar(rng);
    }
    match rng.below(4) {
        0 => {
            let n = rng.below(5);
            Value::Array((0..n).map(|_| random_value(rng, depth - 1)).collect())
        }
        1 => {
            let n = 1 + rng.below(5);
            let cols: Vec<&str> = (0..1 + rng.below(4)).map(|_| *rng.pick(KEYS)).collect();
            let strings = rng.below(2) == 0;
            Value::Array(
                (0..n)
                    .map(|_| {
                        Value::Object(
                            cols.iter()
                                .map(|c| {
                                    let cell = if strings {
                                        Value::String(rng.pick(STRS).to_string())
                                    } else {
                                        random_scalar(rng)
                                    };
                                    (c.to_string(), cell)
                                })
                                .collect(),
                        )
                    })
                    .collect(),
            )
        }
        _ => {
            let n = rng.below(5);
            let mut seen: Vec<&str> = Vec::new();
            let mut fields = Vec::new();
            for _ in 0..n {
                let key = *rng.pick(KEYS);
                if seen.contains(&key) {
                    continue;
                }
                seen.push(key);
                fields.push((key.to_string(), random_value(rng, depth - 1)));
            }
            Value::Object(fields)
        }
    }
}

#[test]
fn random_values_round_trip() {
    let mut rng = SplitMix(1);
    for i in 0..6000 {
        let value = random_value(&mut rng, 1 + i % 5);
        let enc = encode(&value).unwrap();
        let back = decode(&enc)
            .unwrap_or_else(|e| panic!("decode failed: {e}\nvalue: {}\nenc:\n{enc}", value.to_json()));
        assert_eq!(back, value, "value: {}\nenc:\n{enc}", value.to_json());
        assert_eq!(Value::parse(&value.to_json()).unwrap(), value);
    }
}
