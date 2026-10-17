use std::collections::BTreeSet;

use super::{mem, p};
use crate::checksum::crc32c;
use crate::{Command, ConstraintKind, Error, Graph, NodeId, RelId, Value, WriteBatch};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn labels(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}

fn every_command_kind() -> WriteBatch {
    WriteBatch {
        epoch: 7,
        commands: vec![
            Command::CreateIndex {
                label: "A".into(),
                key: "k".into(),
            },
            Command::CreateConstraint {
                kind: ConstraintKind::Unique,
                label: "A".into(),
                key: "k".into(),
            },
            Command::CreateConstraint {
                kind: ConstraintKind::Exists,
                label: "A".into(),
                key: "k".into(),
            },
            Command::CreateNode {
                id: NodeId(10),
                labels: labels(&["A", "B"]),
                props: p(&[
                    ("k", Value::Int(1)),
                    ("f", Value::Float(-0.0)),
                    ("s", Value::from("h\u{e9}")),
                    ("l", Value::List(vec![Value::Bool(true), Value::Null])),
                ]),
            },
            Command::CreateNode {
                id: NodeId(11),
                labels: labels(&["A"]),
                props: p(&[("k", Value::Int(2))]),
            },
            Command::SetNodeProps {
                id: NodeId(10),
                props: p(&[("k", Value::Null), ("z", Value::Bytes(vec![0, 9]))]),
            },
            Command::AddLabel {
                id: NodeId(10),
                label: "C".into(),
            },
            Command::RemoveLabel {
                id: NodeId(10),
                label: "B".into(),
            },
            Command::CreateRel {
                id: RelId(3),
                src: NodeId(10),
                dst: NodeId(11),
                rel_type: "R".into(),
                props: p(&[("w", Value::Vector(vec![1.5, -2.0]))]),
            },
            Command::SetRelProps {
                id: RelId(3),
                props: p(&[("w", Value::Null)]),
            },
            Command::DeleteRel { id: RelId(3) },
            Command::DeleteNode { id: NodeId(10) },
            Command::DropConstraint {
                kind: ConstraintKind::Unique,
                label: "A".into(),
                key: "k".into(),
            },
            Command::DropIndex {
                label: "A".into(),
                key: "k".into(),
            },
        ],
    }
}

fn node_ids(g: &Graph) -> Vec<u64> {
    g.begin_read()
        .unwrap()
        .all_nodes()
        .unwrap()
        .map(|n| n.unwrap().id.0)
        .collect()
}

#[test]
fn batch_encoding_round_trips_every_command_kind() {
    let batch = every_command_kind();
    let bytes = batch.encode();
    assert_eq!(WriteBatch::decode(&bytes).unwrap(), batch);
    assert_eq!(WriteBatch::decode(&bytes).unwrap().encode(), bytes);
}

#[test]
fn batch_encoding_is_versioned_and_checksummed() {
    let bytes = every_command_kind().encode();
    assert_eq!(bytes[0], 1);
    let (body, tail) = bytes.split_at(bytes.len() - 4);
    assert_eq!(tail, crc32c(body).to_le_bytes());
}

#[test]
fn every_single_bit_flip_of_a_batch_is_rejected() {
    let bytes = every_command_kind().encode();
    for bit in 0..bytes.len() * 8 {
        let mut bad = bytes.clone();
        bad[bit / 8] ^= 1 << (bit % 8);
        assert!(WriteBatch::decode(&bad).is_err(), "bit {bit} accepted");
    }
}

#[test]
fn every_truncation_of_a_batch_is_rejected() {
    let bytes = every_command_kind().encode();
    for cut in 0..bytes.len() {
        assert!(WriteBatch::decode(&bytes[..cut]).is_err(), "cut {cut}");
    }
}

#[test]
fn unknown_version_is_rejected_even_with_a_valid_checksum() {
    let mut bytes = every_command_kind().encode();
    bytes[0] = 2;
    let n = bytes.len() - 4;
    let crc = crc32c(&bytes[..n]).to_le_bytes();
    bytes[n..].copy_from_slice(&crc);
    assert!(matches!(
        WriteBatch::decode(&bytes),
        Err(Error::InvalidBatch(_))
    ));
}

