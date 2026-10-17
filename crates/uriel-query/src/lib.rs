//! # uriel-query
//!
//! A Cypher query engine over [`uriel_core`]. [`Engine::execute`] parses a statement with
//! [`uriel_cypher`], runs it with a tree-walking interpreter against one read snapshot (or
//! the single write transaction) and returns a [`QueryResult`].
//!
//! ```
//! use std::collections::BTreeMap;
//! use uriel_query::{AccessMode, Engine};
//!
//! let engine = Engine::open_in_memory(uriel_core::MemoryBudget::mib(64)).unwrap();
//! let none = BTreeMap::new();
//! engine
//!     .execute("CREATE (:Person {name: 'Ada'})-[:KNOWS]->(:Person {name: 'Bob'})", &none, AccessMode::Write)
//!     .unwrap();
//! let r = engine
//!     .execute("MATCH (a)-[:KNOWS]->(b) RETURN a.name AS a, b.name AS b", &none, AccessMode::Read)
//!     .unwrap();
//! assert_eq!(r.to_json_rows().to_string(), r#"[{"a":"Ada","b":"Bob"}]"#);
//! ```
//!
//! # Result representation
//!
//! Scalars, lists and maps map directly onto [`uriel_core::Value`]. Graph entities are
//! returned as [`Value::Map`] with their properties flattened next to reserved keys
//! (reserved keys win on collision):
//!
//! * **node**: `_id` (integer), `_labels` (list of strings), plus every property;
//! * **relationship**: `_id`, `_type` (string), `_start`, `_end` (node ids), plus every
//!   property;
//! * **path**: a list alternating node maps and relationship maps, starting and ending on
//!   a node.
//!
//! # Transactions
//!
//! One `execute` call is one transaction. A write statement runs inside a single
//! [`uriel_core::WriteTx`] and commits only if every clause succeeds; any error drops the
//! transaction and leaves the graph unchanged.
//!
//! # Result cache
//!
//! Read-mode results are cached in a [`uriel_cache::LocalCache`] keyed by a hash of the
//! query text and parameters. Each entry is tagged with the labels and relationship types
//! the execution touched, or `*` when it scanned unlabeled nodes, followed untyped
//! relationships or called a procedure. A commit invalidates the tags it touched plus `*`.
//! Statements that call `rand()` or `timestamp()` are never cached.
//!
//! # Schema names
//!
//! `uriel-core` indexes and constraints are unnamed. Auto-generated names are
//! `index_<Label>_<key>` and `constraint_<Label>_<key>_<unique|exists>`; names given in
//! `CREATE INDEX name ...` are remembered for the lifetime of the [`Engine`] only.

#![forbid(unsafe_code)]

mod analyze;
mod ctx;
mod error;
mod eval;
mod exec;
mod funcs;
mod matcher;
mod plan;
mod procs;
mod schema;
mod val;
mod write;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use uriel_cache::{CacheMeta, DistCache, LocalCache};
use uriel_core::{CommitInfo, Graph, MemoryBudget, Value};
use uriel_cypher::{Query, QueryBody, QueryMode};

pub use error::QueryError;

/// Whether a statement may modify the graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessMode {
    /// Any clause that writes fails with [`QueryError::ReadOnlyViolation`].
    Read,
    /// Writes are allowed; the statement runs in one atomic transaction.
    Write,
}

/// Counters describing what a statement changed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QueryStats {
    /// Nodes created.
    pub nodes_created: u64,
    /// Nodes deleted.
    pub nodes_deleted: u64,
    /// Relationships created.
    pub rels_created: u64,
    /// Relationships deleted (including those removed by `DETACH DELETE`).
    pub rels_deleted: u64,
    /// Properties assigned.
    pub properties_set: u64,
    /// Labels added (including labels of created nodes).
    pub labels_added: u64,
    /// Labels removed.
    pub labels_removed: u64,
    /// Indexes created.
    pub indexes_added: u64,
    /// Constraints created.
    pub constraints_added: u64,
    /// The result was served from the result cache.
    pub from_cache: bool,
    /// Number of rows in the result (reported by `PROFILE`, set for every statement).
    pub rows_returned: u64,
}

/// Rows returned by a statement.
#[derive(Debug, Clone, PartialEq)]
pub struct QueryResult {
    /// Column names in order.
    pub columns: Vec<String>,
    /// One vector per row, aligned with `columns`.
    pub rows: Vec<Vec<Value>>,
    /// What the statement changed.
    pub stats: QueryStats,
}

