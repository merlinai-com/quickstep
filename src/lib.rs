use std::path::{Path, PathBuf};

use crate::{
    btree::{BPRestart, BPRootInfo, BPTree},
    buffer::MiniPageBuffer,
    error::QSError,
    io_engine::IoEngine,
    lock_manager::LockManager,
    map_table::{MapTable, PageReadGuard},
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

pub struct QuickStep {
    inner_nodes: BPTree,
    cache: MiniPageBuffer,
    io_engine: IoEngine,
    map_table: MapTable,
}

pub struct QuickStepConfig {
    path: PathBuf,
    inner_node_upper_bound: u32,
    leaf_upper_bound: u64,
    cache_size_lg: usize,
}

impl QuickStep {
    pub fn new(config: QuickStepConfig) -> QuickStep {
        todo!()
    }

    pub fn tx(&self) -> QuickStepTx {
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
        let page = self.db.inner_nodes.read_traverse_leaf(key);

        let page_guard = self
            .lock_manager
            .get_or_acquire_read_lock(&self.db.map_table, page)?;

        let res = page_guard.get(&self.db.cache, &self.db.io_engine, key)?;

        Ok(res)
    }

    /// Insert a value that does not already exist
    pub fn insert<'tx>(&'tx mut self, key: &[u8]) -> Result<(), QSError> {
        // find leaf, keep track of those that would need to be written to in a split
        //

        todo!()
    }
}
