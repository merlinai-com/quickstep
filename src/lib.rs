//! Quickstep - A modern, concurrent embedded key-value store implementing the Bf-tree data structure.
//!
//! This implementation is based on the original work by [Raphael Darley](https://github.com/RaphaelDarley/quickstep).
//! The core architecture and implementation are led by Raphael Darley.
//!
//! For more information, see the [README](https://github.com/merlinai-com/quickstep) and
//! [design documentation](../design/).

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    env,
    path::{Path, PathBuf},
    ptr,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};

use crate::{
    btree::{BPTree, ChildPointer, DebugLeafParent, OpType, WriteLockBundle},
    buffer::{MiniPageBuffer, MiniPageIndex},
    error::QSError,
    io_engine::IoEngine,
    lock_manager::{LockManager, WriteGuardWrapper},
    map_table::{MapTable, PageId},
    page_op::{LeafMergePlan, LeafSplitOutcome, LeafSplitPlan, TryPutResult},
    types::{NodeMeta, NodeRef, NodeSize},
    wal::{WalEntryKind, WalManager, WalOp, WalRecord, WalTxnMarker, TXN_META_PAGE_ID},
};

pub mod btree;
pub mod buffer;
pub mod debug;
pub mod error;
pub mod io_engine;
pub mod lock_manager;
pub mod map_table;
pub mod node;
pub mod page_op;
pub mod rand;
pub mod types;
pub mod utils;
pub mod wal;

pub const SPIN_RETRIES: usize = 2 ^ 12;

const _: () = assert!(std::mem::size_of::<usize>() == std::mem::size_of::<u64>());

/// Represents the overall Bf-tree
pub struct QuickStep {
    /// The inner nodes of the Tree, stores no values, but references to leaves
    inner_nodes: BPTree,
    /// The mini-page cache
    cache: MiniPageBuffer,
    /// The interface for all file io operation
    io_engine: IoEngine,
    /// The map from page ids to their location, either in the mini-page buffer or on disk
    map_table: MapTable,
    /// Write-ahead log for tombstones/deletes
    wal: Arc<WalManager>,
    wal_leaf_checkpoint_threshold: usize,
    wal_global_record_threshold: usize,
    wal_global_byte_threshold: usize,
    wal_checkpoint_requested: Arc<AtomicBool>,
    wal_checkpoint_stop: Arc<AtomicBool>,
    wal_checkpoint_thread: Option<thread::JoinHandle<()>>,
    next_txn_id: AtomicU64,
}

impl<'db> Drop for QuickStepTx<'db> {
    fn drop(&mut self) {
        if self.state == TxState::Active {
            self.abort_in_place();
        }
    }
}

const AUTO_MERGE_MIN_ENTRIES: usize = 3;
const DEFAULT_WAL_LEAF_CHECKPOINT_THRESHOLD: usize = 32;
const DEFAULT_WAL_GLOBAL_RECORD_THRESHOLD: usize = 1024;
const DEFAULT_WAL_GLOBAL_BYTE_THRESHOLD: usize = 512 * 1024;
const ENV_WAL_LEAF_THRESHOLD: &str = "QUICKSTEP_WAL_LEAF_THRESHOLD";
const ENV_WAL_GLOBAL_RECORD_THRESHOLD: &str = "QUICKSTEP_WAL_GLOBAL_RECORD_THRESHOLD";
const ENV_WAL_GLOBAL_BYTE_THRESHOLD: &str = "QUICKSTEP_WAL_GLOBAL_BYTE_THRESHOLD";
const CLI_WAL_LEAF_THRESHOLD: &str = "--quickstep-wal-leaf-threshold";
const CLI_WAL_GLOBAL_RECORD_THRESHOLD: &str = "--quickstep-wal-global-record-threshold";
const CLI_WAL_GLOBAL_BYTE_THRESHOLD: &str = "--quickstep-wal-global-byte-threshold";

#[derive(Debug)]
pub struct DebugLeafSnapshot {
    pub page_id: PageId,
    pub disk_addr: u64,
    pub keys: Vec<Vec<u8>>,
}

#[derive(Debug)]
pub struct DebugLeafFences {
    pub page_id: PageId,
    pub disk_addr: u64,
    pub lower: Vec<u8>,
    pub upper: Vec<u8>,
}

#[derive(Debug)]
pub struct DebugWalStats {
    pub total_records: usize,
    pub total_bytes: usize,
    pub leaf_records: Option<usize>,
    pub leaf_bytes: Option<usize>,
}

/// Config to create a new QuickStep instance
pub struct QuickStepConfig {
    /// Path for db information to be persisted
    path: PathBuf,
    /// Upper bounds on number of inner nodes
    /// This value should be tested but expected to be less than 1% of overall space
    inner_node_upper_bound: u32,
    /// Upper bound on the number of leaves that will need to be in the Mapping table
    leaf_upper_bound: u64,
    /// log base 2 of the cache size
    /// 30 - 1gb
    /// 40 - 2tb
    cache_size_lg: usize,
    wal_leaf_checkpoint_threshold: usize,
    wal_global_record_threshold: usize,
    wal_global_byte_threshold: usize,
}

impl QuickStepConfig {
    pub fn new<P: Into<PathBuf>>(
        path: P,
        inner_node_upper_bound: u32,
        leaf_upper_bound: u64,
        cache_size_lg: usize,
    ) -> QuickStepConfig {
        QuickStepConfig {
            path: path.into(),
            inner_node_upper_bound,
            leaf_upper_bound,
            cache_size_lg,
            wal_leaf_checkpoint_threshold: DEFAULT_WAL_LEAF_CHECKPOINT_THRESHOLD,
            wal_global_record_threshold: DEFAULT_WAL_GLOBAL_RECORD_THRESHOLD,
            wal_global_byte_threshold: DEFAULT_WAL_GLOBAL_BYTE_THRESHOLD,
        }
    }

    pub fn with_env_overrides(mut self) -> QuickStepConfig {
        if let Some(val) = read_env_usize(ENV_WAL_LEAF_THRESHOLD) {
            self.wal_leaf_checkpoint_threshold = val;
        }
        if let Some(val) = read_env_usize(ENV_WAL_GLOBAL_RECORD_THRESHOLD) {
            self.wal_global_record_threshold = val;
        }
        if let Some(val) = read_env_usize(ENV_WAL_GLOBAL_BYTE_THRESHOLD) {
            self.wal_global_byte_threshold = val;
        }
        self
    }

