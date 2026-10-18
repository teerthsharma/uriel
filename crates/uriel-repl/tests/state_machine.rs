mod common;

use common::{digest, props, Mini};
use uriel_core::{Command, Value, WriteBatch};
use uriel_kv::raft::{Index, NodeId, Raft, Role, Storage};
use uriel_kv::sim::SimDisk;
use uriel_repl::{Ack, Fault, Replica, WriteError};

fn lone_leader_with(entries: &[Vec<u8>]) -> common::Rep {
    let disk = SimDisk::default();
    let backend = std::rc::Rc::new(std::cell::RefCell::new(common::MemBackend::default()));
    let inner = uriel_kv::durable::FileStorage::open(disk.clone(), 8).unwrap();
    let storage = common::NodeStorage { inner, drop_applied: false };
    let mut raft = Raft::new(NodeId(1), vec![NodeId(1)], storage, 1).unwrap();
    while raft.role() != Role::Leader {
        raft.tick();
    }
    for e in entries {
        raft.propose(e.clone()).unwrap();
    }
    let graph = common::open_graph(&backend.borrow());
    Replica::new(raft, graph, Box::new(|| unreachable!("no snapshot in this test")))
}

#[test]
fn the_builder_allocates_ids_from_the_applied_counters_and_leaves_the_live_graph_alone() {
    let mut c = Mini::new(3, 3);
    let l = c.elect();
    let before = digest(c.nodes[l].rep.graph());
    let t = c.nodes[l]
        .rep
        .write(|b| {
            let a = b.create_node(&["A"], props(&[("k", Value::Int(1))]));
            let n = b.create_node(&["A"], props(&[]));
            b.create_rel(a, n, "R", props(&[]));
        })
        .unwrap();
    assert_eq!(digest(c.nodes[l].rep.graph()), before);
    c.pump();
    assert_eq!(c.nodes[l].rep.drain_acks(), vec![(t, Ack::Committed { epoch: 1 })]);
    let read = c.nodes[l].rep.graph().begin_read().unwrap();
    assert_eq!(read.next_ids().unwrap(), (2, 1));
}

#[test]
fn a_write_is_acknowledged_only_after_it_is_committed_and_applied_on_the_leader() {
    let mut c = Mini::new(3, 4);
    let l = c.elect();
    let t = c.nodes[l]
        .rep
        .write(|b| {
            b.create_node(&["A"], props(&[]));
        })
        .unwrap();
    assert!(c.nodes[l].rep.drain_acks().is_empty());
    assert_eq!(c.nodes[l].rep.graph().epoch(), 0);
    let staged = c.nodes[l].rep.stage().unwrap();
    let msgs = staged.messages().to_vec();
    c.nodes[l].rep.finish(staged);
    assert!(c.nodes[l].rep.drain_acks().is_empty());
    c.route(msgs);
    c.deliver_queue();
    assert!(c.nodes[l].rep.drain_acks().is_empty());
    c.pump();
    assert_eq!(c.nodes[l].rep.drain_acks(), vec![(t, Ack::Committed { epoch: 1 })]);
    assert_eq!(c.nodes[l].rep.graph().epoch(), 1);
}

#[test]
fn an_uncommitted_write_is_invisible_on_every_replica_and_is_reported_unknown_after_the_leader_falls() {
    let mut c = Mini::new(3, 5);
    let l = c.elect();
    c.isolated.insert(l);
    let t = c.nodes[l]
        .rep
        .write(|b| {
            b.create_node(&["Ghost"], props(&[]));
        })
        .unwrap();
    c.tick(30);
    for n in &c.nodes {
        assert_eq!(n.rep.graph().epoch(), 0);
    }
    assert!(c.nodes[l].rep.drain_acks().is_empty());
    let new_leader = c.leader().unwrap();
    assert_ne!(new_leader, l);
    c.nodes[new_leader]
        .rep
        .write(|b| {
            b.create_node(&["Real"], props(&[]));
        })
        .unwrap();
    c.tick(5);
    c.isolated.clear();
    c.tick(40);
    assert_eq!(c.nodes[l].rep.drain_acks(), vec![(t, Ack::Unknown)]);
    let want = digest(c.nodes[new_leader].rep.graph());
    for n in &c.nodes {
        assert_eq!(digest(n.rep.graph()), want);
    }
    assert_eq!(c.nodes[l].rep.graph().epoch(), 1);
}

