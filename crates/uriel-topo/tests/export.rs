use uriel_topo::export::KernelCsr;
use uriel_topo::persistence::Metric;
use uriel_topo::rng::SplitMix64;
use uriel_topo::schedule::{dense_causal, schedule, Blocks, Schedule, ScheduleParams};
use uriel_topo::Error;

fn line_blocks(centroids: &[f64]) -> Blocks {
    let n = centroids.len();
    Blocks::new((0..n as u64).collect(), centroids.to_vec(), 1, vec![1; n]).unwrap()
}

fn random_blocks(n: usize, dim: usize, seed: u64) -> Blocks {
    let mut rng = SplitMix64::new(seed);
    let ids: Vec<u64> = (0..n as u64).map(|i| i * 3 + 10).collect();
    let c: Vec<f64> = (0..n * dim).map(|_| rng.normal()).collect();
    let tokens: Vec<usize> = (0..n).map(|_| 16 + rng.below(100)).collect();
    Blocks::new(ids, c, dim, tokens).unwrap()
}

fn params(sink: usize, local: usize, salient: usize, relevant: usize) -> ScheduleParams {
    ScheduleParams {
        sink_blocks: sink,
        local_window: local,
        salient_k: salient,
        relevant_m: relevant,
        token_budget: None,
        metric: Metric::Euclidean,
    }
}

fn export(b: &Blocks, p: &ScheduleParams, q: Option<&[f64]>) -> KernelCsr {
    KernelCsr::from_schedule(&schedule(b, p, q).unwrap(), 32).unwrap()
}

fn rows(c: &KernelCsr) -> Vec<Vec<i32>> {
    (0..c.blocks())
        .map(|i| c.indices()[c.offsets()[i] as usize..c.offsets()[i + 1] as usize].to_vec())
        .collect()
}

#[test]
fn sink_one_local_two_matches_hand_computed_kernel_rows() {
    let b = line_blocks(&[0.0, 10.0, 20.0, 30.0, 40.0, 50.0]);
    let c = export(&b, &params(1, 2, 0, 0), None);
    assert_eq!(c.offsets(), &[0, 1, 3, 6, 9, 12, 15]);
    assert_eq!(c.indices(), &[0, 0, 1, 0, 1, 2, 0, 2, 3, 0, 3, 4, 0, 4, 5]);
}

#[test]
fn fewer_blocks_than_sink_plus_local_equals_dense_causal() {
    let b = line_blocks(&[0.0, 10.0, 20.0]);
    let c = export(&b, &params(4, 5, 0, 0), None);
    assert_eq!(c.offsets(), &[0, 1, 3, 6]);
    assert_eq!(c.indices(), &[0, 0, 1, 0, 1, 2]);
}

#[test]
fn single_block_is_one_diagonal_entry() {
    let b = line_blocks(&[3.0]);
    let c = export(&b, &params(1, 4, 8, 0), None);
    assert_eq!(c.offsets(), &[0, 1]);
    assert_eq!(c.indices(), &[0]);
}

#[test]
fn all_blocks_selected_is_byte_identical_to_dense_causal() {
    let b = random_blocks(23, 4, 5);
    let sparse = export(&b, &params(1, 1, 23, 0), None);
    let dense = KernelCsr::from_schedule(&dense_causal(23), 32).unwrap();
    assert_eq!(sparse.encode(), dense.encode());
    assert_eq!(dense.offsets()[23], 23 * 24 / 2);
}

#[test]
fn relevant_blocks_for_the_last_row_are_nearest_centroids_to_the_query() {
    let b = line_blocks(&[0.0, 10.0, 20.0, 30.0, 40.0, 50.0]);
    let c = export(&b, &params(1, 1, 0, 2), Some(&[12.0]));
    assert_eq!(
        rows(&c),
        vec![
            vec![0],
            vec![0, 1],
            vec![0, 2],
            vec![0, 3],
            vec![0, 4],
            vec![0, 1, 2, 5],
        ]
    );
}

#[test]
fn duplicate_centroids_break_salient_ties_by_lowest_block_index() {
    let b = line_blocks(&[7.0; 5]);
    let c = export(&b, &params(1, 1, 2, 0), None);
    assert_eq!(
        rows(&c),
        vec![
            vec![0],
            vec![0, 1],
            vec![0, 1, 2],
            vec![0, 1, 3],
            vec![0, 1, 4],
        ]
    );
}