    pub fn with_cli_overrides<I, S>(mut self, args: I) -> QuickStepConfig
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut iter = args.into_iter();
        while let Some(arg) = iter.next() {
            let token: String = arg.into();
            if let Some(value) = parse_cli_override(&token, CLI_WAL_LEAF_THRESHOLD, &mut iter) {
                self.wal_leaf_checkpoint_threshold = value;
                continue;
            }
            if let Some(value) =
                parse_cli_override(&token, CLI_WAL_GLOBAL_RECORD_THRESHOLD, &mut iter)
            {
                self.wal_global_record_threshold = value;
                continue;
            }
            if let Some(value) =
                parse_cli_override(&token, CLI_WAL_GLOBAL_BYTE_THRESHOLD, &mut iter)
            {
                self.wal_global_byte_threshold = value;
                continue;
            }
        }
        self
    }

    pub fn with_wal_thresholds(
        mut self,
        leaf_checkpoint: usize,
        global_record: usize,
        global_bytes: usize,
    ) -> QuickStepConfig {
        self.wal_leaf_checkpoint_threshold = leaf_checkpoint;
        self.wal_global_record_threshold = global_record;
        self.wal_global_byte_threshold = global_bytes;
        self
    }

    pub fn wal_thresholds(&self) -> (usize, usize, usize) {
        (
            self.wal_leaf_checkpoint_threshold,
            self.wal_global_record_threshold,
            self.wal_global_byte_threshold,
        )
    }
}

impl QuickStep {
    pub fn new(mut config: QuickStepConfig) -> QuickStep {
        config = config
            .with_env_overrides()
            .with_cli_overrides(env::args().skip(1));

        let QuickStepConfig {
            path,
            inner_node_upper_bound,
            leaf_upper_bound,
            cache_size_lg,
            wal_leaf_checkpoint_threshold,
            wal_global_record_threshold,
            wal_global_byte_threshold,
        } = config;

        let data_path = resolve_data_path(&path);

        let io_engine =
            IoEngine::open(&data_path).expect("failed to open quickstep data file for writing");
        let wal_path = wal_path_for(&data_path);
        let wal = Arc::new(
            WalManager::open(&wal_path).expect("failed to open quickstep write-ahead log file"),
        );
        let cache = MiniPageBuffer::new(cache_size_lg);
        let wal_checkpoint_requested = Arc::new(AtomicBool::new(false));
        let wal_checkpoint_stop = Arc::new(AtomicBool::new(false));
        let wal_checkpoint_thread = {
            let wal_clone = Arc::clone(&wal);
            let stop_clone = Arc::clone(&wal_checkpoint_stop);
            let flag_clone = Arc::clone(&wal_checkpoint_requested);
            let record_thresh = wal_global_record_threshold;
            let byte_thresh = wal_global_byte_threshold;
            Some(thread::spawn(move || {
                while !stop_clone.load(Ordering::Relaxed) {
                    if wal_clone.total_records() >= record_thresh
                        || wal_clone.total_bytes() >= byte_thresh
                    {
                        flag_clone.store(true, Ordering::Release);
                    }
                    thread::sleep(Duration::from_millis(50));
                }
            }))
        };

        let mut quickstep = QuickStep {
            inner_nodes: BPTree::new(inner_node_upper_bound),
            cache,
            io_engine,
            map_table: MapTable::new(leaf_upper_bound),
            wal,
            wal_leaf_checkpoint_threshold,
            wal_global_record_threshold,
            wal_global_byte_threshold,
            wal_checkpoint_requested,
            wal_checkpoint_stop,
            wal_checkpoint_thread,
            next_txn_id: AtomicU64::new(1),
        };

        quickstep.ensure_root_leaf_on_disk();
        quickstep.replay_wal();

        // initialise root leaf (page 0 for now)
        let root_page = quickstep.map_table.init_leaf_entry(0);
        quickstep.inner_nodes.set_leaf_root(root_page);

        quickstep
    }

    /// Create a new transaction for isolated operations
    pub fn tx(&self) -> QuickStepTx<'_> {
        let txn_id = self.next_txn_id.fetch_add(1, Ordering::Relaxed);
        self.wal
            .append_txn_marker(WalTxnMarker::Begin, WalEntryKind::Redo, txn_id)
            .expect("failed to record txn begin");
        // coordination is done via the locks so it can just hold a reference to the db
        QuickStepTx {
            db: self,
            lock_manager: LockManager::new(),
            txn_id,
            wal_entry_kind: WalEntryKind::Redo,
            undo_log: Vec::new(),
            state: TxState::Active,
        }
    }
}

impl Drop for QuickStep {
    fn drop(&mut self) {
        self.wal_checkpoint_stop.store(true, Ordering::Release);
        if let Some(handle) = self.wal_checkpoint_thread.take() {
            let _ = handle.join();
        }
    }
}

impl QuickStep {
    fn ensure_root_leaf_on_disk(&self) {
        let mut leaf = self.io_engine.get_page(0);
        {
            let meta = leaf.as_mut();
            if meta.record_count() >= 2 {
                return;
            }
            meta.format_leaf(PageId(0), NodeSize::LeafPage, 0);
        }
        self.io_engine.write_page(0, &leaf);
    }

    /// Test helper to inspect the root after splits; not intended for production use.
    pub fn debug_root_leaf_parent(&self) -> Option<DebugLeafParent> {
        self.inner_nodes.debug_root_leaf_parent()
    }

    pub fn debug_root_level(&self) -> u16 {
        self.inner_nodes.root_level()
    }

    /// Test helper: materialises the user keys stored in the specified leaf page.
    /// This acquires a transient read lock on the map table entry and copies the keys,
    /// so it is safe to drop immediately after use in tests.
    pub fn debug_leaf_snapshot(&self, page_id: PageId) -> Result<DebugLeafSnapshot, QSError> {
        let guard = self.map_table.read_page_entry(page_id)?;
        let snapshot = match guard.node() {
            NodeRef::MiniPage(index) => {
                let meta = unsafe { self.cache.get_meta_ref(index) };
                DebugLeafSnapshot {
                    page_id,
                    disk_addr: meta.leaf(),
                    keys: collect_user_keys(meta),
                }
            }
            NodeRef::Leaf(disk_addr) => {
                let disk_leaf = self.io_engine.get_page(disk_addr);
                let meta = disk_leaf.as_ref();
                DebugLeafSnapshot {
                    page_id,
                    disk_addr,
                    keys: collect_user_keys(meta),
                }
            }
        };
        Ok(snapshot)
    }