impl QueryResult {
    /// An array of `{column: value}` objects, the shape returned by the official
    /// `mcp-neo4j-cypher` server. Values use [`Value::to_json`].
    pub fn to_json_rows(&self) -> serde_json::Value {
        let rows = self
            .rows
            .iter()
            .map(|row| {
                let obj: serde_json::Map<String, serde_json::Value> = self
                    .columns
                    .iter()
                    .zip(row)
                    .map(|(c, v)| (c.clone(), v.to_json()))
                    .collect();
                serde_json::Value::Object(obj)
            })
            .collect();
        serde_json::Value::Array(rows)
    }
}

struct Shared {
    cache: Arc<LocalCache>,
    pending: Mutex<Vec<CommitInfo>>,
    dist_on: AtomicBool,
}

struct Dist {
    cache: DistCache,
    runtime: tokio::runtime::Runtime,
}

/// A graph database handle with Cypher execution and a result cache.
pub struct Engine {
    graph: Graph,
    shared: Arc<Shared>,
    dist: Option<Dist>,
    registry: schema::Registry,
}

impl Engine {
    /// Opens (creating if absent) the database file at `path`.
    pub fn open(path: impl AsRef<Path>, budget: MemoryBudget) -> Result<Engine, QueryError> {
        Ok(Engine::from_graph(Graph::open(path, budget)?))
    }

    pub fn open_read_only(
        path: impl AsRef<Path>,
        budget: MemoryBudget,
    ) -> Result<Engine, QueryError> {
        Ok(Engine::from_graph(Graph::open_read_only(path, budget)?))
    }

    /// Opens a database that lives only in memory.
    pub fn open_in_memory(budget: MemoryBudget) -> Result<Engine, QueryError> {
        Ok(Engine::from_graph(Graph::open_in_memory(budget)?))
    }

    fn from_graph(graph: Graph) -> Engine {
        let shared = Arc::new(Shared {
            cache: Arc::new(LocalCache::new(graph.budget().query_cache())),
            pending: Mutex::new(Vec::new()),
            dist_on: AtomicBool::new(false),
        });
        let s = shared.clone();
        graph.on_commit(Box::new(move |info: &CommitInfo| {
            for t in info.labels.iter().chain(info.rel_types.iter()) {
                s.cache.invalidate_tag_before(t, info.epoch);
            }
            s.cache.invalidate_tag_before("*", info.epoch);
            if s.dist_on.load(Ordering::SeqCst) {
                s.pending
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(info.clone());
            }
        }));
        Engine {
            graph,
            shared,
            dist: None,
            registry: schema::Registry::default(),
        }
    }

