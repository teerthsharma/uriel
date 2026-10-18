mod common;

use common::{random_request, random_response, Rng};
use uriel_host::codec::{begin_frame, finish_frame, parse_frame, put_str, put_u32, put_u8};
use uriel_host::{
    encode_index, encode_index_file, encode_query, AccessMode, CodecError, IndexReport, Request,
    Response, Value, DEFAULT_MAX_FRAME, MSG_INDEX, MSG_INDEX_FILE, MSG_INDEX_OK, MSG_PING,
    MSG_QUERY, MSG_STATS,
};
use std::collections::BTreeMap;

const KNOWN_KINDS: [u8; 13] = [
    0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x81, 0x82, 0x83, 0x84, 0x85, 0xE0, 0xE1,
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

#[test]
fn index_message_kinds_are_pinned() {
    assert_eq!(MSG_INDEX, 0x05);
    assert_eq!(MSG_INDEX_FILE, 0x06);
    assert_eq!(MSG_INDEX_OK, 0x85);
}

#[test]
fn index_frame_layout_is_prefix_kind_then_one_string() {
    let mut buf = Vec::new();
    encode_index(&mut buf, "ab", DEFAULT_MAX_FRAME).expect("encode");
    assert_eq!(buf, [1 + 4 + 2, 0, 0, 0, MSG_INDEX, 2, 0, 0, 0, b'a', b'b']);
    encode_index_file(&mut buf, "p", "xyz", DEFAULT_MAX_FRAME).expect("encode");
    assert_eq!(
        buf,
        [1 + 4 + 1 + 4 + 3, 0, 0, 0, MSG_INDEX_FILE, 1, 0, 0, 0, b'p', 3, 0, 0, 0, b'x', b'y', b'z']
    );
}

#[test]
fn indexed_response_carries_the_seven_report_counts_in_order() {
    let report = IndexReport {
        files_scanned: 1,
        files_indexed: 2,
        files_unchanged: 3,
        files_skipped: 4,
        files_removed: 5,
        chunks_added: 6,
        chunks_removed: 7,
    };
    let mut buf = Vec::new();
    Response::Indexed(report.clone())
        .encode(&mut buf, DEFAULT_MAX_FRAME)
        .expect("encode");
    assert_eq!(buf.len(), 4 + 1 + 7 * 8);
    assert_eq!(buf[4], MSG_INDEX_OK);
    for (i, expected) in (1u64..=7).enumerate() {
        let at = 5 + i * 8;
        assert_eq!(&buf[at..at + 8], &expected.to_le_bytes());
    }
    assert_eq!(
        Response::decode(&buf, DEFAULT_MAX_FRAME).expect("decode"),
        Response::Indexed(report)
    );
}

#[test]
fn an_index_file_over_the_frame_cap_is_refused_by_the_encoder_and_the_prefix_check() {
    let mut buf = Vec::new();
    let big = "x".repeat(2_048);
    assert!(matches!(
        encode_index_file(&mut buf, "p", &big, 1_024),
        Err(CodecError::FrameTooLarge { max: 1_024, .. })
    ));
    assert!(matches!(
        encode_index(&mut buf, &big, 1_024),
        Err(CodecError::FrameTooLarge { max: 1_024, .. })
    ));
    encode_index_file(&mut buf, "p", "x", DEFAULT_MAX_FRAME).expect("encode");
    let mut lying = buf.clone();
    lying[..4].copy_from_slice(&(1_025u32).to_le_bytes());
    assert_eq!(
        Request::decode(&lying, 1_024),
        Err(CodecError::FrameTooLarge { len: 1_025, max: 1_024 })
    );
}

#[test]
fn index_counts_larger_than_the_remaining_bytes_are_rejected_without_allocating() {
    for kind in [MSG_INDEX, MSG_INDEX_FILE] {
        let mut frame = Vec::new();
        begin_frame(&mut frame, kind);
        put_u32(&mut frame, u32::MAX);
        finish_frame(&mut frame, DEFAULT_MAX_FRAME).expect("finish");
        assert!(matches!(
            Request::decode(&frame, DEFAULT_MAX_FRAME),
            Err(CodecError::CountOverflow { .. })
        ));
    }
    let mut frame = Vec::new();
    begin_frame(&mut frame, MSG_INDEX_FILE);
    put_str(&mut frame, "p");
    put_u32(&mut frame, u32::MAX);
    finish_frame(&mut frame, DEFAULT_MAX_FRAME).expect("finish");
    assert!(matches!(
        Request::decode(&frame, DEFAULT_MAX_FRAME),
        Err(CodecError::CountOverflow { .. })
    ));
}

#[test]
fn index_frames_with_trailing_bytes_or_bad_utf8_are_rejected() {
    let mut frame = Vec::new();
    encode_index(&mut frame, "r", DEFAULT_MAX_FRAME).expect("encode");
    frame.push(0);
    let body = (frame.len() - 4) as u32;
    frame[..4].copy_from_slice(&body.to_le_bytes());
    assert_eq!(Request::decode(&frame, DEFAULT_MAX_FRAME), Err(CodecError::Trailing(1)));

    begin_frame(&mut frame, MSG_INDEX_FILE);
    put_str(&mut frame, "p");
    put_u32(&mut frame, 2);
    frame.extend_from_slice(&[0xFF, 0xFE]);
    finish_frame(&mut frame, DEFAULT_MAX_FRAME).expect("finish");
    assert_eq!(Request::decode(&frame, DEFAULT_MAX_FRAME), Err(CodecError::InvalidUtf8));
}

fn decoded_text_bytes(request: &Request) -> usize {
    match request {
        Request::Index { root } => root.len(),
        Request::IndexFile { path, content } => path.len() + content.len(),
        _ => 0,
    }
}

#[test]
fn index_decoders_survive_twenty_thousand_adversarial_inputs_with_bounded_output() {
    let mut rng = Rng::new(0x5EA1_0B07);
    let kinds = [MSG_INDEX, MSG_INDEX_FILE, MSG_INDEX_OK];
    let mut valid = Vec::new();
    for round in 0..20_000usize {
        let kind = kinds[rng.index(kinds.len())];
        let mut frame = Vec::new();
        match rng.index(5) {
            0 => {
                begin_frame(&mut frame, kind);
                let len = rng.index(120);
                frame.extend_from_slice(&rng.bytes(len));
                finish_frame(&mut frame, DEFAULT_MAX_FRAME).expect("finish");
            }
            1 => {
                begin_frame(&mut frame, kind);
                for _ in 0..rng.index(4) {
                    put_u32(&mut frame, [u32::MAX, u32::MAX - 1, 1 << 31, 0, 1, 7][rng.index(6)]);
                    let len = rng.index(10);
                    frame.extend_from_slice(&rng.bytes(len));
                }
                finish_frame(&mut frame, DEFAULT_MAX_FRAME).expect("finish");
            }
            2 => {
                let request = if rng.flag() {
                    Request::Index {
                        root: common::random_string(&mut rng, 30),
                    }
                } else {
                    Request::IndexFile {
                        path: common::random_string(&mut rng, 30),
                        content: common::random_string(&mut rng, 90),
                    }
                };
                request.encode(&mut frame, DEFAULT_MAX_FRAME).expect("encode");
                for _ in 0..1 + rng.index(4) {
                    let at = rng.index(frame.len());
                    let mask = if rng.flag() { 0x7F } else { 0xFF };
                    frame[at] = rng.draw() as u8 & mask;
                }
            }
            3 => {
                let request = Request::IndexFile {
                    path: common::random_string(&mut rng, 30),
                    content: common::random_string(&mut rng, 90),
                };
                request.encode(&mut frame, DEFAULT_MAX_FRAME).expect("encode");
                let cut = rng.index(frame.len());
                frame.truncate(cut);
                if frame.len() >= 5 {
                    let body = (frame.len() - 4) as u32;
                    frame[..4].copy_from_slice(&body.to_le_bytes());
                }
            }
            _ => {
                let len = rng.index(200);
                frame = rng.bytes(len);
                if round % 2 == 0 && frame.len() >= 5 {
                    let body = (frame.len() - 4) as u32;
                    frame[..4].copy_from_slice(&body.to_le_bytes());
                    frame[4] = kind;
                }
            }
        }
        if let Ok(request) = Request::decode(&frame, DEFAULT_MAX_FRAME) {
            assert!(decoded_text_bytes(&request) <= frame.len());
            valid.push(request);
        }
        let _ = Response::decode(&frame, DEFAULT_MAX_FRAME);
    }
    assert!(valid.len() > 200, "only {} adversarial inputs decoded", valid.len());
    assert!(valid.iter().any(|r| matches!(r, Request::Index { .. })));
    assert!(valid.iter().any(|r| matches!(r, Request::IndexFile { .. })));
}