    pub fn debug_leaf_fences(&self, page_id: PageId) -> Result<DebugLeafFences, QSError> {
        let guard = self.map_table.read_page_entry(page_id)?;
        let (disk_addr, lower, upper) = match guard.node() {
            NodeRef::MiniPage(index) => {
                let meta = unsafe { self.cache.get_meta_ref(index) };
                let (lower, upper) = collect_fence_keys(meta);
                (meta.leaf(), lower, upper)
            }
            NodeRef::Leaf(disk_addr) => {
                let disk_leaf = self.io_engine.get_page(disk_addr);
                let meta = disk_leaf.as_ref();
                let (lower, upper) = collect_fence_keys(meta);
                (disk_addr, lower, upper)
            }
        };

        Ok(DebugLeafFences {
            page_id,
            disk_addr,
            lower,
            upper,
        })
    }

    pub fn debug_wal_stats(&self, page_id: Option<PageId>) -> DebugWalStats {
        let (leaf_records, leaf_bytes) = page_id
            .and_then(|pid| self.wal.leaf_stats(pid))
            .map(|(records, bytes)| (Some(records), Some(bytes)))
            .unwrap_or((None, None));

        DebugWalStats {
            total_records: self.wal.total_records(),
            total_bytes: self.wal.total_bytes(),
            leaf_records,
            leaf_bytes,
        }
    }

    fn replay_wal(&self) {
        let mut grouped = self.wal.records_grouped();
        if grouped.is_empty() {
            return;
        }

        let txn_meta = grouped.remove(&TXN_META_PAGE_ID).unwrap_or_default();
        let committed = self.committed_txn_ids(&txn_meta);

        for (page_key, records) in grouped.into_iter() {
            let page_id = PageId(page_key);
            if page_key as usize >= self.map_table.capacity() {
                continue;
            }
            if !self.map_table.has_entry(page_id) {
                continue;
            }
            let mut lower: Option<Vec<u8>> = None;
            let mut upper: Option<Vec<u8>> = None;
            let mut entries: BTreeMap<Vec<u8>, Vec<u8>> = BTreeMap::new();

            for record in records {
                let WalRecord {
                    page_id: _,
                    key,
                    lower_fence: record_lower,
                    upper_fence: record_upper,
                    kind,
                    txn_id,
                    op,
                    ..
                } = record;
                if matches!(kind, WalEntryKind::Undo) {
                    continue;
                }
                if let Some(committed) = committed.as_ref() {
                    if !committed.contains(&txn_id) {
                        continue;
                    }
                }
                lower = Some(record_lower);
                upper = Some(record_upper);
                match op {
                    WalOp::Tombstone => {
                        entries.remove(&key);
                    }
                    WalOp::Put { value } => {
                        entries.insert(key, value);
                    }
                    WalOp::TxnMarker(_) => continue,
                }
            }

            let (lower_fence, upper_fence) = match (lower, upper) {
                (Some(l), Some(u)) => (l, u),
                _ => continue,
            };

            let guard = self
                .map_table
                .read_page_entry(page_id)
                .expect("WAL replay requires mapped page");
            let node_ref = guard.node();
            let disk_addr = match node_ref {
                NodeRef::Leaf(addr) => addr,
                NodeRef::MiniPage(idx) => unsafe { self.cache.get_meta_ref(idx) }.leaf(),
            };

            {
                let mut leaf = self.io_engine.get_page(disk_addr);
                {
                    let meta = leaf.as_mut();
                    meta.reset_user_entries_with_fences(&lower_fence, &upper_fence);
                    meta.replay_entries(
                        entries
                            .iter()
                            .map(|(key, value)| (key.as_slice(), value.as_slice())),
                    )
                    .expect("disk leaf should accept WAL replay");
                }
                self.io_engine.write_page(disk_addr, &leaf);
            }

            if let NodeRef::MiniPage(idx) = node_ref {
                let meta = unsafe { self.cache.get_meta_mut(idx) };
                meta.reset_user_entries_with_fences(&lower_fence, &upper_fence);
                meta.replay_entries(
                    entries
                        .iter()
                        .map(|(key, value)| (key.as_slice(), value.as_slice())),
                )
                .expect("cached leaf should accept WAL replay");
            }
        }
        self.wal.clear().expect("failed to clear WAL after replay");
    }

    fn committed_txn_ids(&self, txn_meta: &[WalRecord]) -> Option<HashSet<u64>> {
        if txn_meta.is_empty() {
            return None;
        }
        let mut latest: HashMap<u64, WalTxnMarker> = HashMap::new();
        for record in txn_meta {
            if let WalOp::TxnMarker(marker) = &record.op {
                latest.insert(record.txn_id, *marker);
            }
        }
        if latest.is_empty() {
            return None;
        }
        let committed: HashSet<u64> = latest
            .into_iter()
            .filter_map(|(txn_id, marker)| match marker {
                WalTxnMarker::Commit => Some(txn_id),
                _ => None,
            })
            .collect();
        Some(committed)
    }

    pub fn debug_wal_record_count(&self) -> usize {
        self.wal.total_records()
    }
}

pub struct QuickStepTx<'db> {
    db: &'db QuickStep,
    lock_manager: LockManager<'db>,
    txn_id: u64,
    wal_entry_kind: WalEntryKind,
    undo_log: Vec<UndoAction>,
    state: TxState,
    // changes for rollback
}

#[derive(Debug, PartialEq, Eq)]
enum TxState {
    Active,
    Committed,
    Aborted,
}

#[derive(Debug)]
enum UndoAction {
    Restore {
        page_id: PageId,
        key: Vec<u8>,
        value: Vec<u8>,
    },
    Remove {
        page_id: PageId,
        key: Vec<u8>,
    },
}

