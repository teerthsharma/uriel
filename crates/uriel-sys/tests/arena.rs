mod common;

use common::Rng;
use uriel_sys::{Arena, SysError, Width, SLAB_ALIGN};

#[test]
fn cap_is_rounded_down_to_slab_multiple_and_tiny_caps_are_rejected() {
    assert_eq!(Arena::with_cap(0).err(), Some(SysError::CapTooSmall));
    assert_eq!(Arena::with_cap(63).err(), Some(SysError::CapTooSmall));
    let a = Arena::with_cap(1000).unwrap();
    assert_eq!(a.cap(), 960);
    assert_eq!(a.region().len(), 960);
    assert_eq!(a.used(), 0);
    assert_eq!(a.remaining(), 960);
}

#[test]
fn allocation_past_the_cap_errors_and_never_grows() {
    let mut a = Arena::with_cap(1000).unwrap();
    for i in 0..15 {
        let s = a.alloc(64, Width::W8).unwrap();
        assert_eq!(s.offset(), i * 64);
    }
    assert_eq!(a.used(), 960);
    for bytes in [1usize, 64, 65, 4096, usize::MAX, usize::MAX - 1, usize::MAX - 62] {
        let err = a.alloc(bytes, Width::W8).err();
        assert!(matches!(err, Some(SysError::CapExceeded { remaining: 0, .. })), "bytes={bytes} err={err:?}");
    }
    assert_eq!(a.used(), 960);
    assert_eq!(a.region().len(), 960);
    assert_eq!(a.alloc(0, Width::W8).err(), Some(SysError::ZeroSize));
}

#[test]
fn an_exact_fit_succeeds_and_one_byte_more_fails() {
    let mut a = Arena::with_cap(640).unwrap();
    a.alloc(100, Width::W8).unwrap();
    assert_eq!(a.used(), 128);
    assert!(a.alloc(512, Width::W8).is_ok());
    assert_eq!(a.remaining(), 0);
    let mut b = Arena::with_cap(640).unwrap();
    b.alloc(100, Width::W8).unwrap();
    assert_eq!(
        b.alloc(513, Width::W8).err(),
        Some(SysError::CapExceeded { requested: 576, remaining: 512 })
    );
    assert_eq!(b.used(), 128);
}

#[test]
fn misaligned_slab_length_for_the_width_is_rejected_without_consuming_space() {
    let mut a = Arena::with_cap(4096).unwrap();
    assert_eq!(a.alloc(10, Width::W32).err(), Some(SysError::Misaligned));
    assert_eq!(a.alloc(12, Width::W64).err(), Some(SysError::Misaligned));
    assert_eq!(a.used(), 0);
    assert!(a.alloc(12, Width::W32).is_ok());
}

#[test]
fn hundred_thousand_random_allocations_never_exceed_the_cap() {
    let cap = 1 << 20;
    let mut a = Arena::with_cap(cap).unwrap();
    let mut rng = Rng(0xA4E4A);
    let (mut ok, mut over, mut zero, mut resets) = (0u32, 0u32, 0u32, 0u32);
    let widths = [Width::W8, Width::W16, Width::W32, Width::W64];
    for i in 0..100_000u32 {
        if i % 997 == 996 {
            a.reset().unwrap();
            resets += 1;
        }
        let bytes = match rng.below(10) {
            0 => 0,
            1 => usize::MAX - rng.below(100) as usize,
            2 => rng.next() as usize,
            3 => (cap as u64 + rng.below(4096)) as usize,
            _ => rng.below(2048) as usize,
        };
        let width = widths[rng.below(4) as usize];
        let before = a.used();
        match a.alloc(bytes, width) {
            Ok(s) => {
                ok += 1;
                assert_eq!(s.offset(), before);
                assert_eq!(s.offset() % SLAB_ALIGN, 0);
                assert_eq!(s.padded_len() % SLAB_ALIGN, 0);
                assert!(s.padded_len() >= s.len());
                assert_eq!(a.used(), before + s.padded_len());
                assert!(a.region().alignment_of(&s.read_cap()).unwrap() >= SLAB_ALIGN);
            }
            Err(SysError::CapExceeded { .. }) => {
                over += 1;
                assert_eq!(a.used(), before);
            }
            Err(SysError::ZeroSize) => {
                zero += 1;
                assert_eq!(a.used(), before);
            }
            Err(SysError::Misaligned) => assert_eq!(a.used(), before),
            Err(other) => panic!("unexpected {other:?}"),
        }
        assert!(a.used() <= cap);
        assert_eq!(a.region().len(), cap);
    }
    assert!(ok > 1000 && over > 1000 && zero > 1000 && resets > 50, "ok={ok} over={over} zero={zero} resets={resets}");
}

#[test]
fn slabs_in_one_arena_are_disjoint() {
    let mut a = Arena::with_cap(1 << 16).unwrap();
    let mut slabs = Vec::new();
    let mut rng = Rng(7);
    while let Ok(s) = a.alloc(1 + rng.below(700) as usize, Width::W8) {
        slabs.push(s);
    }
    assert!(slabs.len() > 50);
    for (i, s) in slabs.iter().enumerate() {
        a.region_mut().view_mut::<u8>(&s.write_cap()).unwrap().fill(i as u8 + 1);
    }
    for (i, s) in slabs.iter().enumerate() {
        let v = a.region().view::<u8>(&s.read_cap()).unwrap();
        assert!(v.iter().all(|&b| b == i as u8 + 1), "slab {i} was overwritten");
    }
}

#[test]
fn two_arenas_cannot_alias() {
    let mut a = Arena::with_cap(1 << 16).unwrap();
    let mut b = Arena::with_cap(1 << 16).unwrap();
    let sa = a.alloc(1 << 16, Width::W8).unwrap();
    let sb = b.alloc(1 << 16, Width::W8).unwrap();
    assert_ne!(a.region().id(), b.region().id());
    a.region_mut().view_mut::<u8>(&sa.write_cap()).unwrap().fill(0xAA);
    b.region_mut().view_mut::<u8>(&sb.write_cap()).unwrap().fill(0x55);
    assert!(a.region().view::<u8>(&sa.read_cap()).unwrap().iter().all(|&x| x == 0xAA));
    assert!(b.region().view::<u8>(&sb.read_cap()).unwrap().iter().all(|&x| x == 0x55));
    assert_eq!(b.region().view::<u8>(&sa.read_cap()).err(), Some(SysError::WrongRegion));
    assert_eq!(a.region().view::<u8>(&sb.read_cap()).err(), Some(SysError::WrongRegion));
    assert_eq!(
        b.region_mut().write::<u8>(&sa.write_cap(), 0, 1).err(),
        Some(SysError::WrongRegion)
    );
}

#[test]
fn reset_zeroes_memory_revokes_old_slabs_and_restores_full_capacity() {
    let mut a = Arena::with_cap(4096).unwrap();
    let s = a.alloc(4096, Width::W8).unwrap();
    a.region_mut().view_mut::<u8>(&s.write_cap()).unwrap().fill(0xFF);
    assert_eq!(a.remaining(), 0);
    a.reset().unwrap();
    assert_eq!(a.used(), 0);
    assert_eq!(a.remaining(), 4096);
    assert_eq!(a.region().view::<u8>(&s.read_cap()).err(), Some(SysError::WrongRegion));
    let t = a.alloc(4096, Width::W8).unwrap();
    assert!(a.region().view::<u8>(&t.read_cap()).unwrap().iter().all(|&b| b == 0));
    assert!(a.alloc(1, Width::W8).is_err());
}
