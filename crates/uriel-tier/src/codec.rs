
use std::borrow::Cow;

use crate::error::{Result, TierError};

pub const TAG_RAW: u8 = 0xF0;
pub const TAG_LZ4: u8 = 0xF1;
pub const TAG_COLD: u8 = 0xF2;
pub const MAX_DECODED_LEN: usize = 256 * 1024 * 1024;

const COLD_LEVEL: i32 = 19;

pub trait Codec {
    fn tag(&self) -> u8;
    fn compress(&self, data: &[u8]) -> Vec<u8>;
    fn decompress(&self, data: &[u8]) -> Result<Vec<u8>>;
}

fn corrupt(msg: impl Into<String>) -> TierError {
    TierError::Corrupt(msg.into())
}

const LZ4_MAX_RATIO: usize = 256;
const ZSTD_MAX_RATIO: usize = 65_536;
const RATIO_SLACK: usize = 1024;

fn split_len(data: &[u8]) -> Result<(usize, &[u8])> {
    let (head, body) = data
        .split_first_chunk::<4>()
        .ok_or_else(|| corrupt("compressed payload shorter than its length header"))?;
    let len = u32::from_le_bytes(*head) as usize;
    if len > MAX_DECODED_LEN {
        return Err(corrupt(format!("declared length {len} exceeds the limit")));
    }
    Ok((len, body))
}

fn check_plausible(len: usize, body: usize, ratio: usize) -> Result<()> {
    if len > body.saturating_mul(ratio).saturating_add(RATIO_SLACK) {
        return Err(corrupt(format!(
            "declared length {len} is implausible for {body} compressed bytes"
        )));
    }
    Ok(())
}

