use uriel_algo::{Stable, Stop, XorShift64};

fn reference(values: &[u8], need: usize, cap: usize) -> Vec<Stop> {
    let mut out = Vec::new();
    let mut streak = 0;
    for (i, v) in values.iter().enumerate() {
        streak = if i > 0 && values[i - 1] == *v { streak + 1 } else { 1 };
        out.push(if streak >= need {
            Stop::Converged
        } else if i + 1 >= cap {
            Stop::CapReached
        } else {
            Stop::Continue
        });
    }
    out
}

fn drive(values: &[u8], need: usize, cap: usize) -> Vec<Stop> {
    let mut s = Stable::new(need, cap);
    values.iter().map(|&v| s.observe(v)).collect()
}

#[test]
fn two_equal_observations_in_a_row_converge_on_the_second() {
    assert_eq!(
        drive(&[1, 2, 2], 2, 10),
        vec![Stop::Continue, Stop::Continue, Stop::Converged]
    );
}

#[test]
fn equal_observations_that_are_not_consecutive_do_not_converge() {
    assert_eq!(
        drive(&[1, 2, 1, 2, 1], 2, 10),
        vec![Stop::Continue; 5]
    );
}

#[test]
fn a_need_of_one_converges_on_the_first_observation() {
    assert_eq!(drive(&[7], 1, 10), vec![Stop::Converged]);
}

#[test]
fn the_cap_stops_a_sequence_that_never_repeats_on_exactly_the_cap_th_observation() {
    let values: Vec<u8> = (0..20).collect();
    let got = drive(&values, 2, 5);
    assert_eq!(&got[..4], &[Stop::Continue; 4]);
    assert_eq!(got[4], Stop::CapReached);
}

#[test]
fn convergence_on_the_cap_th_observation_wins_over_the_cap() {
    assert_eq!(drive(&[1, 1], 2, 2), vec![Stop::Continue, Stop::Converged]);
}

#[test]
fn a_zero_need_or_cap_behaves_as_one() {
    assert_eq!(drive(&[1], 0, 5), vec![Stop::Converged]);
    assert_eq!(drive(&[1, 2], 2, 0), vec![Stop::CapReached, Stop::CapReached]);
}

#[test]
fn observing_agrees_with_the_reference_on_random_sequences() {
    let mut rng = XorShift64::new(0x57AB1E);
    for _ in 0..4000 {
        let len = 1 + (rng.next_u64() % 16) as usize;
        let alphabet = 1 + rng.next_u64() % 3;
        let values: Vec<u8> = (0..len).map(|_| (rng.next_u64() % alphabet) as u8).collect();
        let need = 1 + (rng.next_u64() % 4) as usize;
        let cap = 1 + (rng.next_u64() % 12) as usize;
        assert_eq!(
            drive(&values, need, cap),
            reference(&values, need, cap),
            "{values:?} need {need} cap {cap}"
        );
    }
}

#[test]
fn two_instances_fed_the_same_sequence_agree() {
    let mut rng = XorShift64::new(0xD37);
    for _ in 0..500 {
        let values: Vec<u8> = (0..12).map(|_| (rng.next_u64() % 3) as u8).collect();
        assert_eq!(drive(&values, 3, 9), drive(&values, 3, 9));
    }
}

#[test]
fn non_copy_values_are_compared_by_equality() {
    let mut s = Stable::new(2, 5);
    assert_eq!(s.observe(String::from("a")), Stop::Continue);
    assert_eq!(s.observe(String::from("a")), Stop::Converged);
}
