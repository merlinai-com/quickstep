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
    btree::{BPRestart, BPRootInfo, BPTree, OpType},
    buffer::{MiniPageBuffer, MiniPageIndex},
    error::QSError,
    io_engine::IoEngine,
    lock_manager::{LockManager, PageGuard, WriteGuardWrapper},
    map_table::{MapTable, PageReadGuard},
    node::InsufficientSpace,
    page_op::TryPutResult,
    types::{NodeMeta, NodeSize},
};

pub mod btree;
pub mod buffer;
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
    // TODO: return option of slice, representing if there was a value overwritten
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
                let _locks = self
                    .db
                    .inner_nodes
                    .write_lock(res.overflow_point, OpType::Split, key);

                let _new_guard = self.new_mini_page(NodeSize::LeafPage, None);

                todo!("leaf splitting is not yet implemented");
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
    fn try_put_with_promotion<'guard>(
        db: &'db QuickStep,
        page_guard: &mut WriteGuardWrapper<'guard, 'db>,
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
        page_guard: &mut WriteGuardWrapper<'_, 'db>,
        disk_addr: u64,
    ) -> Result<(), QSError> {
        let disk_leaf = page_guard.load_leaf(&db.io_engine, disk_addr)?;
        let cache_index = db
            .cache
            .alloc(NodeSize::LeafPage)
            .ok_or(QSError::CacheExhausted)?;

        unsafe {
            let dst = db.cache.get_meta_ptr(cache_index) as *mut u8;
            let src = disk_leaf.as_ref() as *const NodeMeta as *const u8;
            ptr::copy_nonoverlapping(src, dst, NodeSize::LeafPage.size_in_bytes());
            let mini_index = MiniPageIndex::new(cache_index);
            page_guard.get_write_guard().set_mini_page(mini_index);
        }

        Ok(())
    }

    fn new_mini_page<'tx>(
        &'tx mut self,
        size: NodeSize,
        disk_addr: Option<u64>,
    ) -> WriteGuardWrapper<'tx, 'db> {
        let new_mini_page = self.db.cache.alloc(size).expect("todo");

        unsafe { NodeMeta::init(self, new_mini_page, size, disk_addr) }
    }
}
