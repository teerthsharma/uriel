//! Table definitions: the on-disk layout. See `docs/DESIGN.md` and the module docs of
//! [`crate::keys`] for key formats.

use redb::TableDefinition;

/// `node id -> node record`
pub(crate) const NODES: TableDefinition<u64, &[u8]> = TableDefinition::new("nodes");
/// `rel id -> rel record`
pub(crate) const RELS: TableDefinition<u64, &[u8]> = TableDefinition::new("rels");
/// `src(8) type 0x00 rel(8) -> dst node id`
pub(crate) const OUT_ADJ: TableDefinition<&[u8], u64> = TableDefinition::new("adj_out");
/// `dst(8) type 0x00 rel(8) -> src node id`
pub(crate) const IN_ADJ: TableDefinition<&[u8], u64> = TableDefinition::new("adj_in");
/// `label 0x00 node(8) -> ()`
pub(crate) const LABEL_IDX: TableDefinition<&[u8], ()> = TableDefinition::new("label_idx");
/// `type 0x00 rel(8) -> ()`
pub(crate) const TYPE_IDX: TableDefinition<&[u8], ()> = TableDefinition::new("type_idx");
/// `label 0x00 key 0x00 value_key node(8) -> ()`, one shared table for all property indexes
pub(crate) const PROP_IDX: TableDefinition<&[u8], ()> = TableDefinition::new("prop_idx");
/// `'L' label` / `'R' type` -> live entity count
pub(crate) const COUNTS: TableDefinition<&[u8], u64> = TableDefinition::new("counts");
/// `label 0x00 key` -> flag (1 = backs a unique constraint)
pub(crate) const INDEXES: TableDefinition<&[u8], u8> = TableDefinition::new("indexes");
/// `kind label 0x00 key` -> ()
pub(crate) const CONSTRAINTS: TableDefinition<&[u8], ()> = TableDefinition::new("constraints");
/// counters: `format`, `next_node`, `next_rel`, `node_count`, `rel_count`, `epoch`
pub(crate) const META: TableDefinition<&str, u64> = TableDefinition::new("meta");

/// Keys of the [`META`] table.
pub(crate) mod meta {
    pub(crate) const FORMAT: &str = "format";
    pub(crate) const NEXT_NODE: &str = "next_node";
    pub(crate) const NEXT_REL: &str = "next_rel";
    pub(crate) const NODE_COUNT: &str = "node_count";
    pub(crate) const REL_COUNT: &str = "rel_count";
    pub(crate) const EPOCH: &str = "epoch";
}
pub(crate) const NODE_SUMS: TableDefinition<u64, u32> = TableDefinition::new("node_sums");
pub(crate) const REL_SUMS: TableDefinition<u64, u32> = TableDefinition::new("rel_sums");