#[test]
fn decoder_never_panics_on_twenty_thousand_adversarial_inputs() {
    let valid = every_command_kind().encode();
    let mut rng = Rng(0xC0FFEE);
    for i in 0..20_000u32 {
        let input: Vec<u8> = match i % 4 {
            0 => (0..rng.below(300)).map(|_| rng.next() as u8).collect(),
            1 => {
                let mut b = valid.clone();
                for _ in 0..1 + rng.below(4) {
                    let at = rng.below(b.len() as u64) as usize;
                    b[at] = rng.next() as u8;
                }
                b
            }
            2 => {
                let mut b = valid[..valid.len() - 4].to_vec();
                for _ in 0..1 + rng.below(6) {
                    let at = rng.below(b.len() as u64) as usize;
                    b[at] = rng.next() as u8;
                }
                if rng.below(2) == 0 {
                    b.truncate(rng.below(b.len() as u64) as usize);
                }
                let crc = crc32c(&b).to_le_bytes();
                b.extend_from_slice(&crc);
                b
            }
            _ => {
                let mut b = vec![1u8];
                b.extend_from_slice(&rng.next().to_le_bytes());
                b.extend_from_slice(&u32::MAX.to_le_bytes());
                for _ in 0..rng.below(40) {
                    b.push(rng.next() as u8);
                }
                let crc = crc32c(&b).to_le_bytes();
                b.extend_from_slice(&crc);
                b
            }
        };
        let _ = WriteBatch::decode(&input);
    }
}

#[test]
fn recording_a_commit_yields_its_commands_with_explicit_ids() {
    let g = mem();
    let mut tx = g.begin_write_recording().unwrap();
    let a = tx.create_node(&["A"], p(&[("k", Value::Int(1))])).unwrap();
    let b = tx.create_node(&["B"], p(&[("gone", Value::Null)])).unwrap();
    let r = tx.create_rel(a, b, "R", p(&[])).unwrap();
    tx.set_node_props(a, p(&[("k", Value::Int(2))])).unwrap();
    tx.add_label(b, "C").unwrap();
    tx.remove_label(b, "B").unwrap();
    tx.set_rel_props(r, p(&[("w", Value::Int(3))])).unwrap();
    let (epoch, batch) = tx.commit_recorded().unwrap();
    let batch = batch.expect("a dirty recording commit yields a batch");
    assert_eq!(epoch, 1);
    assert_eq!(batch.epoch, 1);
    assert_eq!(
        batch.commands,
        vec![
            Command::CreateNode {
                id: a,
                labels: labels(&["A"]),
                props: p(&[("k", Value::Int(1))]),
            },
            Command::CreateNode {
                id: b,
                labels: labels(&["B"]),
                props: p(&[]),
            },
            Command::CreateRel {
                id: r,
                src: a,
                dst: b,
                rel_type: "R".into(),
                props: p(&[]),
            },
            Command::SetNodeProps {
                id: a,
                props: p(&[("k", Value::Int(2))]),
            },
            Command::AddLabel {
                id: b,
                label: "C".into(),
            },
            Command::RemoveLabel {
                id: b,
                label: "B".into(),
            },
            Command::SetRelProps {
                id: r,
                props: p(&[("w", Value::Int(3))]),
            },
        ]
    );
}

#[test]
fn detach_delete_is_recorded_as_explicit_rel_deletes_then_the_node() {
    let g = mem();
    let mut tx = g.begin_write().unwrap();
    let a = tx.create_node(&["A"], p(&[])).unwrap();
    let b = tx.create_node(&["A"], p(&[])).unwrap();
    let r1 = tx.create_rel(a, b, "R", p(&[])).unwrap();
    let r2 = tx.create_rel(b, a, "R", p(&[])).unwrap();
    let r3 = tx.create_rel(a, a, "R", p(&[])).unwrap();
    tx.commit().unwrap();
    let mut tx = g.begin_write_recording().unwrap();
    tx.detach_delete_node(a).unwrap();
    let (_, batch) = tx.commit_recorded().unwrap();
    assert_eq!(
        batch.unwrap().commands,
        vec![
            Command::DeleteRel { id: r1 },
            Command::DeleteRel { id: r2 },
            Command::DeleteRel { id: r3 },
            Command::DeleteNode { id: a },
        ]
    );
}

