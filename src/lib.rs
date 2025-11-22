//! Quickstep - A modern, concurrent embedded key-value store implementing the Bf-tree data structure.
//!
//! This implementation is based on the original work by [Raphael Darley](https://github.com/RaphaelDarley/quickstep).
//! The core architecture and implementation are led by Raphael Darley.
//!
//! For more information, see the [README](https://github.com/merlinai-com/quickstep) and
//! [design documentation](../design/).

use std::{
    path::{Path, PathBuf},
    ptr,
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
}

const AUTO_MERGE_MIN_ENTRIES: usize = 3;

#[derive(Debug)]
pub struct DebugLeafSnapshot {
    pub page_id: PageId,
    pub disk_addr: u64,
    pub keys: Vec<Vec<u8>>,
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
        }
    }
}

impl QuickStep {
    pub fn new(config: QuickStepConfig) -> QuickStep {
        let QuickStepConfig {
            path,
            inner_node_upper_bound,
            leaf_upper_bound,
            cache_size_lg,
        } = config;

        let data_path = resolve_data_path(&path);

        let io_engine =
            IoEngine::open(&data_path).expect("failed to open quickstep data file for writing");
        let cache = MiniPageBuffer::new(cache_size_lg);

        let mut quickstep = QuickStep {
            inner_nodes: BPTree::new(inner_node_upper_bound),
            cache,
            io_engine,
            map_table: MapTable::new(leaf_upper_bound),
        };

        quickstep.ensure_root_leaf_on_disk();

        // initialise root leaf (page 0 for now)
        let root_page = quickstep.map_table.init_leaf_entry(0);
        quickstep.inner_nodes.set_leaf_root(root_page);

        quickstep
    }

    /// Create a new transaction for isolated operations
    pub fn tx(&self) -> QuickStepTx<'_> {
        // coordination is done via the locks so it can just hold a reference to the db
        QuickStepTx {
            db: self,
            lock_manager: LockManager::new(),
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
}

pub struct QuickStepTx<'db> {
    db: &'db QuickStep,
    lock_manager: LockManager<'db>,
    // changes for rollback
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
        // find leaf, keep track of those that would need to be written to in a split
        let res = self.db.inner_nodes.read_traverse_leaf(key)?;

        // We've found the page now get a write lock, keeping in mind we might already have one of some kind
        let mut page_guard = self
            .lock_manager
            .get_upgrade_or_acquire_write_lock(&self.db.map_table, res.page)?;

        match Self::try_put_with_promotion(self.db, &mut page_guard, key, val)? {
            TryPutResult::Success => return Ok(()),
            TryPutResult::NeedsSplit => {
                // We know which locks we need, so try to acquire them, if we fail then it might
                // be because another thread modified the tree which we weren't looking, so we should restart
                let split_plan = Self::plan_leaf_split(self.db, &mut page_guard);

                let lock_bundle =
                    self.db
                        .inner_nodes
                        .write_lock(res.overflow_point, OpType::Split, key);

                let mut lock_bundle = match lock_bundle {
                    Ok(l) => l,
                    Err(e) => return Err(e),
                };

                let mut right_guard = self.new_mini_page(NodeSize::LeafPage, None)?;

                let split_outcome = Self::apply_leaf_split(
                    self.db,
                    &mut page_guard,
                    &mut right_guard,
                    &split_plan,
                )?;

                debug::record_split_event(
                    page_guard.page_id().0,
                    right_guard.page_id().0,
                    split_outcome.pivot_key.clone(),
                    split_outcome.left_count,
                    split_outcome.right_count,
                );

                self.insert_into_parents_after_leaf_split(
                    &mut lock_bundle,
                    page_guard.page_id(),
                    &split_outcome.pivot_key,
                    right_guard.page_id(),
                )?;

                let pivot = split_outcome.pivot_key.as_slice();
                let target_guard = if key >= pivot {
                    &mut right_guard
                } else {
                    &mut page_guard
                };

                match Self::try_put_with_promotion(self.db, target_guard, key, val)? {
                    TryPutResult::Success => return Ok(()),
                    TryPutResult::NeedsSplit => {
                        todo!("split cascading is not yet implemented");
                    }
                    TryPutResult::NeedsPromotion(_) => {
                        unreachable!("promotion handled before returning")
                    }
                }
            }
            TryPutResult::NeedsPromotion(_) => unreachable!("promotion handled before returning"),
        }
    }

    pub fn abort(self) {}

    pub fn commit(self) {}
}

fn resolve_data_path(path: &Path) -> PathBuf {
    if path.is_dir() || path.extension().is_none() {
        path.join("quickstep.db")
    } else {
        path.to_path_buf()
    }
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
                .evict(&self.db.map_table, &self.db.io_engine)?;
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
        let index = match page_guard.get_write_guard().node() {
            NodeRef::MiniPage(idx) => idx,
            NodeRef::Leaf(_) => unreachable!("mini page expected after promotion"),
        };
        let meta = unsafe { self.db.cache.get_meta_mut(index) };
        let removed = meta.remove_key(key);
        if !removed {
            return Ok(false);
        }
        if meta.user_entry_count() <= AUTO_MERGE_MIN_ENTRIES {
            self.try_auto_merge(page_guard.page_id())?;
        }
        Ok(true)
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