impl<'db> QuickStepTx<'db> {
    /// Get a value
    pub fn get<'tx>(&'tx mut self, key: &[u8]) -> Result<Option<&'tx [u8]>, QSError> {
        let page = self.db.inner_nodes.read_traverse_leaf(key)?.page;

        let page_guard = self
            .lock_manager
            .get_or_acquire_read_lock(&self.db.map_table, page)?;

        let res = page_guard.get(&self.db.cache, &self.db.io_engine, key)?;

        Ok(res)
    }

    /// Insert or update a value
    pub fn put<'tx>(&'tx mut self, key: &[u8], val: &[u8]) -> Result<(), QSError> {
        let res = self.db.inner_nodes.read_traverse_leaf(key)?;

        let mut page_guard = self
            .lock_manager
            .get_upgrade_or_acquire_write_lock(&self.db.map_table, res.page)?;

        let undo_value = Self::existing_value(self.db, &mut page_guard, key);

        loop {
            match Self::try_put_with_promotion(self.db, &mut page_guard, key, val)? {
                TryPutResult::Success => {
                    self.append_wal_put(&mut page_guard, key, val, undo_value.clone())?;
                    self.maybe_global_checkpoint()?;
                    return Ok(());
                }
                TryPutResult::NeedsSplit => {
                    page_guard = self.split_current_leaf(page_guard, key)?;
                }
                TryPutResult::NeedsPromotion(_) => unreachable!("promotion handled before returning"),
            }
        }
    }

    pub fn abort(mut self) {
        self.abort_in_place();
    }

    pub fn commit(mut self) {
        self.commit_in_place();
    }

    fn commit_in_place(&mut self) {
        if self.state != TxState::Active {
            return;
        }
        self.db
            .wal
            .append_txn_marker(WalTxnMarker::Commit, self.wal_entry_kind, self.txn_id)
            .expect("failed to record txn commit");
        self.undo_log.clear();
        self.state = TxState::Committed;
    }

    fn abort_in_place(&mut self) {
        if self.state != TxState::Active {
            return;
        }
        self.apply_undo_actions()
            .expect("failed to roll back transaction");
        self.db
            .wal
            .append_txn_marker(WalTxnMarker::Abort, self.wal_entry_kind, self.txn_id)
            .expect("failed to record txn abort");
        self.undo_log.clear();
        self.state = TxState::Aborted;
    }
}

fn resolve_data_path(path: &Path) -> PathBuf {
    if path.is_dir() || path.extension().is_none() {
        path.join("quickstep.db")
    } else {
        path.to_path_buf()
    }
}

fn wal_path_for(data_path: &Path) -> PathBuf {
    let mut wal_path = data_path.to_path_buf();
    wal_path.set_extension("wal");
    wal_path
}

fn read_env_usize(key: &str) -> Option<usize> {
    env::var(key)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
}

fn parse_cli_override<I, S>(token: &str, flag: &str, iter: &mut I) -> Option<usize>
where
    I: Iterator<Item = S>,
    S: Into<String>,
{
    if let Some(rest) = token.strip_prefix(flag) {
        if let Some(value) = rest.strip_prefix('=') {
            return value.parse::<usize>().ok();
        }
    }
    if token == flag {
        if let Some(next) = iter.next() {
            let value: String = next.into();
            return value.parse::<usize>().ok();
        }
    }
    None
}

impl<'db> QuickStepTx<'db> {
    fn plan_leaf_split(
        db: &'db QuickStep,
        page_guard: &mut WriteGuardWrapper<'db>,
    ) -> LeafSplitPlan {
        let write_guard = page_guard.get_write_guard();
        match write_guard.node() {
            NodeRef::MiniPage(idx) => {
                let node_meta = unsafe { db.cache.get_meta_ref(idx) };
                LeafSplitPlan::from_node(node_meta)
            }
            NodeRef::Leaf(_) => unreachable!("leaf splits only apply to cached mini-pages"),
        }
    }

    fn split_current_leaf(
        &mut self,
        mut left_guard: WriteGuardWrapper<'db>,
        key: &[u8],
    ) -> Result<WriteGuardWrapper<'db>, QSError> {
        let (mut lock_bundle, page_id) = self.lock_bundle_for_split(key)?;
        debug_assert_eq!(
            page_id,
            left_guard.page_id(),
            "split lock bundle must reference active leaf"
        );

        let mut right_guard = self.new_mini_page(NodeSize::LeafPage, None)?;
        let split_plan = Self::plan_leaf_split(self.db, &mut left_guard);

        let split_outcome =
            Self::apply_leaf_split(self.db, &mut left_guard, &mut right_guard, &split_plan)?;

        debug::record_split_event(
            left_guard.page_id().0,
            right_guard.page_id().0,
            split_outcome.pivot_key.clone(),
            split_outcome.left_count,
            split_outcome.right_count,
        );

        self.insert_into_parents_after_leaf_split(
            &mut lock_bundle,
            left_guard.page_id(),
            &split_outcome.pivot_key,
            right_guard.page_id(),
        )?;

