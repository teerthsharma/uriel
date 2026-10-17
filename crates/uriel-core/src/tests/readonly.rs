use redb::ReadableDatabase;

use super::{p, tmp_graph_path};
use crate::tables::{meta, META};
use crate::{Error, Graph, MemoryBudget, Mode};

fn budget() -> MemoryBudget {
    MemoryBudget::mib(32)
}

fn seeded_at_format(path: &std::path::Path, format: u64) {
    let g = Graph::open(path, budget()).unwrap();
    let mut tx = g.begin_write().unwrap();
    tx.create_node(&["Person"], p(&[("name", "Ada".into())])).unwrap();
    tx.commit().unwrap();
    let w = g.db().begin_write().unwrap();
    w.open_table(META).unwrap().insert(meta::FORMAT, format).unwrap();
    w.commit().unwrap();
}

#[test]
fn a_read_only_open_leaves_an_old_format_file_byte_identical() {
    for format in [1u64, 2] {
        let (_dir, path) = tmp_graph_path();
        seeded_at_format(&path, format);
        let before = std::fs::read(&path).unwrap();
        {
            let g = Graph::open_read_only(&path, budget()).unwrap();
            let r = g.begin_read().unwrap();
            assert_eq!(r.node_count().unwrap(), 1);
            assert_eq!(r.nodes_by_label("Person").unwrap().count(), 1);
        }
        assert_eq!(std::fs::read(&path).unwrap(), before, "format {format} file changed");
    }
}

#[test]
fn a_normal_open_does_upgrade_the_same_file_so_the_check_can_fail() {
    let (_dir, path) = tmp_graph_path();
    seeded_at_format(&path, 2);
    let before = std::fs::read(&path).unwrap();
    drop(Graph::open(&path, budget()).unwrap());
    assert_ne!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn a_read_only_graph_refuses_every_write_and_cannot_recover_into_full() {
    let (_dir, path) = tmp_graph_path();
    seeded_at_format(&path, 3);
    let g = Graph::open_read_only(&path, budget()).unwrap();
    assert!(g.is_read_only());
    assert!(matches!(g.begin_write(), Err(Error::ReadOnly)));
    assert!(matches!(g.create_index("Person", "name"), Err(Error::ReadOnly)));
    assert_eq!(g.try_recover().unwrap(), Mode::ReadOnly);
    assert!(matches!(g.begin_write(), Err(Error::ReadOnly)));
}

#[test]
fn a_read_only_graph_sees_the_committed_data() {
    let (_dir, path) = tmp_graph_path();
    seeded_at_format(&path, 3);
    let g = Graph::open_read_only(&path, budget()).unwrap();
    assert_eq!(g.epoch(), 1);
    assert_eq!(g.budget(), budget());
    let nodes: Vec<_> = g
        .begin_read()
        .unwrap()
        .nodes_by_label("Person")
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(nodes[0].props, p(&[("name", "Ada".into())]));
}

#[test]
fn a_read_only_open_of_a_missing_file_fails_and_creates_nothing() {
    let (dir, path) = tmp_graph_path();
    assert!(Graph::open_read_only(&path, budget()).is_err());
    assert!(!path.exists());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn a_normal_graph_is_not_read_only() {
    let g = Graph::open_in_memory(budget()).unwrap();
    assert!(!g.is_read_only());
}
