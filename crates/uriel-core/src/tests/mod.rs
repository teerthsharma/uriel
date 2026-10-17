mod budget;
mod codec;
mod constraints;
mod crash;
mod degraded;
mod epoch;
mod fault;
mod graph;
mod index;
mod integrity;
mod placed;
mod schema;
mod tier;
mod value;

use std::path::PathBuf;

use redb::{ReadableDatabase, ReadableTable, ReadableTableMetadata, TableDefinition};

use crate::{Graph, Props, Value};

pub(crate) fn mem() -> Graph {
    Graph::open_in_memory(crate::MemoryBudget::mib(80)).expect("open in memory")
}

pub(crate) fn p(pairs: &[(&str, Value)]) -> Props {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), v.clone()))
        .collect()
}

pub(crate) fn tmp_graph_path() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("seal.redb");
    (dir, path)
}

/// Number of entries in a raw table, bypassing the public API.
pub(crate) fn raw_len<K: redb::Key + 'static, V: redb::Value + 'static>(
    g: &Graph,
    def: TableDefinition<K, V>,
) -> u64 {
    let r = g.db().begin_read().unwrap();
    r.open_table(def).unwrap().len().unwrap()
}

/// All keys of a raw `&[u8]`-keyed table.
pub(crate) fn raw_keys<V: redb::Value + 'static>(
    g: &Graph,
    def: TableDefinition<&[u8], V>,
) -> Vec<Vec<u8>> {
    let r = g.db().begin_read().unwrap();
    let t = r.open_table(def).unwrap();
    t.iter()
        .unwrap()
        .map(|e| e.unwrap().0.value().to_vec())
        .collect()
}
