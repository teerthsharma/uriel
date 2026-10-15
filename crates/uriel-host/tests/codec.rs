mod common;

use common::{random_request, random_response, Rng};
use uriel_host::codec::{begin_frame, finish_frame, parse_frame, put_str, put_u32, put_u8};
use uriel_host::{
    encode_query, AccessMode, CodecError, Request, Response, Value, DEFAULT_MAX_FRAME, MSG_PING,
    MSG_QUERY, MSG_STATS,
};
use std::collections::BTreeMap;

const KNOWN_KINDS: [u8; 10] = [
    0x01, 0x02, 0x03, 0x04, 0x81, 0x82, 0x83, 0x84, 0xE0, 0xE1,
];

#[test]
fn two_thousand_random_requests_round_trip() {
    let mut rng = Rng::new(0x5EA1_0001);
    let mut buf = Vec::new();
    for _ in 0..2_000 {
        let request = random_request(&mut rng);
        request.encode(&mut buf, DEFAULT_MAX_FRAME).expect("encode");
        let back = Request::decode(&buf, DEFAULT_MAX_FRAME).expect("decode");
        assert_eq!(request, back);
        assert_eq!(format!("{request:?}"), format!("{back:?}"));
    }
}

#[test]
fn two_thousand_random_responses_round_trip() {
    let mut rng = Rng::new(0x5EA1_0002);
    let mut buf = Vec::new();
    for _ in 0..2_000 {
        let response = random_response(&mut rng);
        response
            .encode(&mut buf, DEFAULT_MAX_FRAME)
            .expect("encode");
        let back = Response::decode(&buf, DEFAULT_MAX_FRAME).expect("decode");
        assert_eq!(response, back);
        assert_eq!(format!("{response:?}"), format!("{back:?}"));
    }
}

#[test]
fn frame_layout_is_little_endian_length_then_type_then_payload() {
    let mut buf = Vec::new();
    Request::Ping { nonce: 0x0102_0304_0506_0708 }
        .encode(&mut buf, DEFAULT_MAX_FRAME)
        .expect("encode");
    assert_eq!(buf.len(), 4 + 1 + 8);
    assert_eq!(&buf[..4], &9u32.to_le_bytes());
    assert_eq!(buf[4], MSG_PING);
    assert_eq!(&buf[5..], &0x0102_0304_0506_0708u64.to_le_bytes());
    Request::Stats
        .encode(&mut buf, DEFAULT_MAX_FRAME)
        .expect("encode");
    assert_eq!(buf, [1, 0, 0, 0, MSG_STATS]);
}

#[test]
fn decoder_never_panics_on_twenty_thousand_deterministic_byte_strings() {
    let mut rng = Rng::new(0x5EA1_0003);
    for round in 0..20_000usize {
        let len = rng.index(200);
        let mut bytes = rng.bytes(len);
        if round % 2 == 0 && bytes.len() >= 5 {
            let body = (bytes.len() - 4) as u32;
            bytes[..4].copy_from_slice(&body.to_le_bytes());
            bytes[4] = KNOWN_KINDS[rng.index(KNOWN_KINDS.len())];
        }
        let _ = Request::decode(&bytes, DEFAULT_MAX_FRAME);
        let _ = Response::decode(&bytes, DEFAULT_MAX_FRAME);
    }
}

#[test]
fn every_truncation_of_a_valid_frame_is_an_error() {
    let mut rng = Rng::new(0x5EA1_0004);
    let mut buf = Vec::new();
    for round in 0..300 {
        if round % 2 == 0 {
            random_request(&mut rng)
                .encode(&mut buf, DEFAULT_MAX_FRAME)
                .expect("encode");
            for cut in 0..buf.len() {
                assert!(Request::decode(&buf[..cut], DEFAULT_MAX_FRAME).is_err());
            }
        } else {
            random_response(&mut rng)
                .encode(&mut buf, DEFAULT_MAX_FRAME)
                .expect("encode");
            for cut in 0..buf.len() {
                assert!(Response::decode(&buf[..cut], DEFAULT_MAX_FRAME).is_err());
            }
        }
    }
}