#[test]
fn duplicate_centroids_with_relevance_break_distance_ties_by_lowest_block_index() {
    let b = line_blocks(&[7.0; 4]);
    let c = export(&b, &params(0, 1, 0, 2), Some(&[7.0]));
    assert_eq!(rows(&c)[3], vec![0, 1, 3]);
}

#[test]
fn missing_diagonal_is_added_like_the_kernel_reference_builder() {
    let b = line_blocks(&[0.0, 10.0, 20.0, 30.0]);
    let c = export(&b, &params(1, 0, 0, 0), None);
    assert_eq!(rows(&c), vec![vec![0], vec![0, 1], vec![0, 2], vec![0, 3]]);
}

#[test]
fn every_exported_row_is_causal_sorted_unique_and_ends_on_the_diagonal() {
    for seed in 0..40u64 {
        let mut rng = SplitMix64::new(seed);
        let n = 1 + rng.below(30);
        let b = random_blocks(n, 3, seed);
        let p = params(rng.below(4), rng.below(5), rng.below(6), 0);
        let c = export(&b, &p, None);
        assert_eq!(c.blocks(), n);
        for (i, row) in rows(&c).iter().enumerate() {
            assert!(row.windows(2).all(|w| w[0] < w[1]));
            assert_eq!(*row.last().unwrap(), i as i32);
            assert!(row[0] >= 0);
        }
    }
}

#[test]
fn export_matches_independent_sink_local_oracle_on_random_shapes() {
    for seed in 0..60u64 {
        let mut rng = SplitMix64::new(seed + 1000);
        let n = 1 + rng.below(40);
        let sink = rng.below(6);
        let local = 1 + rng.below(6);
        let b = random_blocks(n, 2, seed);
        let c = export(&b, &params(sink, local, 0, 0), None);
        for (i, row) in rows(&c).iter().enumerate() {
            let mut want: Vec<i32> = (0..sink.min(i + 1)).map(|x| x as i32).collect();
            want.extend(((i + 1).saturating_sub(local)..=i).map(|x| x as i32));
            want.sort_unstable();
            want.dedup();
            assert_eq!(*row, want);
        }
    }
}

#[test]
fn golden_bytes_for_a_two_block_dense_schedule() {
    let c = KernelCsr::from_schedule(&dense_causal(2), 16).unwrap();
    let want: Vec<u8> = vec![
        b'U', b'C', b'S', b'R', 1, 0, 0, 0, 16, 0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0, 0, 0, 0, 0, 1, 0,
        0, 0, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0,
    ];
    assert_eq!(c.encode(), want);
}

#[test]
fn encoding_is_deterministic_across_runs_and_threads() {
    let b = random_blocks(31, 4, 9);
    let p = params(2, 3, 4, 0);
    let base = export(&b, &p, None).encode();
    assert_eq!(export(&b, &p, None).encode(), base);
    let handles: Vec<_> = (0..4)
        .map(|_| {
            let b = b.clone();
            let p = p.clone();
            std::thread::spawn(move || export(&b, &p, None).encode())
        })
        .collect();
    for h in handles {
        assert_eq!(h.join().unwrap(), base);
    }
}

#[test]
fn decode_inverts_encode_for_random_schedules() {
    for seed in 0..50u64 {
        let mut rng = SplitMix64::new(seed + 77);
        let n = 1 + rng.below(35);
        let b = random_blocks(n, 3, seed);
        let p = params(rng.below(3), 1 + rng.below(4), rng.below(5), 0);
        let c = export(&b, &p, None);
        let back = KernelCsr::decode(&c.encode()).unwrap();
        assert_eq!(back, c);
        assert_eq!(back.block_size(), 32);
    }
}

#[test]
fn exported_arrays_equal_schedule_to_csr_when_the_diagonal_is_already_present() {
    let b = random_blocks(20, 3, 7);
    let s = schedule(&b, &params(2, 3, 4, 0), None).unwrap();
    let (indptr, indices) = s.to_csr().unwrap();
    let c = KernelCsr::from_schedule(&s, 64).unwrap();
    assert_eq!(c.offsets(), indptr.as_slice());
    assert_eq!(c.indices(), indices.as_slice());
}