    /// Additionally stores and looks up read results in a distributed cache. The cache
    /// calls are bridged with a small current-thread runtime owned by the engine.
    pub fn with_dist_cache(mut self, cache: DistCache) -> Engine {
        if let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            self.dist = Some(Dist { cache, runtime });
            self.shared.dist_on.store(true, Ordering::SeqCst);
        }
        self
    }

    pub fn cache_capacity(&self) -> usize {
        self.shared.cache.max_bytes()
    }

    /// The underlying storage handle.
    pub fn graph(&self) -> &Graph {
        &self.graph
    }

    /// Labels, relationship types, properties, indexes and constraints as JSON
    /// (the serialized [`uriel_core::Schema`]).
    pub fn schema_json(&self) -> serde_json::Value {
        match self.graph.schema() {
            Ok(s) => serde_json::to_value(s).unwrap_or(serde_json::Value::Null),
            Err(e) => serde_json::json!({ "error": e.to_string() }),
        }
    }

    fn flush_dist(&self) {
        let Some(d) = &self.dist else { return };
        let pending: Vec<CommitInfo> = std::mem::take(
            &mut *self
                .shared
                .pending
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        );
        for info in pending {
            let mut tags: Vec<String> = info
                .labels
                .iter()
                .chain(info.rel_types.iter())
                .cloned()
                .collect();
            tags.push("*".into());
            d.runtime.block_on(d.cache.invalidate(&tags, info.epoch));
        }
    }

    /// Parses and runs one statement with `params` bound to `$name` parameters.
    pub fn execute(
        &self,
        query: &str,
        params: &BTreeMap<String, Value>,
        mode: AccessMode,
    ) -> Result<QueryResult, QueryError> {
        let ast = uriel_cypher::parse(query)?;
        self.flush_dist();
        let writes = analyze::write_kind(&ast);
        if mode == AccessMode::Read {
            if let Some(w) = writes {
                return Err(QueryError::ReadOnlyViolation(w.to_string()));
            }
        }
        if ast.mode == Some(QueryMode::Explain) {
            return self.explain(&ast);
        }
        let mut result = match &ast.body {
            QueryBody::Schema(cmd) => {
                if writes.is_some() {
                    schema::run_write(&self.graph, &self.registry, cmd)?
                } else {
                    schema::run_show(&self.graph, &self.registry, cmd)?
                }
            }
            QueryBody::Regular(q) => {
                if writes.is_some() {
                    self.run_write(q, params)?
                } else {
                    let cacheable = mode == AccessMode::Read && ast.mode.is_none();
                    self.run_read(query, q, params, cacheable)?
                }
            }
        };
        result.stats.rows_returned = result.rows.len() as u64;
        Ok(result)
    }

    fn explain(&self, ast: &Query) -> Result<QueryResult, QueryError> {
        let read = self.graph.begin_read()?;
        let text = plan::explain(ast, &read);
        Ok(QueryResult {
            columns: vec!["plan".into()],
            rows: vec![vec![Value::String(text)]],
            stats: QueryStats::default(),
        })
    }

    fn run_write(
        &self,
        q: &uriel_cypher::RegularQuery,
        params: &BTreeMap<String, Value>,
    ) -> Result<QueryResult, QueryError> {
        let tx = self.graph.begin_write()?;
        let read = self.graph.begin_read()?;
        let mut ctx = ctx::Ctx::new(read, Some(tx), params);
        // on error `ctx` (and the write transaction inside it) is dropped: rollback
        let (columns, rows) = exec::run_top(&mut ctx, q)?;
        let stats = ctx.stats.clone();
        let tx = ctx.tx.take();
        drop(ctx);
        if let Some(tx) = tx {
            tx.commit()?;
        }
        Ok(QueryResult {
            columns,
            rows,
            stats,
        })
    }

    fn run_read(
        &self,
        text: &str,
        q: &uriel_cypher::RegularQuery,
        params: &BTreeMap<String, Value>,
        cacheable: bool,
    ) -> Result<QueryResult, QueryError> {
        let key = cacheable.then(|| cache_key(text, params));
        if let Some((k, fingerprint)) = &key {
            if let Some(hit) = self.cache_get(k, fingerprint) {
                return Ok(hit);
            }
        }
        let read = self.graph.begin_read()?;
        let epoch = read.epoch();
        let mut ctx = ctx::Ctx::new(read, None, params);
        let (columns, rows) = exec::run_top(&mut ctx, q)?;
        let result = QueryResult {
            columns,
            rows,
            stats: ctx.stats.clone(),
        };
        if let Some((k, fingerprint)) = &key {
            if !ctx.nondet {
                let tags: Vec<String> = if ctx.star {
                    vec!["*".into()]
                } else {
                    ctx.tags.iter().cloned().collect()
                };
                self.cache_put(
                    k,
                    fingerprint,
                    &result,
                    CacheMeta::new(epoch).with_tags(tags),
                );
            }
        }
        Ok(result)
    }

    fn cache_get(&self, key: &str, fingerprint: &str) -> Option<QueryResult> {
        let mut bytes = self.shared.cache.get(key);
        if bytes.is_none() {
            if let Some(d) = &self.dist {
                bytes = d.runtime.block_on(d.cache.get(key));
            }
        }
        let doc: serde_json::Value = serde_json::from_slice(&bytes?).ok()?;
        if doc.get("fp")?.as_str()? != fingerprint {
            return None;
        }
        let columns = doc
            .get("columns")?
            .as_array()?
            .iter()
            .map(|c| c.as_str().map(String::from))
            .collect::<Option<Vec<_>>>()?;
        let mut rows = Vec::new();
        for row in doc.get("rows")?.as_array()? {
            let cells = row
                .as_array()?
                .iter()
                .map(|c| Value::from_json(c).ok())
                .collect::<Option<Vec<_>>>()?;
            rows.push(cells);
        }
        Some(QueryResult {
            columns,
            rows,
            stats: QueryStats {
                from_cache: true,
                ..QueryStats::default()
            },
        })
    }

    fn cache_put(&self, key: &str, fingerprint: &str, r: &QueryResult, meta: CacheMeta) {
        let doc = serde_json::json!({
            "fp": fingerprint,
            "columns": r.columns,
            "rows": r.rows.iter().map(|row| row.iter().map(Value::to_json).collect::<Vec<_>>()).collect::<Vec<_>>(),
        });
        let Ok(bytes) = serde_json::to_vec(&doc) else {
            return;
        };
        let bytes: Arc<[u8]> = Arc::from(bytes);
        self.shared.cache.insert(key, bytes.clone(), meta.clone());
        if let Some(d) = &self.dist {
            d.runtime.block_on(d.cache.put(key, bytes, meta));
        }
    }
}

fn cache_key(text: &str, params: &BTreeMap<String, Value>) -> (String, String) {
    let ps: serde_json::Map<String, serde_json::Value> = params
        .iter()
        .map(|(k, v)| (k.clone(), v.to_json()))
        .collect();
    let fingerprint = format!("{text}\u{0}{}", serde_json::Value::Object(ps));
    let h = uriel_cache::hash::hash64(fingerprint.as_bytes(), 0x5ea1);
    (format!("q:{h:016x}"), fingerprint)
}