#[test]
fn a_follower_and_a_busy_leader_refuse_writes_and_a_rejected_batch_proposes_nothing() {
    let mut c = Mini::new(3, 6);
    let l = c.elect();
    let f = (l + 1) % 3;
    assert!(matches!(c.nodes[f].rep.write(|_| {}), Err(WriteError::NotLeader(_))));
    assert!(matches!(c.nodes[l].rep.write(|_| {}), Err(WriteError::Empty)));
    let last = c.nodes[l].rep.raft().storage().last_index();
    let bad = c.nodes[l].rep.write(|b| b.delete_node(uriel_core::NodeId(404)));
    assert!(matches!(bad, Err(WriteError::Rejected(_))));
    assert_eq!(c.nodes[l].rep.raft().storage().last_index(), last);
    c.nodes[l]
        .rep
        .write(|b| {
            b.create_node(&["A"], props(&[]));
        })
        .unwrap();
    let again = c.nodes[l].rep.write(|b| {
        b.create_node(&["A"], props(&[]));
    });
    assert!(matches!(again, Err(WriteError::Busy)));
}

#[test]
fn an_entry_redelivered_after_a_crash_between_apply_and_advance_is_skipped_and_never_applied_twice() {
    let mut c = Mini::new(3, 7);
    let l = c.elect();
    c.nodes[l]
        .rep
        .write(|b| {
            b.create_node(&["A"], props(&[("k", Value::Int(1))]));
        })
        .unwrap();
    let staged = c.nodes[l].rep.stage().unwrap();
    let msgs = staged.messages().to_vec();
    c.nodes[l].rep.finish(staged);
    c.route(msgs);
    c.deliver_queue();
    for i in 0..3 {
        if i != l {
            c.stage_and_finish(i);
        }
    }
    c.deliver_queue();
    let staged = c.nodes[l].rep.stage().unwrap();
    assert_eq!(c.nodes[l].rep.graph().epoch(), 1);
    let applied_before = c.nodes[l].rep.raft().applied_index();
    drop(staged);
    let before = digest(c.nodes[l].rep.graph());
    c.restart(l);
    assert_eq!(c.nodes[l].rep.raft().applied_index(), applied_before);
    c.nodes[l].rep.stage().unwrap();
    assert!(c.nodes[l].rep.fault().is_none());
    assert_eq!(c.nodes[l].rep.redelivered(), 1);
    assert_eq!(digest(c.nodes[l].rep.graph()), before);
}

#[test]
fn an_entry_that_cannot_decode_latches_a_fault_and_is_never_skipped() {
    let junk = vec![0xde, 0xad, 0xbe, 0xef, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
    let mut r = lone_leader_with(&[junk]);
    assert!(r.stage().is_none());
    assert!(matches!(r.fault(), Some(Fault::Decode { .. })));
    assert!(r.stage().is_none());
    assert_eq!(r.raft().applied_index(), Index(0));
    let w = r.write(|b| {
        b.create_node(&["A"], props(&[]));
    });
    assert!(matches!(w, Err(WriteError::Faulted)));
}

#[test]
fn a_batch_from_the_future_latches_a_fault_and_changes_nothing() {
    let gap = WriteBatch {
        epoch: 5,
        commands: vec![Command::CreateNode {
            id: uriel_core::NodeId(0),
            labels: Default::default(),
            props: props(&[]),
        }],
    };
    let mut r = lone_leader_with(&[gap.encode()]);
    let before = digest(r.graph());
    assert!(r.stage().is_none());
    assert!(matches!(r.fault(), Some(Fault::Apply { .. })));
    assert_eq!(digest(r.graph()), before);
    assert_eq!(r.graph().epoch(), 0);
}

#[test]
fn a_replica_that_missed_compacted_entries_catches_up_from_a_snapshot_to_an_equal_graph() {
    let mut c = Mini::new(3, 9);
    let l = c.elect();
    let lag = (l + 1) % 3;
    c.isolated.insert(lag);
    for k in 0..6 {
        c.nodes[l]
            .rep
            .write(|b| {
                let n = b.create_node(&["A"], props(&[("k", Value::Int(k))]));
                b.create_rel(n, n, "SELF", props(&[]));
            })
            .unwrap();
        c.pump();
        c.nodes[l].rep.drain_acks();
    }
    assert!(c.nodes[l].rep.compact().unwrap());
    assert_eq!(c.nodes[lag].rep.graph().epoch(), 0);
    c.isolated.clear();
    c.tick(30);
    assert_eq!(c.nodes[lag].rep.snapshots_installed(), 1);
    assert_eq!(digest(c.nodes[lag].rep.graph()), digest(c.nodes[l].rep.graph()));
    assert_eq!(c.nodes[lag].rep.graph().epoch(), 6);
    c.nodes[l].rep.write(|b| {
        b.create_node(&["After"], props(&[]));
    })
    .unwrap();
    c.tick(6);
    assert_eq!(digest(c.nodes[lag].rep.graph()), digest(c.nodes[l].rep.graph()));
    assert!(c.nodes[lag].rep.fault().is_none());
}