#[test]
fn block_size_must_be_a_positive_power_of_two() {
    let s = dense_causal(3);
    for bad in [0u32, 3, 24, 100] {
        assert!(matches!(
            KernelCsr::from_schedule(&s, bad),
            Err(Error::InvalidParameter(_))
        ));
    }
    for good in [1u32, 16, 128, 1 << 20] {
        assert!(KernelCsr::from_schedule(&s, good).is_ok());
    }
}

#[test]
fn schedule_that_is_not_causal_is_rejected() {
    let s = Schedule {
        indptr: vec![0, 1, 3],
        indices: vec![0, 0, 2],
    };
    assert!(KernelCsr::from_schedule(&s, 16).is_err());
}

#[test]
fn constructor_rejects_broken_csr_structure() {
    assert!(KernelCsr::new(16, vec![0, 1, 3], vec![0, 0, 1]).is_ok());
    assert!(KernelCsr::new(16, vec![], vec![]).is_err());
    assert!(KernelCsr::new(16, vec![1, 2], vec![0]).is_err());
    assert!(KernelCsr::new(16, vec![0, 1, 3], vec![0, 0]).is_err());
    assert!(KernelCsr::new(16, vec![0, 3, 1], vec![0, 0, 1]).is_err());
    assert!(KernelCsr::new(16, vec![0, 1, 3], vec![0, 1, 1]).is_err());
    assert!(KernelCsr::new(16, vec![0, 1, 3], vec![0, 1, 0]).is_err());
    assert!(KernelCsr::new(16, vec![0, 1, 3], vec![0, -1, 1]).is_err());
    assert!(KernelCsr::new(16, vec![0, 1, 3], vec![0, 0, 2]).is_err());
}

#[test]
fn decode_rejects_each_header_and_length_fault() {
    let good = KernelCsr::from_schedule(&dense_causal(3), 16)
        .unwrap()
        .encode();
    assert!(KernelCsr::decode(&good).is_ok());
    assert!(KernelCsr::decode(&[]).is_err());
    assert!(KernelCsr::decode(&good[..19]).is_err());
    assert!(KernelCsr::decode(&good[..good.len() - 1]).is_err());
    let mut longer = good.clone();
    longer.push(0);
    assert!(KernelCsr::decode(&longer).is_err());
    let mut bad_magic = good.clone();
    bad_magic[0] = b'X';
    assert!(KernelCsr::decode(&bad_magic).is_err());
    let mut bad_version = good.clone();
    bad_version[4] = 2;
    assert!(KernelCsr::decode(&bad_version).is_err());
    let mut bad_block = good.clone();
    bad_block[8] = 24;
    assert!(KernelCsr::decode(&bad_block).is_err());
    let mut huge_blocks = good.clone();
    huge_blocks[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(KernelCsr::decode(&huge_blocks).is_err());
    let mut huge_nnz = good.clone();
    huge_nnz[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(KernelCsr::decode(&huge_nnz).is_err());
}

#[test]
fn decode_never_panics_on_20000_random_byte_strings() {
    let mut rng = SplitMix64::new(0xC5);
    let seeds: Vec<Vec<u8>> = (1..6)
        .map(|n| {
            KernelCsr::from_schedule(&dense_causal(n * 2), 32)
                .unwrap()
                .encode()
        })
        .collect();
    let mut accepted = 0usize;
    for i in 0..20_000usize {
        let bytes: Vec<u8> = match i % 4 {
            0 => (0..rng.below(80)).map(|_| rng.next_u64() as u8).collect(),
            1 => {
                let mut v = vec![b'U', b'C', b'S', b'R', 1, 0, 0, 0];
                v.extend((0..rng.below(64)).map(|_| rng.next_u64() as u8));
                v
            }
            _ => {
                let mut v = seeds[rng.below(seeds.len())].clone();
                for _ in 0..1 + rng.below(4) {
                    let at = rng.below(v.len());
                    v[at] = rng.next_u64() as u8;
                }
                if rng.below(4) == 0 {
                    v.truncate(rng.below(v.len() + 1));
                }
                v
            }
        };
        if let Ok(c) = KernelCsr::decode(&bytes) {
            accepted += 1;
            assert_eq!(c.encode(), bytes);
        }
    }
    assert!(accepted < 20_000);
}