        let pivot_key = split_outcome.pivot_key.clone();
        if key >= pivot_key.as_slice() {
            drop(left_guard);
            Ok(right_guard)
        } else {
            drop(right_guard);
            Ok(left_guard)
        }
    }

    fn lock_bundle_for_split(
        &self,
        key: &[u8],
    ) -> Result<(WriteLockBundle<'db>, PageId), QSError> {
        let res = self.db.inner_nodes.read_traverse_leaf(key)?;
        let bundle = self
            .db
            .inner_nodes
            .write_lock(res.overflow_point, OpType::Split, key)?;
        Ok((bundle, res.page))
    }

    fn append_wal_put(
        &mut self,
        guard: &mut WriteGuardWrapper<'db>,
        key: &[u8],
        val: &[u8],
        undo_value: Option<Vec<u8>>,
    ) -> Result<(), QSError> {
        let page_id = guard.page_id();
        let (_disk_addr, lower_fence, upper_fence) = Self::leaf_snapshot(self.db, guard);
        self.db
            .wal
            .append_put(
                page_id,
                key,
                val,
                &lower_fence,
                &upper_fence,
                self.wal_entry_kind,
                self.txn_id,
            )
            .expect("failed to record put in WAL");
        if let Some(prev) = undo_value.as_ref() {
            self.db
                .wal
                .append_put(
                    page_id,
                    key,
                    prev,
                    &lower_fence,
                    &upper_fence,
                    WalEntryKind::Undo,
                    self.txn_id,
                )
                .expect("failed to record undo put in WAL");
        } else {
            self.db
                .wal
                .append_tombstone(
                    page_id,
                    key,
                    &lower_fence,
                    &upper_fence,
                    WalEntryKind::Undo,
                    self.txn_id,
                )
                .expect("failed to record undo tombstone in WAL");
        }
        self.log_put_undo(page_id, key, undo_value);
        Self::maybe_checkpoint_leaf(self.db, guard, page_id)?;
        Ok(())
    }

    fn log_put_undo(&mut self, page_id: PageId, key: &[u8], undo_value: Option<Vec<u8>>) {
        match undo_value {
            Some(value) => self.undo_log.push(UndoAction::Restore {
                page_id,
                key: key.to_vec(),
                value,
            }),
            None => self.undo_log.push(UndoAction::Remove {
                page_id,
                key: key.to_vec(),
            }),
        }
    }

    fn log_delete_undo(&mut self, page_id: PageId, key: &[u8], value: Option<Vec<u8>>) {
        if let Some(value) = value {
            self.undo_log.push(UndoAction::Restore {
                page_id,
                key: key.to_vec(),
                value,
            });
        }
    }

    fn apply_undo_actions(&mut self) -> Result<(), QSError> {
        while let Some(action) = self.undo_log.pop() {
            self.apply_undo_action(action)?;
        }
        Ok(())
    }

    fn apply_undo_action(&mut self, action: UndoAction) -> Result<(), QSError> {
        let page_id = match &action {
            UndoAction::Restore { page_id, .. } | UndoAction::Remove { page_id, .. } => *page_id,
        };
        let mut guard = self
            .lock_manager
            .get_upgrade_or_acquire_write_lock(&self.db.map_table, page_id)?;
        Self::ensure_mini_page(self.db, &mut guard)?;
        let index = match guard.get_write_guard().node() {
            NodeRef::MiniPage(idx) => idx,
            NodeRef::Leaf(_) => unreachable!("mini page expected after promotion"),
        };
        let meta = unsafe { self.db.cache.get_meta_mut(index) };
        match action {
            UndoAction::Restore { key, value, .. } => {
                meta.remove_key_physical(&key);
                meta.try_put(&key, &value)
                    .map_err(|_| QSError::SplitFailed)?;
            }
            UndoAction::Remove { key, .. } => {
                meta.remove_key_physical(&key);
            }
        }
        Ok(())
    }

    fn leaf_snapshot(
        db: &'db QuickStep,
        guard: &mut WriteGuardWrapper<'db>,
    ) -> (u64, Vec<u8>, Vec<u8>) {
        match guard.get_write_guard().node() {
            NodeRef::MiniPage(idx) => {
                let meta = unsafe { db.cache.get_meta_ref(idx) };
                let (lower, upper) = meta.fence_bounds();
                (meta.leaf(), lower, upper)
            }
            NodeRef::Leaf(addr) => {
                let leaf = db.io_engine.get_page(addr);
                let meta = leaf.as_ref();
                let (lower, upper) = collect_fence_keys(meta);
                (addr, lower, upper)
            }
        }
    }

    fn existing_value(
        db: &'db QuickStep,
        guard: &mut WriteGuardWrapper<'db>,
        key: &[u8],
    ) -> Option<Vec<u8>> {
        match guard.get_write_guard().node() {
            NodeRef::MiniPage(idx) => {
                let meta = unsafe { db.cache.get_meta_ref(idx) };
                meta.get(key).map(|value| value.to_vec())
            }
            NodeRef::Leaf(addr) => {
                let leaf = db.io_engine.get_page(addr);
                leaf.as_ref().get(key).map(|value| value.to_vec())
            }
        }
    }

    fn maybe_checkpoint_leaf(
        db: &'db QuickStep,
        guard: &mut WriteGuardWrapper<'db>,
        page_id: PageId,
    ) -> Result<(), QSError> {
        if !db
            .wal
            .should_checkpoint_page(page_id, db.wal_leaf_checkpoint_threshold)
        {
            return Ok(());
        }
        Self::ensure_mini_page(db, guard)?;
        guard.merge_to_disk(&db.cache, &db.io_engine);
        db.wal
            .checkpoint_page(page_id)
            .expect("failed to checkpoint WAL for leaf");
        Ok(())
    }

    fn maybe_global_checkpoint(&mut self) -> Result<(), QSError> {
        let requested = self.db.wal_checkpoint_requested.load(Ordering::Acquire);
        let candidate = self
            .db
            .wal
            .global_checkpoint_candidate(
                self.db.wal_global_record_threshold,
                self.db.wal_global_byte_threshold,
            )
            .or_else(|| {
                if requested {
                    self.db.wal.global_checkpoint_candidate(0, 0)
                } else {
                    None
                }
            });
        if let Some(page_id) = candidate {
            let mut guard = self
                .lock_manager
                .get_upgrade_or_acquire_write_lock(&self.db.map_table, page_id)?;
            Self::ensure_mini_page(self.db, &mut guard)?;
            guard.merge_to_disk(&self.db.cache, &self.db.io_engine);
            self.db
                .wal
                .checkpoint_page(page_id)
                .expect("failed to checkpoint WAL for candidate leaf");
            self.db
                .wal_checkpoint_requested
                .store(false, Ordering::Release);
        }
        Ok(())
    }

    fn apply_leaf_split(
        db: &'db QuickStep,
        left_guard: &mut WriteGuardWrapper<'db>,
        right_guard: &mut WriteGuardWrapper<'db>,
        plan: &LeafSplitPlan,
    ) -> Result<LeafSplitOutcome, QSError> {
        let left_index = match left_guard.get_write_guard().node() {
            NodeRef::MiniPage(idx) => idx,
            NodeRef::Leaf(_) => return Err(QSError::SplitFailed),
        };

        let right_index = match right_guard.get_write_guard().node() {
            NodeRef::MiniPage(idx) => idx,
            NodeRef::Leaf(_) => return Err(QSError::SplitFailed),
        };

        let copy_bytes = unsafe { db.cache.get_meta_ref(left_index).size().size_in_bytes() };

        unsafe {
            let src = db.cache.get_meta_ptr(left_index.index) as *const u8;
            let dst = db.cache.get_meta_ptr(right_index.index) as *mut u8;
            ptr::copy_nonoverlapping(src, dst, copy_bytes);
        }

        let left_meta = unsafe { db.cache.get_meta_mut(left_index) };
        let right_meta = unsafe { db.cache.get_meta_mut(right_index) };
        let right_page_id = right_meta.page_id();
        let right_disk_addr = right_meta.leaf();

        plan.apply(left_meta, right_meta)
            .map_err(|_| QSError::SplitFailed)
            .map(|outcome| {
                right_meta.set_identity(right_page_id, right_disk_addr);
                outcome
            })
    }
    fn try_put_with_promotion(
        db: &'db QuickStep,
        page_guard: &mut WriteGuardWrapper<'db>,
        key: &[u8],
        val: &[u8],
    ) -> Result<TryPutResult, QSError> {
        let attempt = page_guard.try_put(&db.cache, key, val);
        match attempt {
            TryPutResult::NeedsPromotion(addr) => {
                Self::promote_leaf_to_mini_page(db, page_guard, addr)?;
                Self::try_put_with_promotion(db, page_guard, key, val)
            }
            other => Ok(other),
        }
    }

    fn promote_leaf_to_mini_page(
        db: &'db QuickStep,
        page_guard: &mut WriteGuardWrapper<'db>,
        disk_addr: u64,
    ) -> Result<(), QSError> {
        let cache_index = db
            .cache
            .alloc(NodeSize::LeafPage)
            .ok_or(QSError::CacheExhausted)?;

        let disk_leaf = page_guard.load_leaf(&db.io_engine, disk_addr)?;
        let src_ptr = disk_leaf.as_ref() as *const NodeMeta as *const u8;
        let leaf_bytes = NodeSize::LeafPage.size_in_bytes();

        unsafe {
            let mini_index = MiniPageIndex::new(cache_index);
            let write_guard = page_guard.get_write_guard();
            let logical_page = write_guard.page;
            write_guard.set_mini_page(mini_index);

            let dst = db.cache.get_meta_ptr(cache_index) as *mut u8;
            ptr::copy_nonoverlapping(src_ptr, dst, leaf_bytes);
            let node_meta = db.cache.get_meta_mut(mini_index);
            debug_assert!(
                node_meta.record_count() >= 2,
                "disk leaf for page {} missing fence keys",
                logical_page.0
            );
            node_meta.mark_hot();
        }

        Ok(())
    }

    fn ensure_mini_page(
        db: &'db QuickStep,
        page_guard: &mut WriteGuardWrapper<'db>,
    ) -> Result<(), QSError> {
        loop {
            match page_guard.get_write_guard().node() {
                NodeRef::MiniPage(_) => return Ok(()),
                NodeRef::Leaf(addr) => {
                    Self::promote_leaf_to_mini_page(db, page_guard, addr)?;
                }
            }
        }
    }

    fn new_mini_page(
        &mut self,
        size: NodeSize,
        disk_addr: Option<u64>,
    ) -> Result<WriteGuardWrapper<'db>, QSError> {
        let new_mini_page = loop {
            if let Some(idx) = self.db.cache.alloc(size) {
                break idx;
            }
            self.db
                .cache
                .evict(&self.db.map_table, &self.db.io_engine, &self.db.wal)?;
        };

        let mut guard = unsafe { NodeMeta::init(self, new_mini_page, size, disk_addr) };

        if let NodeRef::MiniPage(index) = guard.get_write_guard().node() {
            let meta = unsafe { self.db.cache.get_meta_mut(index) };
            meta.ensure_fence_keys();
        }

        Ok(guard)
    }

    fn insert_into_parents_after_leaf_split(
        &mut self,
        lock_bundle: &mut WriteLockBundle<'db>,
        left_leaf: PageId,
        pivot_key: &[u8],
        right_leaf: PageId,
    ) -> Result<(), QSError> {
        if lock_bundle.chain.is_empty() {
            return self.db.inner_nodes.promote_leaf_root(
                lock_bundle
                    .root_lock
                    .as_mut()
                    .expect("root lock must exist for root split"),
                left_leaf,
                right_leaf,
                pivot_key,
            );
        }

        let parent_idx = lock_bundle.chain.len() - 1;
        let level = lock_bundle.chain[parent_idx].level;
        let guard = &mut lock_bundle.chain[parent_idx].guard;

        match guard.insert_entry_after_child(
            level,
            ChildPointer::Leaf(left_leaf),
            pivot_key,
            ChildPointer::Leaf(right_leaf),
        ) {
            Ok(()) => Ok(()),
            Err(QSError::NodeFull) => {
                let split = self.db.inner_nodes.split_inner_node(
                    guard,
                    level,
                    ChildPointer::Leaf(left_leaf),
                    pivot_key,
                    ChildPointer::Leaf(right_leaf),
                )?;

                let pending = PendingParentSplit {
                    left_child: ChildPointer::Inner(guard.node_id()),
                    pivot_key: split.pivot_key,
                    right_child: ChildPointer::Inner(split.right_node),
                    child_level: level,
                };

                self.bubble_split_up(lock_bundle, parent_idx, pending)
            }
            Err(e) => Err(e),
        }
    }

    fn bubble_split_up(
        &mut self,
        lock_bundle: &mut WriteLockBundle<'db>,
        mut idx: usize,
        mut pending: PendingParentSplit,
    ) -> Result<(), QSError> {
        while idx > 0 {
            idx -= 1;
            let level = lock_bundle.chain[idx].level;
            let guard = &mut lock_bundle.chain[idx].guard;
            match guard.insert_entry_after_child(
                level,
                pending.left_child,
                &pending.pivot_key,
                pending.right_child,
            ) {
                Ok(()) => return Ok(()),
                Err(QSError::NodeFull) => {
                    let split = self.db.inner_nodes.split_inner_node(
                        guard,
                        level,
                        pending.left_child,
                        &pending.pivot_key,
                        pending.right_child,
                    )?;

                    pending = PendingParentSplit {
                        left_child: ChildPointer::Inner(guard.node_id()),
                        pivot_key: split.pivot_key,
                        right_child: ChildPointer::Inner(split.right_node),
                        child_level: level,
                    };
                }
                Err(e) => return Err(e),
            }
        }

        let root_lock = lock_bundle
            .root_lock
            .as_mut()
            .expect("root lock must exist for cascading split");
        self.db.inner_nodes.promote_inner_root(
            root_lock,
            pending.left_child.as_inner(),
            pending.right_child.as_inner(),
            &pending.pivot_key,
            pending.child_level,
        )
    }

    fn merge_leaf_pages(
        &mut self,
        left_guard: &mut WriteGuardWrapper<'db>,
        right_guard: &mut WriteGuardWrapper<'db>,
        lock_bundle: &mut WriteLockBundle<'db>,
    ) -> Result<(), QSError> {
        Self::ensure_mini_page(self.db, left_guard)?;
        Self::ensure_mini_page(self.db, right_guard)?;

        let left_index = match left_guard.get_write_guard().node() {
            NodeRef::MiniPage(idx) => idx,
            NodeRef::Leaf(_) => unreachable!("mini page expected after promotion"),
        };
        let right_index = match right_guard.get_write_guard().node() {
            NodeRef::MiniPage(idx) => idx,
            NodeRef::Leaf(_) => unreachable!("mini page expected after promotion"),
        };

        let left_meta = unsafe { self.db.cache.get_meta_mut(left_index) };
        let right_meta = unsafe { self.db.cache.get_meta_mut(right_index) };
        let plan = LeafMergePlan::from_nodes(left_meta, right_meta);
        let outcome = plan
            .apply(left_meta, right_meta)
            .map_err(|_| QSError::MergeFailed)?;

        debug::record_merge_event(
            left_guard.page_id().0,
            right_guard.page_id().0,
            outcome.merged_count,
        );

        self.remove_parent_after_merge(lock_bundle, left_guard.page_id(), right_guard.page_id())
    }

    fn remove_parent_after_merge(
        &mut self,
        lock_bundle: &mut WriteLockBundle<'db>,
        survivor: PageId,
        removed: PageId,
    ) -> Result<(), QSError> {
        if lock_bundle.chain.is_empty() {
            return Ok(());
        }

        let parent_idx = lock_bundle.chain.len() - 1;
        let level = lock_bundle.chain[parent_idx].level;
        let guard = &mut lock_bundle.chain[parent_idx].guard;
        let demote = self.db.inner_nodes.remove_child_after_merge(
            guard,
            level,
            ChildPointer::Leaf(survivor),
            ChildPointer::Leaf(removed),
        )?;

        if let Some(mut child) = demote {
            if parent_idx == 0 {
                if let Some(ref mut root_lock) = lock_bundle.root_lock {
                    self.db
                        .inner_nodes
                        .demote_root_after_merge(root_lock, child, level)?;
                }
                return Ok(());
            }

            lock_bundle.chain.pop();
            let mut idx = parent_idx - 1;
            loop {
                let parent_level = lock_bundle.chain[idx].level;
                let guard = &mut lock_bundle.chain[idx].guard;
                let demotion = self.db.inner_nodes.remove_child_after_merge(
                    guard,
                    parent_level,
                    child,
                    ChildPointer::Inner(guard.node_id()),
                )?;

                if let Some(child_ptr) = demotion {
                    if idx == 0 {
                        if let Some(ref mut root_lock) = lock_bundle.root_lock {
                            self.db.inner_nodes.demote_root_after_merge(
                                root_lock,
                                child_ptr,
                                parent_level,
                            )?;
                        }
                        break;
                    } else {
                        child = child_ptr;
                        idx -= 1;
                        continue;
                    }
                } else {
                    break;
                }
            }
        }

        Ok(())
    }
}

