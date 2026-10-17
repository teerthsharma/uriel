//! The [`Graph`] handle: opening, transactions, epoch and commit notifications.

use std::collections::BTreeSet;
use std::fs::File;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, RwLock};

use redb::backends::{FileBackend, InMemoryBackend};
use redb::{Builder, Database, ReadableDatabase, ReadableTable, StorageBackend};

use crate::budget::MemoryBudget;
use crate::catalog::{ConstraintDef, ConstraintKind, IndexDef, Schema};
use crate::checksum::crc32c;
use crate::error::{Error, Result};
use crate::overlay::Overlay;
use crate::placement::Placement;
use crate::read::ReadTx;
use crate::tables::{
    meta, CONSTRAINTS, COUNTS, INDEXES, IN_ADJ, LABEL_IDX, META, NODES, NODE_SUMS, OUT_ADJ,
    PROP_IDX, RELS, REL_SUMS, TYPE_IDX,
};
use crate::tier::{TierState, TierStats};
use crate::write::WriteTx;

pub(crate) const FORMAT_VERSION: u64 = 3;
pub(crate) const SUMLESS_FORMAT_VERSION: u64 = 2;
pub(crate) const LEGACY_FORMAT_VERSION: u64 = 1;

#[derive(Debug, Clone)]
pub struct Options {
    pub budget: MemoryBudget,
    pub read_only: bool,
    pub cache_bytes: usize,
    pub hot_entries: usize,
    pub hot_bytes: usize,
    pub warm_bytes: usize,
    pub cold_threshold_bytes: usize,
    pub compress_cold: bool,
    pub admission_gate: bool,
    pub placement: Placement,
    pub region_ids: usize,
    pub plan_every: usize,
    pub communities: Option<Arc<Vec<u32>>>,
    pub record_events: bool,
}

impl Options {
    pub fn new(budget: MemoryBudget) -> Options {
        Options {
            budget,
            read_only: false,
            cache_bytes: budget.redb_cache(),
            hot_entries: budget.hot_entries(),
            hot_bytes: budget.hot(),
            warm_bytes: budget.warm(),
            cold_threshold_bytes: 256,
            compress_cold: true,
            admission_gate: true,
            placement: Placement::Local,
            region_ids: 16,
            plan_every: 128,
            communities: None,
            record_events: false,
        }
    }
}

/// What a committed write transaction changed, delivered to commit subscribers.
///
/// A consumer that caches query results can drop every entry whose query mentions one of
/// `labels` or `rel_types`. Two things are deliberately *not* reported and must be handled
/// by the consumer: a query with an untyped relationship pattern depends on all
/// relationship types, and a query over unlabeled nodes depends on all nodes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitInfo {
    /// Epoch assigned to this commit (the graph's [`Graph::epoch`] right after it).
    pub epoch: u64,
    /// Labels of every node whose existence, labels or properties changed, plus labels
    /// whose indexes or constraints changed.
    pub labels: BTreeSet<String>,
    /// Types of every relationship created, deleted or changed (including those removed by
    /// `detach_delete_node`).
    pub rel_types: BTreeSet<String>,
}

/// Labels and relationship types modified so far by a [`WriteTx`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Touched {
    /// Modified labels.
    pub labels: BTreeSet<String>,
    /// Modified relationship types.
    pub rel_types: BTreeSet<String>,
}

/// Handle returned by [`Graph::on_commit`], used to [`Graph::unsubscribe`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SubscriptionId(pub u64);

