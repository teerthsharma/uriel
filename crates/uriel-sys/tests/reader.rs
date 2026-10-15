mod common;

use std::path::PathBuf;

use common::Rng;
use uriel_sys::{
    AsyncReader, ReadJob, ReaderBackend, ReaderMode, Region, Request, SysError, Width, MAX_BATCH,
};

const UNIT: usize = 4096;

struct TempFile {
    path: PathBuf,
    bytes: Vec<u8>,
}

impl TempFile {
    fn new(tag: &str, len: usize) -> TempFile {
        let mut rng = Rng(len as u64 ^ 0xF11E);
        let bytes: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
        let path = std::env::temp_dir().join(format!("uriel-sys-{}-{tag}.bin", std::process::id()));
        std::fs::write(&path, &bytes).unwrap();
        TempFile { path, bytes }
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn region(bytes: usize) -> Region {
    Region::allocate(Request {
        bytes,
        align: UNIT,
        prefer_huge: false,
    })
    .unwrap()
}

fn read_whole(mode: ReaderMode, file: &TempFile) -> (ReaderBackend, Vec<u8>) {
    let mut reader = AsyncReader::open_with(&file.path, mode).unwrap();
    assert_eq!(reader.file_len(), file.bytes.len() as u64);
    let slots = file.bytes.len().div_ceil(UNIT);
    let mut r = region(slots * UNIT);
    let cap = r.cap_write(0, slots * UNIT, Width::W8).unwrap();
    let n = reader.read_at(&mut r, &cap, 0).unwrap();
    assert_eq!(n, file.bytes.len());
    let got = r.view::<u8>(&cap.downgrade()).unwrap()[..n].to_vec();
    (reader.backend(), got)
}

#[test]
fn auto_mode_reads_exactly_the_file_bytes_and_reports_its_backend() {
    let file = TempFile::new("auto", 5 * UNIT + 123);
    let (backend, got) = read_whole(ReaderMode::Auto, &file);
    println!("auto backend: {backend:?}");
    assert_eq!(got, file.bytes);
    #[cfg(windows)]
    assert_eq!(backend, ReaderBackend::Unbuffered);
    #[cfg(not(windows))]
    assert_eq!(backend, ReaderBackend::Buffered);
}

#[test]
fn forced_buffered_fallback_is_taken_and_reads_identical_bytes() {
    let file = TempFile::new("forced", 3 * UNIT + 1);
    let (backend, got) = read_whole(ReaderMode::ForceBuffered, &file);
    assert_eq!(backend, ReaderBackend::Buffered);
    assert_eq!(got, file.bytes);
}

#[test]
fn a_failed_unbuffered_open_falls_back_to_buffered_and_records_why() {
    let file = TempFile::new("failopen", 4 * UNIT + 77);
    let (backend, got) = read_whole(ReaderMode::AutoWithFailedUnbufferedOpen, &file);
    assert_eq!(backend, ReaderBackend::Buffered);
    assert_eq!(got, file.bytes);
    let reader = AsyncReader::open_with(&file.path, ReaderMode::AutoWithFailedUnbufferedOpen).unwrap();
    assert!(matches!(reader.fallback_reason(), Some(SysError::Os { .. })), "{:?}", reader.fallback_reason());
    let forced = AsyncReader::open_with(&file.path, ReaderMode::ForceBuffered).unwrap();
    assert_eq!(forced.fallback_reason(), None);
    let auto = AsyncReader::open(&file.path).unwrap();
    #[cfg(windows)]
    assert_eq!(auto.fallback_reason(), None);
    #[cfg(not(windows))]
    assert_eq!(auto.fallback_reason(), Some(&SysError::Unsupported));
}

#[test]
fn a_failed_unbuffered_open_on_a_missing_file_still_reports_the_buffered_error() {
    let missing = std::env::temp_dir().join("uriel-sys-failopen-not-here.bin");
    let err = AsyncReader::open_with(&missing, ReaderMode::AutoWithFailedUnbufferedOpen).err();
    assert_eq!(err, Some(SysError::Io(std::io::ErrorKind::NotFound)));
}

#[test]
fn exact_sector_multiple_and_tiny_files_read_correctly() {
    for len in [1usize, 511, UNIT, 2 * UNIT, 7 * UNIT + 1] {
        let file = TempFile::new(&format!("len{len}"), len);
        for mode in [ReaderMode::Auto, ReaderMode::ForceBuffered] {
            let (_, got) = read_whole(mode, &file);
            assert_eq!(got, file.bytes, "len={len} mode={mode:?}");
        }
    }
}

#[test]
fn batch_reads_land_in_the_right_slots_and_match_both_backends() {
    let file = TempFile::new("batch", 40 * UNIT + 17);
    let mut rng = Rng(99);
    for mode in [ReaderMode::Auto, ReaderMode::ForceBuffered] {
        let mut reader = AsyncReader::open_with(&file.path, mode).unwrap();
        let mut r = region(16 * UNIT);
        for _ in 0..20 {
            let mut jobs = Vec::new();
            let mut want = Vec::new();
            for slot in 0..4usize {
                let units = 1 + rng.below(4) as usize;
                let first = rng.below(41) as usize * UNIT;
                let cap = r.cap_write(slot * 4 * UNIT, units * UNIT, Width::W8).unwrap();
                jobs.push(ReadJob { cap, file_offset: first as u64 });
                let end = (first + units * UNIT).min(file.bytes.len());
                want.push(file.bytes.get(first..end).unwrap_or(&[]).to_vec());
            }
            let done = reader.read_batch(&mut r, &jobs).unwrap();
            for (job, (n, expect)) in jobs.iter().zip(done.iter().zip(&want)) {
                assert_eq!(n, &expect.len(), "mode={mode:?}");
                let got = r.view::<u8>(&job.cap.downgrade()).unwrap();
                assert_eq!(&got[..*n], expect.as_slice());
            }
        }
    }
}

#[test]
fn reads_past_end_of_file_return_zero_bytes() {
    let file = TempFile::new("eof", 2 * UNIT);
    for mode in [ReaderMode::Auto, ReaderMode::ForceBuffered] {
        let mut reader = AsyncReader::open_with(&file.path, mode).unwrap();
        let mut r = region(UNIT);
        let cap = r.cap_write(0, UNIT, Width::W8).unwrap();
        assert_eq!(reader.read_at(&mut r, &cap, 2 * UNIT as u64).unwrap(), 0);
        assert_eq!(reader.read_at(&mut r, &cap, 100 * UNIT as u64).unwrap(), 0);
        assert_eq!(reader.read_at(&mut r, &cap, UNIT as u64).unwrap(), UNIT);
    }
}

#[test]
fn misaligned_requests_are_rejected_on_both_backends() {
    let file = TempFile::new("mis", 4 * UNIT);
    for mode in [ReaderMode::Auto, ReaderMode::ForceBuffered] {
        let mut reader = AsyncReader::open_with(&file.path, mode).unwrap();
        assert!(reader.alignment_unit() >= UNIT);
        let mut r = region(8 * UNIT);
        let good = r.cap_write(0, UNIT, Width::W8).unwrap();
        assert_eq!(reader.read_at(&mut r, &good, 1), Err(SysError::Misaligned));
        assert_eq!(reader.read_at(&mut r, &good, 512), Err(SysError::Misaligned));
        assert_eq!(reader.read_at(&mut r, &good, u64::MAX), Err(SysError::Misaligned));
        let short = r.cap_write(0, 100, Width::W8).unwrap();
        assert_eq!(reader.read_at(&mut r, &short, 0), Err(SysError::Misaligned));
        let odd_len = r.cap_write(0, UNIT + 512, Width::W8).unwrap();
        assert_eq!(reader.read_at(&mut r, &odd_len, 0), Err(SysError::Misaligned));
        let odd_start = r.cap_write(512, UNIT, Width::W8).unwrap();
        assert_eq!(reader.read_at(&mut r, &odd_start, 0), Err(SysError::Misaligned));
        assert_eq!(reader.read_at(&mut r, &good, 0), Ok(UNIT));
    }
}

#[test]
fn wrong_width_wrong_region_overlap_and_oversized_batches_are_rejected() {
    let file = TempFile::new("rej", 4 * UNIT);
    let mut reader = AsyncReader::open(&file.path).unwrap();
    let mut r = region(8 * UNIT);
    let other = region(8 * UNIT);
    let wide = r.cap_write(0, UNIT, Width::W32).unwrap();
    assert_eq!(reader.read_at(&mut r, &wide, 0), Err(SysError::WrongWidth));
    let foreign = other.cap_write(0, UNIT, Width::W8).unwrap();
    assert_eq!(reader.read_at(&mut r, &foreign, 0), Err(SysError::WrongRegion));
    let a = r.cap_write(0, 2 * UNIT, Width::W8).unwrap();
    let b = r.cap_write(UNIT, 2 * UNIT, Width::W8).unwrap();
    let jobs = [ReadJob { cap: a, file_offset: 0 }, ReadJob { cap: b, file_offset: 0 }];
    assert_eq!(reader.read_batch(&mut r, &jobs), Err(SysError::Overlap));
    let one = r.cap_write(0, UNIT, Width::W8).unwrap();
    let many = vec![ReadJob { cap: one, file_offset: 0 }; MAX_BATCH + 1];
    assert_eq!(reader.read_batch(&mut r, &many), Err(SysError::TooManyJobs));
    assert_eq!(reader.read_batch(&mut r, &[]), Ok(vec![]));
    assert_eq!(reader.read_at(&mut r, &one, 0), Ok(UNIT));
}

#[test]
fn missing_files_and_hostile_paths_return_errors() {
    let missing = std::env::temp_dir().join("uriel-sys-definitely-not-here.bin");
    assert!(matches!(AsyncReader::open(&missing), Err(SysError::Io(_))));
    assert!(AsyncReader::open("bad\0name").is_err());
    assert!(AsyncReader::open("").is_err());
    assert!(AsyncReader::open(std::env::temp_dir()).is_err() || cfg!(unix));
}