struct PendingParentSplit {
    left_child: ChildPointer,
    pivot_key: Vec<u8>,
    right_child: ChildPointer,
    child_level: u16,
}

fn collect_user_keys(meta: &NodeMeta) -> Vec<Vec<u8>> {
    let prefix = meta.get_node_prefix();
    meta.entries()
        .filter(|entry| !entry.meta.fence())
        .map(|entry| {
            let mut key = Vec::with_capacity(prefix.len() + entry.key_suffix.len());
            key.extend_from_slice(prefix);
            key.extend_from_slice(entry.key_suffix);
            key
        })
        .collect()
}

fn collect_fence_keys(meta: &NodeMeta) -> (Vec<u8>, Vec<u8>) {
    assert!(
        meta.record_count() >= 2,
        "leaf must contain at least the two fence keys"
    );
    let lower_meta = meta.get_kv_meta(0);
    let upper_meta = meta.get_kv_meta(meta.record_count() as usize - 1);
    assert!(
        lower_meta.fence() && upper_meta.fence(),
        "first and last entries must be fences"
    );
    let lower = meta.get_stored_key_from_meta(lower_meta).to_vec();
    let upper = meta.get_stored_key_from_meta(upper_meta).to_vec();
    (lower, upper)
}

fn collect_user_records(meta: &NodeMeta) -> Vec<(Vec<u8>, Vec<u8>)> {
    let prefix = meta.get_node_prefix();
    meta.entries()
        .filter(|entry| !entry.meta.fence())
        .map(|entry| {
            let mut key = Vec::with_capacity(prefix.len() + entry.key_suffix.len());
            key.extend_from_slice(prefix);
            key.extend_from_slice(entry.key_suffix);
            (key, entry.value.to_vec())
        })
        .collect()
}