#[test]
fn rejected_operations_are_not_recorded() {
    let g = mem();
    g.create_unique_constraint("A", "k").unwrap();
    let mut tx = g.begin_write_recording().unwrap();
    tx.create_node(&["A"], p(&[("k", Value::Int(1))])).unwrap();
    let dup = tx.create_node(&["A"], p(&[("k", Value::Int(1))]));
    assert!(matches!(dup, Err(Error::ConstraintViolation { .. })));
    assert!(tx.delete_node(NodeId(99)).is_err());
    let (_, batch) = tx.commit_recorded().unwrap();
    assert_eq!(batch.unwrap().commands.len(), 1);
}

#[test]
fn rolled_back_and_empty_and_unrecorded_transactions_yield_no_batch() {
    let g = mem();
    let mut tx = g.begin_write_recording().unwrap();
    tx.create_node(&["A"], p(&[])).unwrap();
    drop(tx);
    let tx = g.begin_write_recording().unwrap();
    assert_eq!(tx.commit_recorded().unwrap(), (0, None));
    let mut tx = g.begin_write().unwrap();
    tx.create_node(&["A"], p(&[])).unwrap();
    assert_eq!(tx.commit_recorded().unwrap(), (1, None));
}

#[test]
fn applying_a_recorded_batch_reproduces_state_epoch_and_ids() {
    let a = mem();
    let b = mem();
    let mut tx = a.begin_write_recording().unwrap();
    tx.create_index("A", "k").unwrap();
    let n1 = tx.create_node(&["A"], p(&[("k", Value::Int(1))])).unwrap();
    let n2 = tx.create_node(&["A"], p(&[("k", Value::Int(2))])).unwrap();
    tx.create_rel(n1, n2, "R", p(&[])).unwrap();
    let (_, batch) = tx.commit_recorded().unwrap();
    let batch = WriteBatch::decode(&batch.unwrap().encode()).unwrap();
    assert_eq!(b.apply_batch(&batch).unwrap(), 1);
    assert_eq!(b.epoch(), a.epoch());
    assert_eq!(node_ids(&b), vec![n1.0, n2.0]);
    let r = b.begin_read().unwrap();
    assert_eq!(
        r.nodes_by_prop("A", "k", &Value::Int(2)).unwrap().count(),
        1
    );
    assert_eq!(r.out_rels(n1, Some("R")).unwrap().count(), 1);
    assert_eq!(b.list_indexes().unwrap().len(), 1);
}

#[test]
fn applying_the_same_batch_twice_is_rejected_on_epoch() {
    let a = mem();
    let b = mem();
    let mut tx = a.begin_write_recording().unwrap();
    tx.create_node(&["A"], p(&[])).unwrap();
    let batch = tx.commit_recorded().unwrap().1.unwrap();
    b.apply_batch(&batch).unwrap();
    let again = b.apply_batch(&batch);
    assert!(matches!(
        again,
        Err(Error::StaleBatch {
            expected: 2,
            found: 1
        })
    ));
    assert_eq!(node_ids(&b), vec![0]);
}

#[test]
fn a_batch_from_the_future_is_rejected_on_epoch() {
    let a = mem();
    let b = mem();
    for _ in 0..2 {
        let mut tx = a.begin_write_recording().unwrap();
        tx.create_node(&["A"], p(&[])).unwrap();
        tx.commit_recorded().unwrap();
    }
    let mut tx = a.begin_write_recording().unwrap();
    tx.create_node(&["A"], p(&[])).unwrap();
    let third = tx.commit_recorded().unwrap().1.unwrap();
    assert!(matches!(
        b.apply_batch(&third),
        Err(Error::StaleBatch {
            expected: 1,
            found: 3
        })
    ));
    assert!(node_ids(&b).is_empty());
}

#[test]
fn applying_never_allocates_ids_it_takes_them_from_the_batch() {
    let g = mem();
    let batch = WriteBatch {
        epoch: 1,
        commands: vec![
            Command::CreateNode {
                id: NodeId(500),
                labels: labels(&["A"]),
                props: p(&[]),
            },
            Command::CreateNode {
                id: NodeId(900),
                labels: labels(&["A"]),
                props: p(&[]),
            },
        ],
    };
    g.apply_batch(&batch).unwrap();
    assert_eq!(node_ids(&g), vec![500, 900]);
    let mut tx = g.begin_write().unwrap();
    assert_eq!(
        tx.create_node(&["A"], p(&[])).unwrap(),
        NodeId(901),
        "next_node is kept at max id + 1"
    );
}