fn with_len(len: usize, body: Vec<u8>) -> Vec<u8> {
    let len32 = u32::try_from(len).unwrap_or(u32::MAX);
    let mut out = Vec::with_capacity(body.len() + 4);
    out.extend_from_slice(&len32.to_le_bytes());
    out.extend_from_slice(&body);
    out
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Raw;

impl Codec for Raw {
    fn tag(&self) -> u8 {
        TAG_RAW
    }
    fn compress(&self, data: &[u8]) -> Vec<u8> {
        data.to_vec()
    }
    fn decompress(&self, data: &[u8]) -> Result<Vec<u8>> {
        Ok(data.to_vec())
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Lz4;

impl Codec for Lz4 {
    fn tag(&self) -> u8 {
        TAG_LZ4
    }
    fn compress(&self, data: &[u8]) -> Vec<u8> {
        with_len(data.len(), lz4_flex::block::compress(data))
    }
    fn decompress(&self, data: &[u8]) -> Result<Vec<u8>> {
        let (len, body) = split_len(data)?;
        check_plausible(len, body.len(), LZ4_MAX_RATIO)?;
        let out =
            lz4_flex::block::decompress(body, len).map_err(|e| corrupt(format!("lz4: {e}")))?;
        if out.len() != len {
            return Err(corrupt("lz4: length mismatch"));
        }
        Ok(out)
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Cold;

impl Codec for Cold {
    fn tag(&self) -> u8 {
        TAG_COLD
    }
    fn compress(&self, data: &[u8]) -> Vec<u8> {
        match zstd::bulk::compress(data, COLD_LEVEL) {
            Ok(body) => with_len(data.len(), body),
            Err(_) => with_len(
                data.len(),
                zstd::bulk::compress(data, 3).unwrap_or_default(),
            ),
        }
    }
    fn decompress(&self, data: &[u8]) -> Result<Vec<u8>> {
        let (len, body) = split_len(data)?;
        if len == 0 && body.is_empty() {
            return Ok(Vec::new());
        }
        check_plausible(len, body.len(), ZSTD_MAX_RATIO)?;
        let out = zstd::bulk::decompress(body, len).map_err(|e| corrupt(format!("zstd: {e}")))?;
        if out.len() != len {
            return Err(corrupt("zstd: length mismatch"));
        }
        Ok(out)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tier {
    Hot,
    Warm,
    Cold,
}

pub fn is_tagged(bytes: &[u8]) -> bool {
    matches!(bytes.first(), Some(&(TAG_RAW | TAG_LZ4 | TAG_COLD)))
}

pub fn encode_value(bytes: &[u8], tier: Tier, threshold: usize) -> Vec<u8> {
    let codec: &dyn Codec = match tier {
        Tier::Hot => &Raw,
        Tier::Warm => &Lz4,
        Tier::Cold => &Cold,
    };
    if codec.tag() != TAG_RAW && bytes.len() >= threshold {
        let packed = codec.compress(bytes);
        if packed.len() < bytes.len() {
            let mut out = Vec::with_capacity(packed.len() + 1);
            out.push(codec.tag());
            out.extend_from_slice(&packed);
            return out;
        }
    }
    let mut out = Vec::with_capacity(bytes.len() + 1);
    out.push(TAG_RAW);
    out.extend_from_slice(bytes);
    out
}

pub fn decode_value(bytes: &[u8]) -> Result<Vec<u8>> {
    let Some((&tag, rest)) = bytes.split_first() else {
        return Ok(Vec::new());
    };
    match tag {
        TAG_RAW => Ok(rest.to_vec()),
        TAG_LZ4 => Lz4.decompress(rest),
        TAG_COLD => Cold.decompress(rest),
        _ => Ok(bytes.to_vec()),
    }
}

pub fn decode_value_cow(bytes: &[u8]) -> Result<Cow<'_, [u8]>> {
    match bytes.split_first() {
        None => Ok(Cow::Borrowed(bytes)),
        Some((&TAG_RAW, rest)) => Ok(Cow::Borrowed(rest)),
        Some((&TAG_LZ4, rest)) => Lz4.decompress(rest).map(Cow::Owned),
        Some((&TAG_COLD, rest)) => Cold.decompress(rest).map(Cow::Owned),
        Some(_) => Ok(Cow::Borrowed(bytes)),
    }
}

pub fn logical_len(bytes: &[u8]) -> Result<usize> {
    match bytes.split_first() {
        None => Ok(0),
        Some((&TAG_RAW, rest)) => Ok(rest.len()),
        Some((&(TAG_LZ4 | TAG_COLD), rest)) => split_len(rest).map(|(len, _)| len),
        Some(_) => Ok(bytes.len()),
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
    }

    fn samples() -> Vec<Vec<u8>> {
        let mut rng = Rng(12345);
        let random: Vec<u8> = (0..4096).map(|_| rng.next() as u8).collect();
        let text = "the quick brown fox jumps over the lazy dog. "
            .repeat(200)
            .into_bytes();
        vec![
            vec![],
            vec![0],
            vec![0xF0],
            vec![0xF1, 2, 3],
            random,
            text,
            vec![7; 100_000],
        ]
    }

    #[test]
    fn codecs_round_trip() {
        let codecs: [&dyn Codec; 3] = [&Raw, &Lz4, &Cold];
        for c in codecs {
            for s in samples() {
                let packed = c.compress(&s);
                assert_eq!(
                    c.decompress(&packed).unwrap(),
                    s,
                    "codec tag {:#x}",
                    c.tag()
                );
            }
        }
    }

    #[test]
    fn tagged_values_round_trip_in_every_tier() {
        for tier in [Tier::Hot, Tier::Warm, Tier::Cold] {
            for threshold in [0, 16, 1 << 20] {
                for s in samples() {
                    let enc = encode_value(&s, tier, threshold);
                    assert!(is_tagged(&enc));
                    assert_eq!(decode_value(&enc).unwrap(), s, "{tier:?} thr {threshold}");
                }
            }
        }
    }

    #[test]
    fn small_values_stay_raw_and_big_compressible_ones_shrink() {
        let small = vec![b'a'; 31];
        let enc = encode_value(&small, Tier::Cold, 32);
        assert_eq!(enc[0], TAG_RAW);
        assert_eq!(enc.len(), small.len() + 1);

        let big = vec![b'a'; 4096];
        let enc = encode_value(&big, Tier::Cold, 32);
        assert_eq!(enc[0], TAG_COLD);
        assert!(enc.len() < 100);
        let enc = encode_value(&big, Tier::Warm, 32);
        assert_eq!(enc[0], TAG_LZ4);
    }

    #[test]
    fn incompressible_values_fall_back_to_raw() {
        let mut rng = Rng(99);
        let noise: Vec<u8> = (0..2048).map(|_| rng.next() as u8).collect();
        let enc = encode_value(&noise, Tier::Cold, 32);
        assert_eq!(enc[0], TAG_RAW);
        assert_eq!(enc.len(), noise.len() + 1);
    }

    #[test]
    fn legacy_untagged_values_decode_unchanged() {
        let legacy_record = [1u8, 0, 0, 0, 0, 2, 0, 0, 0, b'h', b'i'];
        assert!(!is_tagged(&legacy_record));
        assert_eq!(decode_value(&legacy_record).unwrap(), legacy_record);
        assert_eq!(decode_value(&[]).unwrap(), Vec::<u8>::new());
        for first in 0..=u8::MAX {
            if matches!(first, TAG_RAW | TAG_LZ4 | TAG_COLD) {
                continue;
            }
            let v = [first, 9, 9];
            assert_eq!(decode_value(&v).unwrap(), v, "first byte {first:#x}");
        }
    }

    #[test]
    fn cow_decode_matches_and_borrows_when_it_can() {
        for tier in [Tier::Hot, Tier::Warm, Tier::Cold] {
            for s in samples() {
                let enc = encode_value(&s, tier, 0);
                assert_eq!(decode_value_cow(&enc).unwrap().as_ref(), s.as_slice());
                assert_eq!(logical_len(&enc).unwrap(), s.len());
            }
        }
        let legacy = [1u8, 2, 3];
        assert!(matches!(
            decode_value_cow(&legacy).unwrap(),
            Cow::Borrowed(_)
        ));
        assert_eq!(logical_len(&legacy).unwrap(), 3);
        assert_eq!(logical_len(&[]).unwrap(), 0);
    }

    #[test]
    fn implausible_declared_length_is_rejected_before_allocating() {
        let len = (200u32 << 20).to_le_bytes();
        for tag in [TAG_LZ4, TAG_COLD] {
            let v = [tag, len[0], len[1], len[2], len[3], 0, 0];
            let err = decode_value(&v).unwrap_err().to_string();
            assert!(err.contains("implausible"), "tag {tag:#x}: {err}");
            assert!(logical_len(&v).is_ok(), "logical_len reads the header only");
        }
    }

    #[test]
    fn plausibility_bound_never_rejects_real_output() {
        for n in [0usize, 1, 2, 3, 7, 64, 4096, 1 << 20, 8 << 20] {
            let zeros = vec![0u8; n];
            for tier in [Tier::Warm, Tier::Cold] {
                let enc = encode_value(&zeros, tier, 0);
                assert_eq!(decode_value(&enc).unwrap(), zeros, "{tier:?} n={n}");
            }
        }
    }

    fn fuzz_input(rng: &mut Rng) -> Vec<u8> {
        let len = (rng.next() % 96) as usize;
        let mut v: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
        if let Some(first) = v.first_mut() {
            *first = match rng.next() % 4 {
                0 => TAG_RAW,
                1 => TAG_LZ4,
                2 => TAG_COLD,
                _ => *first,
            };
        }
        v
    }

    #[test]
    fn decoders_survive_20000_random_byte_strings() {
        let mut rng = Rng(0xdead_beef_cafe_f00d);
        let (mut ok, mut err) = (0u32, 0u32);
        for _ in 0..20_000 {
            let v = fuzz_input(&mut rng);
            match decode_value(&v) {
                Ok(out) => {
                    ok += 1;
                    assert_eq!(decode_value_cow(&v).unwrap().as_ref(), out.as_slice());
                }
                Err(_) => {
                    err += 1;
                    assert!(decode_value_cow(&v).is_err());
                }
            }
            let _ = logical_len(&v);
        }
        assert!(ok > 0 && err > 0, "ok {ok} err {err}");
    }

    #[test]
    fn every_truncation_and_bit_flip_of_a_valid_value_is_handled() {
        let mut rng = Rng(77);
        let text = "node 17: the quick brown fox jumps over the lazy dog; ".repeat(40);
        for tier in [Tier::Hot, Tier::Warm, Tier::Cold] {
            let enc = encode_value(text.as_bytes(), tier, 0);
            for cut in 0..enc.len() {
                if let Ok(out) = decode_value(&enc[..cut]) {
                    assert!(tier == Tier::Hot || out != text.as_bytes(), "cut {cut}");
                }
            }
            for _ in 0..2_000 {
                let mut m = enc.clone();
                let i = (rng.next() as usize) % m.len();
                m[i] ^= 1 << (rng.next() % 8);
                let _ = decode_value(&m);
                let _ = decode_value_cow(&m);
            }
        }
    }
    #[test]
    fn corrupt_tagged_values_error_instead_of_panicking() {
        assert!(decode_value(&[TAG_LZ4]).is_err());
        assert!(decode_value(&[TAG_COLD, 1, 2]).is_err());
        let mut enc = encode_value(&vec![b'z'; 5000], Tier::Cold, 0);
        enc.truncate(enc.len() - 3);
        assert!(decode_value(&enc).is_err());
        assert!(decode_value(&[TAG_LZ4, 0xff, 0xff, 0xff, 0xff, 0]).is_err());
        let mut enc = encode_value(&vec![b'z'; 5000], Tier::Warm, 0);
        enc[1] ^= 0x55;
        assert!(decode_value(&enc).is_err());
    }
}