impl QuickStep {
    pub fn debug_truncate_leaf(
        &self,
        page_id: PageId,
        keep: usize,
        auto_merge: bool,
    ) -> Result<(), QSError> {
        let mut tx = self.tx();
        let res = tx.debug_truncate_leaf(page_id, keep, auto_merge);
        tx.commit();
        res
    }

    pub fn debug_merge_leaves(&self, left: PageId, right: PageId) -> Result<(), QSError> {
        let mut tx = self.tx();
        let res = tx.debug_merge_leaves(left, right);
        tx.commit();
        res
    }

    pub fn delete(&self, key: &[u8]) -> Result<bool, QSError> {
        let mut tx = self.tx();
        let res = tx.delete(key);
        tx.commit();
        res
    }

    pub fn debug_flush_leaf(&self, page_id: PageId) -> Result<(), QSError> {
        let mut tx = self.tx();
        let res = tx.debug_flush_leaf(page_id);
        tx.commit();
        res
    }

    pub fn debug_flush_root_leaf(&self) -> Result<(), QSError> {
        self.debug_flush_leaf(PageId(0))
    }
}

impl<'db> QuickStepTx<'db> {
    pub fn debug_truncate_leaf(
        &mut self,
        page_id: PageId,
        keep: usize,
        auto_merge: bool,
    ) -> Result<(), QSError> {
        let mut guard = self
            .lock_manager
            .get_upgrade_or_acquire_write_lock(&self.db.map_table, page_id)?;
        Self::ensure_mini_page(self.db, &mut guard)?;
        let index = match guard.get_write_guard().node() {
            NodeRef::MiniPage(idx) => idx,
            NodeRef::Leaf(_) => unreachable!("mini page expected after promotion"),
        };
        let meta = unsafe { self.db.cache.get_meta_mut(index) };
        let mut records = collect_user_records(meta);
        if records.len() <= keep {
            return Ok(());
        }
        records.truncate(keep);
        meta.reset_user_entries();
        meta.replay_entries(
            records
                .iter()
                .map(|(key, value)| (key.as_slice(), value.as_slice())),
        )
        .map_err(|_| QSError::MergeFailed)?;

        if auto_merge && records.len() <= AUTO_MERGE_MIN_ENTRIES {
            self.try_auto_merge(page_id)?;
        }

        Ok(())
    }