#[test]
fn truncated_payloads_with_a_consistent_prefix_never_panic() {
    let mut rng = Rng::new(0x5EA1_0005);
    let mut buf = Vec::new();
    for round in 0..300 {
        if round % 2 == 0 {
            random_request(&mut rng)
                .encode(&mut buf, DEFAULT_MAX_FRAME)
                .expect("encode");
        } else {
            random_response(&mut rng)
                .encode(&mut buf, DEFAULT_MAX_FRAME)
                .expect("encode");
        }
        for cut in 5..buf.len() {
            let mut piece = buf[..cut].to_vec();
            let body = (piece.len() - 4) as u32;
            piece[..4].copy_from_slice(&body.to_le_bytes());
            let _ = Request::decode(&piece, DEFAULT_MAX_FRAME);
            let _ = Response::decode(&piece, DEFAULT_MAX_FRAME);
        }
    }
}

#[test]
fn single_byte_corruption_never_panics() {
    let mut rng = Rng::new(0x5EA1_0006);
    let mut buf = Vec::new();
    for _ in 0..500 {
        random_response(&mut rng)
            .encode(&mut buf, DEFAULT_MAX_FRAME)
            .expect("encode");
        for _ in 0..8 {
            let mut copy = buf.clone();
            let at = rng.index(copy.len());
            copy[at] = rng.draw() as u8;
            let _ = Response::decode(&copy, DEFAULT_MAX_FRAME);
        }
    }
}

#[test]
fn encoder_rejects_a_frame_over_the_configured_maximum() {
    let mut buf = Vec::new();
    let big = "x".repeat(2_048);
    let result = encode_query(&mut buf, &big, &BTreeMap::new(), AccessMode::Read, 1_024);
    assert!(matches!(
        result,
        Err(CodecError::FrameTooLarge { max: 1_024, .. })
    ));
}

#[test]
fn encoder_rejects_a_two_mebibyte_frame_at_the_default_maximum() {
    let mut buf = Vec::new();
    let big = "x".repeat(2 << 20);
    let result = encode_query(
        &mut buf,
        &big,
        &BTreeMap::new(),
        AccessMode::Read,
        DEFAULT_MAX_FRAME,
    );
    assert!(matches!(result, Err(CodecError::FrameTooLarge { .. })));
}

#[test]
fn decoder_rejects_a_declared_length_over_the_maximum_before_reading_the_body() {
    let mut frame = Vec::new();
    frame.extend_from_slice(&(DEFAULT_MAX_FRAME as u32 + 1).to_le_bytes());
    frame.push(MSG_PING);
    let result = parse_frame(&frame, DEFAULT_MAX_FRAME);
    assert_eq!(
        result,
        Err(CodecError::FrameTooLarge {
            len: DEFAULT_MAX_FRAME + 1,
            max: DEFAULT_MAX_FRAME
        })
    );
    assert!(Request::decode(&frame, DEFAULT_MAX_FRAME).is_err());
    assert!(Response::decode(&frame, DEFAULT_MAX_FRAME).is_err());
}

#[test]
fn decoder_rejects_a_zero_length_prefix_and_a_mismatched_length() {
    assert_eq!(
        parse_frame(&[0, 0, 0, 0, 1], DEFAULT_MAX_FRAME),
        Err(CodecError::EmptyFrame)
    );
    assert_eq!(
        parse_frame(&[5, 0, 0, 0, MSG_PING], DEFAULT_MAX_FRAME),
        Err(CodecError::LengthMismatch {
            declared: 5,
            actual: 1
        })
    );
    assert!(matches!(
        parse_frame(&[1, 0], DEFAULT_MAX_FRAME),
        Err(CodecError::Truncated { .. })
    ));
}

