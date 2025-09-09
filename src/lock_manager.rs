use std::collections::HashMap;

use crate::{
    error::QSError,
    io_engine::DiskLeaf,
    map_table::{MapTable, PageId, PageReadGuard, PageWriteGuard},
};

// TODO: optimise
pub struct LockManager<'a> {
    locks: HashMap<u64, PageGuard<'a>>,
}

// Locks need to be held for the length of the transaction

impl<'a> LockManager<'a> {
    pub fn new() -> LockManager<'a> {
        LockManager {
            locks: HashMap::new(),
        }
    }

    pub fn get_or_acquire_read_lock(
        &mut self,
        mapping_table: &'a MapTable,
        page: PageId,
    ) -> Result<&mut PageGuard<'a>, QSError> {
        // I tried entries, but couldn't get the lifetimes to work
        if !self.locks.contains_key(&page.0) {
            let guard: PageReadGuard<'a> = mapping_table.read_page_entry(page);

            self.locks.insert(
                page.0,
                PageGuard {
                    guard_inner: GuardWrapper::Read(guard),
                    leaf: None,
                },
            );
        }

        Ok(self
            .locks
            .get_mut(&page.0)
            .expect("We just ensured that it exists"))
    }
}

pub enum GuardWrapper<'a> {
    Write(PageWriteGuard<'a>),
    Read(PageReadGuard<'a>),
}

pub struct PageGuard<'a> {
    pub guard_inner: GuardWrapper<'a>,
    pub leaf: Option<DiskLeaf>,
}
