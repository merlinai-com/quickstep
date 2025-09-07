use std::path::{Path, PathBuf};

use crate::{
    btree::{BPRestart, BPRootInfo, BPTree},
    buffer::MiniPageBuffer,
    error::QSError,
    io_engine::IoEngine,
    map_table::{MapTable, PageReadGuard},
};

pub mod btree;
pub mod buffer;
pub mod error;
pub mod io_engine;
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
        QuickStepTx { db: self }
    }
}

pub struct QuickStepTx<'db> {
    db: &'db QuickStep,
    read_locks: Vec<PageReadGuard>,
    write_locks: Vec,
    // changes for rollback
}

impl<'db> QuickStepTx<'db> {
    pub fn get<'tx>(&'tx self, key: &[u8]) -> Result<Option<&'tx [u8]>, QSError> {
        let page = self.db.inner_nodes.read_traverse_leaf(key);

        // lock_manager get or upgrade

        let page_guard = self.db.map_table.read_page_entry(page);

        let res = page_guard.get(&self.db.cache, &self.db.io_engine, key);

        // TODO: add page_guard to lock_manager

        Ok(res)
    }
}