#[test]
fn applying_keeps_the_rel_counter_above_the_highest_rel_id() {
    let g = mem();
    let batch = WriteBatch {
        epoch: 1,
        commands: vec![
            Command::CreateNode {
                id: NodeId(0),
                labels: labels(&["A"]),
                props: p(&[]),
            },
            Command::CreateRel {
                id: RelId(41),
                src: NodeId(0),
                dst: NodeId(0),
                rel_type: "R".into(),
                props: p(&[]),
            },
        ],
    };
    g.apply_batch(&batch).unwrap();
    let mut tx = g.begin_write().unwrap();
    let next = tx.create_rel(NodeId(0), NodeId(0), "R", p(&[])).unwrap();
    assert_eq!(next, RelId(42));
}

#[test]
fn a_node_id_below_the_counter_is_a_collision_and_the_batch_is_atomic() {
    let g = mem();
    let mut tx = g.begin_write().unwrap();
    tx.create_node(&["A"], p(&[])).unwrap();
    tx.create_node(&["A"], p(&[])).unwrap();
    tx.commit().unwrap();
    let batch = WriteBatch {
        epoch: 2,
        commands: vec![
            Command::CreateNode {
                id: NodeId(2),
                labels: labels(&["B"]),
                props: p(&[]),
            },
            Command::CreateNode {
                id: NodeId(1),
                labels: labels(&["B"]),
                props: p(&[]),
            },
        ],
    };
    assert!(matches!(
        g.apply_batch(&batch),
        Err(Error::BatchIdCollision { .. })
    ));
    assert_eq!(g.epoch(), 1);
    assert_eq!(node_ids(&g), vec![0, 1]);
}

#[test]
fn a_deleted_node_id_is_never_reused_by_a_batch() {
    let g = mem();
    let mut tx = g.begin_write().unwrap();
    let a = tx.create_node(&["A"], p(&[])).unwrap();
    tx.commit().unwrap();
    let mut tx = g.begin_write().unwrap();
    tx.delete_node(a).unwrap();
    tx.commit().unwrap();
    let batch = WriteBatch {
        epoch: 3,
        commands: vec![Command::CreateNode {
            id: a,
            labels: labels(&["A"]),
            props: p(&[]),
        }],
    };
    assert!(matches!(
        g.apply_batch(&batch),
        Err(Error::BatchIdCollision { .. })
    ));
}

#[test]
fn a_duplicate_id_inside_one_batch_is_a_collision() {
    let g = mem();
    let create = |id| Command::CreateNode {
        id: NodeId(id),
        labels: labels(&["A"]),
        props: p(&[]),
    };
    let batch = WriteBatch {
        epoch: 1,
        commands: vec![create(5), create(5)],
    };
    assert!(matches!(
        g.apply_batch(&batch),
        Err(Error::BatchIdCollision { .. })
    ));
    assert!(node_ids(&g).is_empty());
}

#[test]
fn a_rel_id_below_the_counter_is_a_collision() {
    let g = mem();
    let mut tx = g.begin_write().unwrap();
    let a = tx.create_node(&["A"], p(&[])).unwrap();
    tx.create_rel(a, a, "R", p(&[])).unwrap();
    tx.commit().unwrap();
    let batch = WriteBatch {
        epoch: 2,
        commands: vec![Command::CreateRel {
            id: RelId(0),
            src: a,
            dst: a,
            rel_type: "R".into(),
            props: p(&[]),
        }],
    };
    assert!(matches!(
        g.apply_batch(&batch),
        Err(Error::BatchIdCollision { .. })
    ));
}

#[test]
fn an_empty_batch_is_rejected_because_it_cannot_advance_the_epoch() {
    let g = mem();
    let batch = WriteBatch {
        epoch: 1,
        commands: vec![],
    };
    assert!(matches!(g.apply_batch(&batch), Err(Error::InvalidBatch(_))));
    assert_eq!(g.epoch(), 0);
}