type Callback = Arc<dyn Fn(&CommitInfo) + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mode {
    Full,
    ReadOnly,
    CacheOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModeCause {
    WriteFailed,
    ReadFailed,
    ScrubCorrupt,
    Repaired,
    Recovered,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModeEvent {
    pub seq: u64,
    pub epoch: u64,
    pub from: Mode,
    pub to: Mode,
    pub cause: ModeCause,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RecordId {
    pub table: &'static str,
    pub id: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Integrity {
    Clean,
    Repaired,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScrubReport {
    pub records_checked: usize,
    pub unchecked: usize,
    pub corrupt: Vec<RecordId>,
    pub repaired: usize,
    pub integrity: Integrity,
    pub errors: Vec<String>,
}

impl ScrubReport {
    pub fn is_clean(&self) -> bool {
        self.corrupt.is_empty()
            && self.errors.is_empty()
            && matches!(self.integrity, Integrity::Clean | Integrity::Skipped)
    }
}

struct ModeState {
    mode: Mode,
    corrupt: bool,
    log: Vec<ModeEvent>,
}

type Reopen = Box<dyn Fn() -> Result<Database> + Send + Sync>;

pub(crate) struct Inner {
    db: RwLock<Option<Arc<Database>>>,
    reopen: Option<Reopen>,
    degrade: Option<Reopen>,
    modes: Mutex<ModeState>,
    pub(crate) tier: TierState,
    budget: MemoryBudget,
    read_only: bool,
    epoch: AtomicU64,
    writer_busy: Mutex<bool>,
    writer_cv: Condvar,
    subs: RwLock<Vec<(SubscriptionId, Callback)>>,
    next_sub: AtomicU64,
}

fn open_overlay(path: &Path, cache: usize) -> Result<Database> {
    guard(|| {
        let file = File::open(path).map_err(|e| Error::from(redb::StorageError::Io(e)))?;
        let base = FileBackend::new(file)?;
        let overlay = Overlay::new(base).map_err(|e| Error::from(redb::StorageError::Io(e)))?;
        Builder::new()
            .set_cache_size(cache)
            .create_with_backend(overlay)
            .map_err(Error::from)
    })
}

fn guard<T>(f: impl FnOnce() -> Result<T>) -> Result<T> {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(r) => r,
        Err(_) => Err(Error::Corrupt(
            "storage engine aborted on a corrupt database file".to_string(),
        )),
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Proof that the caller is the one active writer. Dropping it admits the next writer.
pub(crate) struct WriterGate(Arc<Inner>);

impl Drop for WriterGate {
    fn drop(&mut self) {
        *lock(&self.0.writer_busy) = false;
        self.0.writer_cv.notify_one();
    }
}

impl Inner {
    pub(crate) fn database(&self) -> Result<Arc<Database>> {
        self.db
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .ok_or(Error::Unavailable)
    }

    pub(crate) fn current_epoch(&self) -> u64 {
        self.epoch.load(Ordering::SeqCst)
    }

    fn modes(&self) -> MutexGuard<'_, ModeState> {
        lock(&self.modes)
    }

    pub(crate) fn mode(&self) -> Mode {
        self.modes().mode
    }

    pub(crate) fn is_cache_only(&self) -> bool {
        self.mode() == Mode::CacheOnly
    }

    pub(crate) fn disk_gate(&self) -> Result<()> {
        if self.is_cache_only() {
            Err(Error::Unavailable)
        } else {
            Ok(())
        }
    }

    fn transition(&self, to: Mode, cause: ModeCause) -> bool {
        let epoch = self.current_epoch();
        let mut st = self.modes();
        if st.mode == to {
            return false;
        }
        let ev = ModeEvent {
            seq: st.log.len() as u64,
            epoch,
            from: st.mode,
            to,
            cause,
        };
        st.mode = to;
        st.log.push(ev);
        true
    }

    pub(crate) fn write_failed(&self) {
        let moved = {
            let st = self.modes();
            st.mode == Mode::Full
        };
        if moved {
            self.transition(Mode::ReadOnly, ModeCause::WriteFailed);
            self.reopen_or_degrade();
        }
    }

    fn reopen_or_degrade(&self) {
        if self.refresh_db(false).is_err() {
            let _ = self.install_from(self.degrade.as_ref(), false);
        }
    }

    pub(crate) fn read_failed(&self, e: Error) -> Error {
        if !e.is_io() {
            return e;
        }
        if self.mode() != Mode::Full {
            self.transition(Mode::CacheOnly, ModeCause::ReadFailed);
            return Error::Unavailable;
        }
        e
    }

    fn clear_corrupt(&self, cause: ModeCause) {
        let restore = {
            let mut st = self.modes();
            let was = st.corrupt;
            st.corrupt = false;
            was && st.mode == Mode::ReadOnly
                && st.log.last().is_some_and(|e| e.cause == ModeCause::ScrubCorrupt)
        };
        if restore {
            self.transition(Mode::Full, cause);
        }
    }

    fn mark_corrupt(&self) {
        self.modes().corrupt = true;
        self.transition(Mode::ReadOnly, ModeCause::ScrubCorrupt);
    }

    pub(crate) fn refresh_db(&self, drop_first: bool) -> Result<()> {
        self.install_from(self.reopen.as_ref(), drop_first)
    }

    fn install_from(&self, open: Option<&Reopen>, drop_first: bool) -> Result<()> {
        let Some(reopen) = open else {
            return Err(Error::Unavailable);
        };
        let mut slot = self.db.write().unwrap_or_else(PoisonError::into_inner);
        if drop_first {
            drop(slot.take());
        }
        let fresh = Arc::new(reopen()?);
        let epoch = {
            let r = fresh.begin_read()?;
            let m = r.open_table(META)?;
            let e = m.get(meta::EPOCH)?.map(|g| g.value()).unwrap_or(0);
            e
        };
        let previous = self.current_epoch();
        *slot = Some(fresh);
        drop(slot);
        self.epoch.store(epoch, Ordering::SeqCst);
        if epoch == previous {
            self.tier.abort_commit();
        } else {
            self.tier.reset(epoch);
        }
        Ok(())
    }

    fn integrity_check(&self) -> Integrity {
        let mut slot = self.db.write().unwrap_or_else(PoisonError::into_inner);
        let Some(mut arc) = slot.take() else {
            return Integrity::Skipped;
        };
        let out = match Arc::get_mut(&mut arc) {
            None => Integrity::Skipped,
            Some(db) => match catch_unwind(AssertUnwindSafe(|| db.check_integrity())) {
                Ok(Ok(true)) => Integrity::Clean,
                Ok(Ok(false)) => Integrity::Repaired,
                Ok(Err(_)) => Integrity::Failed,
                Err(_) => {
                    drop(slot);
                    drop(catch_unwind(AssertUnwindSafe(move || drop(arc))));
                    return Integrity::Failed;
                }
            },
        };
        *slot = Some(arc);
        out
    }

    fn acquire_writer(self: &Arc<Self>) -> WriterGate {
        let mut busy = lock(&self.writer_busy);
        while *busy {
            busy = self
                .writer_cv
                .wait(busy)
                .unwrap_or_else(PoisonError::into_inner);
        }
        *busy = true;
        drop(busy);
        WriterGate(self.clone())
    }

    /// Publishes a commit: advances the epoch and notifies subscribers. Called while the
    /// writer gate is still held, so notifications are totally ordered by epoch.
    pub(crate) fn publish(&self, info: &CommitInfo) {
        self.epoch.store(info.epoch, Ordering::SeqCst);
        let callbacks: Vec<Callback> = self
            .subs
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|(_, cb)| cb.clone())
            .collect();
        for cb in callbacks {
            cb(info);
        }
    }
}

/// An open property graph. Cheap to clone; clones share the same database.
///
/// `Graph` is `Send + Sync`. Any number of [`ReadTx`] can run concurrently with at most
/// one [`WriteTx`]; a second `begin_write` blocks until the first transaction commits or is
/// dropped.
#[derive(Clone)]
pub struct Graph {
    pub(crate) inner: Arc<Inner>,
}

impl std::fmt::Debug for Graph {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Graph")
            .field("epoch", &self.epoch())
            .finish_non_exhaustive()
    }
}

impl Graph {
    pub fn open(path: impl AsRef<Path>, budget: MemoryBudget) -> Result<Graph> {
        Graph::open_with(path, &Options::new(budget))
    }

    /// Opens (creating if absent) the database at `path`.
    ///
    /// Fails with [`Error::Storage`] if the file is already open in this or another
    /// process, and with [`Error::UnsupportedFormat`] if it was written by an incompatible
    /// version.
    pub fn open_read_only(path: impl AsRef<Path>, budget: MemoryBudget) -> Result<Graph> {
        let options = Options {
            read_only: true,
            ..Options::new(budget)
        };
        Graph::open_with(path, &options)
    }

    pub fn open_with(path: impl AsRef<Path>, options: &Options) -> Result<Graph> {
        let path: PathBuf = path.as_ref().to_path_buf();
        let cache = options.cache_bytes;
        let degrade_path = path.clone();
        let reopen: Reopen = if options.read_only {
            let ro_path = path.clone();
            Box::new(move || open_overlay(&ro_path, cache))
        } else {
            Box::new(move || {
                guard(|| {
                    Builder::new()
                        .set_cache_size(cache)
                        .create(&path)
                        .map_err(Error::from)
                })
            })
        };
        let degrade: Reopen = Box::new(move || open_overlay(&degrade_path, cache));
        let db = reopen()?;
        Graph::init(db, options, Some(reopen), Some(degrade))
    }

    pub fn open_with_backend<B: StorageBackend>(
        factory: impl Fn() -> B + Send + Sync + 'static,
        options: &Options,
    ) -> Result<Graph> {
        let cache = options.cache_bytes;
        let factory = Arc::new(factory);
        let degrade_factory = Arc::clone(&factory);
        let reopen: Reopen = Box::new(move || {
            guard(|| {
                Builder::new()
                    .set_cache_size(cache)
                    .create_with_backend(factory())
                    .map_err(Error::from)
            })
        });
        let degrade: Reopen = Box::new(move || {
            guard(|| {
                let overlay = Overlay::new(degrade_factory())
                    .map_err(|e| Error::from(redb::StorageError::Io(e)))?;
                Builder::new()
                    .set_cache_size(cache)
                    .create_with_backend(overlay)
                    .map_err(Error::from)
            })
        });
        let db = reopen()?;
        Graph::init(db, options, Some(reopen), Some(degrade))
    }

    /// Opens a fresh graph that lives only in memory. Everything is lost on drop.
    pub fn open_in_memory(budget: MemoryBudget) -> Result<Graph> {
        let options = Options::new(budget);
        let db = Builder::new()
            .set_cache_size(options.cache_bytes)
            .create_with_backend(InMemoryBackend::new())?;
        Graph::init(db, &options, None, None)
    }

    fn init(
        db: Database,
        options: &Options,
        reopen: Option<Reopen>,
        degrade: Option<Reopen>,
    ) -> Result<Graph> {
        guard(move || Graph::init_unguarded(db, options, reopen, degrade))
    }

    fn init_unguarded(
        db: Database,
        options: &Options,
        reopen: Option<Reopen>,
        degrade: Option<Reopen>,
    ) -> Result<Graph> {
        // create every table up front so read transactions never see a missing table,
        // and stamp / verify the format version
        let txn = db.begin_write()?;
        {
            txn.open_table(NODES)?;
            txn.open_table(RELS)?;
            txn.open_table(OUT_ADJ)?;
            txn.open_table(IN_ADJ)?;
            txn.open_table(LABEL_IDX)?;
            txn.open_table(TYPE_IDX)?;
            txn.open_table(PROP_IDX)?;
            txn.open_table(COUNTS)?;
            txn.open_table(INDEXES)?;
            txn.open_table(CONSTRAINTS)?;
            let mut node_sums = txn.open_table(NODE_SUMS)?;
            let mut rel_sums = txn.open_table(REL_SUMS)?;
            let mut m = txn.open_table(META)?;
            let found = m.get(meta::FORMAT)?.map(|g| g.value());
            match found {
                None => {
                    m.insert(meta::FORMAT, FORMAT_VERSION)?;
                }
                Some(v) if v == FORMAT_VERSION => {}
                Some(v) if v == LEGACY_FORMAT_VERSION || v == SUMLESS_FORMAT_VERSION => {
                    node_sums.retain(|_, _| false)?;
                    rel_sums.retain(|_, _| false)?;
                    m.insert(meta::FORMAT, FORMAT_VERSION)?;
                }
                Some(found) => {
                    return Err(Error::UnsupportedFormat {
                        found,
                        expected: FORMAT_VERSION,
                    })
                }
            }
        }
        txn.commit()?;

        let epoch = {
            let r = db.begin_read()?;
            let m = r.open_table(META)?;
            let e = m.get(meta::EPOCH)?.map(|g| g.value()).unwrap_or(0);
            e
        };
        Ok(Graph {
            inner: Arc::new(Inner {
                db: RwLock::new(Some(Arc::new(db))),
                reopen,
                degrade,
                modes: Mutex::new(ModeState {
                    mode: if options.read_only {
                        Mode::ReadOnly
                    } else {
                        Mode::Full
                    },
                    corrupt: false,
                    log: Vec::new(),
                }),
                tier: TierState::new(options, epoch),
                budget: options.budget,
                read_only: options.read_only,
                epoch: AtomicU64::new(epoch),
                writer_busy: Mutex::new(false),
                writer_cv: Condvar::new(),
                subs: RwLock::new(Vec::new()),
                next_sub: AtomicU64::new(1),
            }),
        })
    }

    #[cfg(test)]
    pub(crate) fn open_in_memory_with(options: &Options) -> Result<Graph> {
        let db = Builder::new()
            .set_cache_size(8 * 1024 * 1024)
            .create_with_backend(InMemoryBackend::new())?;
        Graph::init(db, options, None, None)
    }

    #[cfg(test)]
    pub(crate) fn db(&self) -> Arc<Database> {
        self.inner.database().expect("database")
    }

    pub fn budget(&self) -> MemoryBudget {
        self.inner.budget
    }

    pub fn is_read_only(&self) -> bool {
        self.inner.read_only
    }

    pub fn placement_log(&self) -> Vec<uriel_place::Event> {
        self.inner.tier.placement_log()
    }

    pub fn placement_hot_regions(&self) -> Vec<u32> {
        self.inner.tier.hot_regions()
    }

    pub fn tier_stats(&self) -> Result<TierStats> {
        if self.inner.is_cache_only() {
            return Ok(self.inner.tier.counters(0, 0));
        }
        let txn = self.inner.database()?.begin_read()?;
        let (mut stored, mut logical) = (0u64, 0u64);
        for def in [NODES, RELS] {
            for e in txn.open_table(def)?.iter()? {
                let (_, v) = e?;
                let bytes = v.value();
                stored += bytes.len() as u64;
                logical += uriel_tier::logical_len(bytes)
                    .map_err(|e| Error::Corrupt(e.to_string()))? as u64;
            }
        }
        Ok(self.inner.tier.counters(stored, logical))
    }

    /// Starts a read transaction: a consistent snapshot that is unaffected by later
    /// commits. Never blocks on writers.
    pub fn begin_read(&self) -> Result<ReadTx> {
        ReadTx::begin(&self.inner)
    }

    /// Starts the write transaction, waiting until any other write transaction has
    /// committed or been dropped.
    ///
    /// Dropping the returned [`WriteTx`] without [`WriteTx::commit`] rolls everything back.
    pub fn begin_write(&self) -> Result<WriteTx> {
        if self.inner.read_only || self.inner.mode() != Mode::Full {
            return Err(Error::ReadOnly);
        }
        let gate = self.inner.acquire_writer();
        if self.inner.mode() != Mode::Full {
            return Err(Error::ReadOnly);
        }
        match WriteTx::begin(self.inner.clone(), gate) {
            Err(e) if e.is_io() => {
                self.inner.write_failed();
                Err(e)
            }
            other => other,
        }
    }

    /// Number of write commits so far; persisted. A commit that changed nothing does not
    /// advance it. Strictly increases by one per effective commit.
    pub fn epoch(&self) -> u64 {
        self.inner.epoch.load(Ordering::SeqCst)
    }

    /// Registers `callback` to run after every effective write commit, with the new epoch
    /// and the labels / relationship types it touched.
    /// The callback runs on the committing thread after the commit is durable and visible
    /// to new readers, while later writers are still held back, so callbacks observe
    /// commits in strictly increasing epoch order. It must be quick and must not start a
    /// write transaction (that would deadlock). Reading, subscribing and unsubscribing
    /// from inside a callback are fine.
    pub fn on_commit(&self, callback: Box<dyn Fn(&CommitInfo) + Send + Sync>) -> SubscriptionId {
        let id = SubscriptionId(self.inner.next_sub.fetch_add(1, Ordering::SeqCst));
        self.inner
            .subs
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .push((id, Arc::from(callback)));
        id
    }

    /// Alias of [`Graph::on_commit`].
    pub fn subscribe_commits(
        &self,
        callback: Box<dyn Fn(&CommitInfo) + Send + Sync>,
    ) -> SubscriptionId {
        self.on_commit(callback)
    }

    /// Removes a subscription. Returns `false` if it was already removed.
    pub fn unsubscribe(&self, id: SubscriptionId) -> bool {
        let mut subs = self
            .inner
            .subs
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        let before = subs.len();
        subs.retain(|(s, _)| *s != id);
        subs.len() != before
    }

    fn in_write<T>(&self, f: impl FnOnce(&mut WriteTx) -> Result<T>) -> Result<T> {
        let mut tx = self.begin_write()?;
        let out = f(&mut tx)?;
        tx.commit()?;
        Ok(out)
    }

    /// Creates a property index in its own transaction. See [`WriteTx::create_index`].
    pub fn create_index(&self, label: &str, key: &str) -> Result<()> {
        self.in_write(|tx| tx.create_index(label, key))
    }

    /// Drops a property index in its own transaction. See [`WriteTx::drop_index`].
    pub fn drop_index(&self, label: &str, key: &str) -> Result<()> {
        self.in_write(|tx| tx.drop_index(label, key))
    }

    /// Lists all indexes, sorted by label then key.
    pub fn list_indexes(&self) -> Result<Vec<IndexDef>> {
        self.begin_read()?.list_indexes()
    }

    /// Creates a uniqueness constraint in its own transaction. See
    /// [`WriteTx::create_unique_constraint`].
    pub fn create_unique_constraint(&self, label: &str, key: &str) -> Result<()> {
        self.in_write(|tx| tx.create_unique_constraint(label, key))
    }

    /// Creates an existence constraint in its own transaction. See
    /// [`WriteTx::create_exists_constraint`].
    pub fn create_exists_constraint(&self, label: &str, key: &str) -> Result<()> {
        self.in_write(|tx| tx.create_exists_constraint(label, key))
    }

    /// Drops a constraint in its own transaction. See [`WriteTx::drop_constraint`].
    pub fn drop_constraint(&self, kind: ConstraintKind, label: &str, key: &str) -> Result<()> {
        self.in_write(|tx| tx.drop_constraint(kind, label, key))
    }

    /// Lists all constraints, sorted by label, key, then kind (unique before exists).
    pub fn list_constraints(&self) -> Result<Vec<ConstraintDef>> {
        self.begin_read()?.list_constraints()
    }

    /// Schema of the current snapshot. See [`ReadTx::schema`].
    pub fn schema(&self) -> Result<Schema> {
        self.begin_read()?.schema()
    }

    pub fn mode(&self) -> Mode {
        self.inner.mode()
    }

    pub fn mode_log(&self) -> Vec<ModeEvent> {
        self.inner.modes().log.clone()
    }

    pub fn scrub(&self) -> Result<ScrubReport> {
        let _gate = self.inner.acquire_writer();
        let db = self.inner.database()?;
        let mut report = ScrubReport {
            records_checked: 0,
            unchecked: 0,
            corrupt: Vec::new(),
            repaired: 0,
            integrity: Integrity::Skipped,
            errors: Vec::new(),
        };
        let scanned = catch_unwind(AssertUnwindSafe(|| scan(&db, &mut report)));
        match scanned {
            Ok(Ok(())) => {}
            Ok(Err(e)) => report.errors.push(e.to_string()),
            Err(_) => {
                drop(catch_unwind(AssertUnwindSafe(move || drop(db))));
                *self.inner.db.write().unwrap_or_else(PoisonError::into_inner) = None;
                self.inner.mark_corrupt();
                return Err(Error::Corrupt(
                    "storage engine aborted while scanning a corrupt database file".to_string(),
                ));
            }
        }
        drop(db);
        report.integrity = self.inner.integrity_check();
        report.corrupt.sort();
        report.corrupt.dedup();
        if !report.errors.is_empty()
            || !report.corrupt.is_empty()
            || report.integrity == Integrity::Failed
        {
            self.inner.mark_corrupt();
        }
        if matches!(report.integrity, Integrity::Repaired | Integrity::Failed) {
            let _ = self.inner.refresh_db(false);
            self.inner.tier.reset(self.inner.current_epoch());
        }
        Ok(report)
    }

    pub fn repair_from(&self, replica: &Graph) -> Result<ScrubReport> {
        if self.inner.read_only {
            return Err(Error::ReadOnly);
        }
        let before = self.scrub()?;
        if before.corrupt.is_empty() {
            return Ok(before);
        }
        {
            let _gate = self.inner.acquire_writer();
            let rdb = replica.inner.database()?;
            let rtx = rdb.begin_read()?;
            let db = self.inner.database()?;
            let w = db.begin_write()?;
            for rid in &before.corrupt {
                let (rec, sums) = if rid.table == "nodes" {
                    (NODES, NODE_SUMS)
                } else {
                    (RELS, REL_SUMS)
                };
                let src = rtx.open_table(rec)?;
                let src_sums = rtx.open_table(sums)?;
                let Some(g) = src.get(rid.id)? else {
                    continue;
                };
                let bytes = g.value();
                let sum = crc32c(bytes);
                if let Some(s) = src_sums.get(rid.id)? {
                    if s.value() != sum {
                        continue;
                    }
                }
                w.open_table(rec)?.insert(rid.id, bytes)?;
                w.open_table(sums)?.insert(rid.id, sum)?;
            }
            w.commit()?;
            self.inner.tier.reset(self.inner.current_epoch());
        }
        let mut after = self.scrub()?;
        after.repaired = before
            .corrupt
            .iter()
            .filter(|r| !after.corrupt.contains(r))
            .count();
        if after.is_clean() {
            self.inner.clear_corrupt(ModeCause::Repaired);
        }
        Ok(after)
    }

    pub fn try_recover(&self) -> Result<Mode> {
        let mode = self.inner.mode();
        if mode == Mode::Full || self.inner.read_only {
            return Ok(mode);
        }
        let reopened = match self.inner.refresh_db(false) {
            Ok(()) => true,
            Err(e) if e.is_already_open() => self.inner.refresh_db(true).is_ok(),
            Err(_) => false,
        };
        if !reopened {
            return Ok(self.inner.mode());
        }
        if self.inner.is_cache_only() {
            self.inner.transition(Mode::ReadOnly, ModeCause::Recovered);
        }
        if self.inner.modes().corrupt {
            match self.scrub() {
                Ok(report) if report.is_clean() => {}
                _ => return Ok(self.inner.mode()),
            }
            self.inner.modes().corrupt = false;
        }
        let probe = self.inner.database().and_then(|db| {
            let w = db.begin_write()?;
            w.open_table(META)?.insert(meta::FORMAT, FORMAT_VERSION)?;
            w.commit()?;
            Ok(())
        });
        match probe {
            Ok(()) => {
                self.inner.transition(Mode::Full, ModeCause::Recovered);
            }
            Err(_) => self.inner.reopen_or_degrade(),
        }
        Ok(self.inner.mode())
    }
}

fn walk<K: redb::Key + 'static, V: redb::Value + 'static>(
    r: &redb::ReadTransaction,
    def: redb::TableDefinition<K, V>,
    name: &str,
    errors: &mut Vec<String>,
) {
    let t = match r.open_table(def) {
        Ok(t) => t,
        Err(e) => {
            errors.push(format!("table {name}: {e}"));
            return;
        }
    };
    match t.iter() {
        Err(e) => errors.push(format!("table {name}: {e}")),
        Ok(it) => {
            for e in it {
                if let Err(err) = e {
                    errors.push(format!("table {name}: {err}"));
                    break;
                }
            }
        }
    }
}

fn scan_records(
    r: &redb::ReadTransaction,
    rec: redb::TableDefinition<u64, &'static [u8]>,
    sums: redb::TableDefinition<u64, u32>,
    name: &'static str,
    report: &mut ScrubReport,
) -> Result<()> {
    let t = r.open_table(rec)?;
    let s = r.open_table(sums)?;
    let mut seen = BTreeSet::new();
    for e in t.iter()? {
        let (k, v) = match e {
            Ok(kv) => kv,
            Err(err) => {
                report.errors.push(format!("table {name}: {err}"));
                break;
            }
        };
        let id = k.value();
        let bytes = v.value();
        report.records_checked += 1;
        seen.insert(id);
        match s.get(id) {
            Ok(Some(g)) => {
                if g.value() != crc32c(bytes) {
                    report.corrupt.push(RecordId { table: name, id });
                }
            }
            Ok(None) => {
                report.unchecked += 1;
                let decoded = if name == "nodes" {
                    crate::codec::decode_node(crate::ids::NodeId(id), bytes).is_ok()
                } else {
                    crate::codec::decode_rel(crate::ids::RelId(id), bytes).is_ok()
                };
                if !decoded {
                    report.corrupt.push(RecordId { table: name, id });
                }
            }
            Err(err) => report.errors.push(format!("table {name}: {err}")),
        }
    }
    for e in s.iter()? {
        match e {
            Ok((k, _)) => {
                if !seen.contains(&k.value()) {
                    report.corrupt.push(RecordId {
                        table: name,
                        id: k.value(),
                    });
                }
            }
            Err(err) => {
                report.errors.push(format!("table {name}_sums: {err}"));
                break;
            }
        }
    }
    Ok(())
}

fn scan(db: &Database, report: &mut ScrubReport) -> Result<()> {
    let r = db.begin_read()?;
    scan_records(&r, NODES, NODE_SUMS, "nodes", report)?;
    scan_records(&r, RELS, REL_SUMS, "rels", report)?;
    let mut errors = Vec::new();
    walk(&r, OUT_ADJ, "adj_out", &mut errors);
    walk(&r, IN_ADJ, "adj_in", &mut errors);
    walk(&r, LABEL_IDX, "label_idx", &mut errors);
    walk(&r, TYPE_IDX, "type_idx", &mut errors);
    walk(&r, PROP_IDX, "prop_idx", &mut errors);
    walk(&r, COUNTS, "counts", &mut errors);
    walk(&r, INDEXES, "indexes", &mut errors);
    walk(&r, CONSTRAINTS, "constraints", &mut errors);
    walk(&r, META, "meta", &mut errors);
    report.errors.extend(errors);
    Ok(())
}