    pub fn debug_merge_leaves(&mut self, left: PageId, right: PageId) -> Result<(), QSError> {
        let mut left_guard = self
            .lock_manager
            .get_upgrade_or_acquire_write_lock(&self.db.map_table, left)?;
        let mut right_guard = self
            .lock_manager
            .get_upgrade_or_acquire_write_lock(&self.db.map_table, right)?;
        let merge_key = self.first_user_key(&mut left_guard)?;
        let read_res = self.db.inner_nodes.read_traverse_leaf(&merge_key)?;
        let lock_bundle =
            self.db
                .inner_nodes
                .write_lock(read_res.underflow_point, OpType::Merge, &merge_key);
        let mut lock_bundle = lock_bundle?;
        self.merge_leaf_pages(&mut left_guard, &mut right_guard, &mut lock_bundle)
    }

    fn try_auto_merge(&mut self, page_id: PageId) -> Result<(), QSError> {
        let Some(snapshot) = self.db.debug_root_leaf_parent() else {
            return Ok(());
        };
        if snapshot.children.len() < 2 {
            return Ok(());
        }
        let Some(idx) = snapshot.children.iter().position(|child| *child == page_id) else {
            return Ok(());
        };
        let neighbor_idx = if idx + 1 < snapshot.children.len() {
            idx + 1
        } else if idx > 0 {
            idx - 1
        } else {
            return Ok(());
        };
        let left_idx = neighbor_idx.min(idx);
        let right_idx = neighbor_idx.max(idx);
        let left_child = snapshot.children[left_idx];
        let right_child = snapshot.children[right_idx];
        self.debug_merge_leaves(left_child, right_child)
    }

    pub fn delete<'tx>(&'tx mut self, key: &[u8]) -> Result<bool, QSError> {
        let res = self.db.inner_nodes.read_traverse_leaf(key)?;
        let mut page_guard = self
            .lock_manager
            .get_upgrade_or_acquire_write_lock(&self.db.map_table, res.page)?;
        Self::ensure_mini_page(self.db, &mut page_guard)?;
        let page_id = page_guard.page_id();
        let index = match page_guard.get_write_guard().node() {
            NodeRef::MiniPage(idx) => idx,
            NodeRef::Leaf(_) => unreachable!("mini page expected after promotion"),
        };
        let deleted_value;
        let user_entries;
        {
            let meta = unsafe { self.db.cache.get_meta_mut(index) };
            deleted_value = meta.get(key).map(|value| value.to_vec());
            if deleted_value.is_none() {
                return Ok(false);
            }
            let removed = meta.mark_tombstone(key);
            if !removed {
                return Ok(false);
            }
            user_entries = meta.user_entry_count();
        }
        let (_disk_addr, lower_fence, upper_fence) = Self::leaf_snapshot(self.db, &mut page_guard);
        self.db
            .wal
            .append_tombstone(
                page_id,
                key,
                &lower_fence,
                &upper_fence,
                self.wal_entry_kind,
                self.txn_id,
            )
            .expect("failed to record delete in WAL");
        if let Some(prev) = deleted_value.as_ref() {
            self.db
                .wal
                .append_put(
                    page_id,
                    key,
                    prev,
                    &lower_fence,
                    &upper_fence,
                    WalEntryKind::Undo,
                    self.txn_id,
                )
                .expect("failed to record undo delete in WAL");
        }
        self.log_delete_undo(page_id, key, deleted_value);
        Self::maybe_checkpoint_leaf(self.db, &mut page_guard, page_id)?;
        self.maybe_global_checkpoint()?;
        if user_entries <= AUTO_MERGE_MIN_ENTRIES {
            self.try_auto_merge(page_id)?;
        }
        Ok(true)
    }

    pub fn debug_flush_leaf(&mut self, page_id: PageId) -> Result<(), QSError> {
        let mut guard = self
            .lock_manager
            .get_upgrade_or_acquire_write_lock(&self.db.map_table, page_id)?;
        Self::ensure_mini_page(self.db, &mut guard)?;
        guard.merge_to_disk(&self.db.cache, &self.db.io_engine);
        self.db
            .wal
            .checkpoint_page(page_id)
            .expect("failed to checkpoint WAL for flushed leaf");
        Ok(())
    }

    fn first_user_key(&mut self, guard: &mut WriteGuardWrapper<'db>) -> Result<Vec<u8>, QSError> {
        Self::ensure_mini_page(self.db, guard)?;
        let index = match guard.get_write_guard().node() {
            NodeRef::MiniPage(idx) => idx,
            NodeRef::Leaf(_) => unreachable!("mini page expected after promotion"),
        };
        let meta = unsafe { self.db.cache.get_meta_ref(index) };
        let prefix = meta.get_node_prefix();
        meta.entries()
            .find(|entry| !entry.meta.fence())
            .map(|entry| {
                let mut key = Vec::with_capacity(prefix.len() + entry.key_suffix.len());
                key.extend_from_slice(prefix);
                key.extend_from_slice(entry.key_suffix);
                key
            })
            .ok_or(QSError::MergeFailed)
    }
}