#[test]
fn decoder_rejects_unknown_message_types_and_trailing_bytes() {
    let mut frame = Vec::new();
    begin_frame(&mut frame, 0x7F);
    finish_frame(&mut frame, DEFAULT_MAX_FRAME).expect("finish");
    assert_eq!(
        Request::decode(&frame, DEFAULT_MAX_FRAME),
        Err(CodecError::UnknownMessage(0x7F))
    );
    begin_frame(&mut frame, MSG_STATS);
    put_u8(&mut frame, 9);
    finish_frame(&mut frame, DEFAULT_MAX_FRAME).expect("finish");
    assert_eq!(
        Request::decode(&frame, DEFAULT_MAX_FRAME),
        Err(CodecError::Trailing(1))
    );
}

#[test]
fn a_count_larger_than_the_remaining_bytes_is_rejected_without_allocating() {
    let mut frame = Vec::new();
    begin_frame(&mut frame, MSG_QUERY);
    put_u32(&mut frame, u32::MAX);
    finish_frame(&mut frame, DEFAULT_MAX_FRAME).expect("finish");
    assert!(matches!(
        Request::decode(&frame, DEFAULT_MAX_FRAME),
        Err(CodecError::CountOverflow { .. })
    ));
}

#[test]
fn invalid_utf8_in_a_string_is_rejected() {
    let mut frame = Vec::new();
    begin_frame(&mut frame, MSG_QUERY);
    put_u32(&mut frame, 2);
    frame.extend_from_slice(&[0xFF, 0xFE]);
    finish_frame(&mut frame, DEFAULT_MAX_FRAME).expect("finish");
    assert_eq!(
        Request::decode(&frame, DEFAULT_MAX_FRAME),
        Err(CodecError::InvalidUtf8)
    );
}

#[test]
fn deeply_nested_values_are_rejected_without_overflowing_the_stack() {
    let mut frame = Vec::new();
    begin_frame(&mut frame, MSG_QUERY);
    put_str(&mut frame, "RETURN $k");
    put_u32(&mut frame, 1);
    put_str(&mut frame, "k");
    for _ in 0..20_000 {
        put_u8(&mut frame, 6);
        put_u32(&mut frame, 1);
    }
    put_u8(&mut frame, 0);
    put_u8(&mut frame, 0);
    finish_frame(&mut frame, 1 << 20).expect("finish");
    assert_eq!(
        Request::decode(&frame, 1 << 20),
        Err(CodecError::TooDeep(uriel_host::MAX_DECODE_DEPTH))
    );
}

#[test]
fn the_encoder_refuses_values_nested_deeper_than_the_decoder_accepts() {
    let mut value = Value::Null;
    for _ in 0..200 {
        value = Value::List(vec![value]);
    }
    let mut params = BTreeMap::new();
    params.insert("k".to_owned(), value);
    let mut buf = Vec::new();
    let result = encode_query(&mut buf, "RETURN $k", &params, AccessMode::Read, DEFAULT_MAX_FRAME);
    assert!(matches!(result, Err(CodecError::TooDeep(_))));
}

#[test]
fn query_parameters_survive_a_round_trip_with_exact_float_bits() {
    let mut params = BTreeMap::new();
    params.insert("nan".to_owned(), Value::Float(f64::NAN));
    params.insert("neg_zero".to_owned(), Value::Float(-0.0));
    params.insert("inf".to_owned(), Value::Float(f64::INFINITY));
    params.insert("min".to_owned(), Value::Int(i64::MIN));
    let mut buf = Vec::new();
    encode_query(&mut buf, "RETURN 1", &params, AccessMode::Write, DEFAULT_MAX_FRAME)
        .expect("encode");
    let Request::Query { params: back, .. } =
        Request::decode(&buf, DEFAULT_MAX_FRAME).expect("decode")
    else {
        panic!("not a query");
    };
    let bits = |v: &Value| match v {
        Value::Float(f) => f.to_bits(),
        other => panic!("not a float: {other:?}"),
    };
    assert_eq!(bits(&back["nan"]), f64::NAN.to_bits());
    assert_eq!(bits(&back["neg_zero"]), (-0.0f64).to_bits());
    assert_eq!(bits(&back["inf"]), f64::INFINITY.to_bits());
    assert_eq!(back["min"], Value::Int(i64::MIN));
}
