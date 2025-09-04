use std::sync::{
    atomic::{AtomicUsize, Ordering},
    RwLock,
};

use crate::{buffer::MiniPageIndex, types::NodeRef, SPIN_RETRIES};

///Needs to be initialised with at least one
pub struct MapTable {
    indirection_arr: Box<[PageEntry]>,
    next_free: AtomicUsize,
    last_active: AtomicUsize,
}

impl MapTable {
    pub fn create_page_entry(&self, node: MiniPageIndex) -> PageWriteGuard {
        let target_idx = self.next_free.fetch_add(1, Ordering::AcqRel);

        let val = PageEntry::new(node);

        let target_ptr = &self.indirection_arr[target_idx] as *const PageEntry as *mut PageEntry;
        // we haven't moved the active pointer, so nobody else can access this value
        unsafe {
            target_ptr.write(val);
        }

        let prev = target_idx - 1;
        for _ in 0..SPIN_RETRIES {
            match self.last_active.compare_exchange_weak(
                prev,
                target_idx,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    return PageWriteGuard {
                        map_table: self,
                        index: target_idx,
                        node: NodeRef::MiniPage(node),
                    }
                }
                Err(_) => std::hint::spin_loop(),
            }
        }

        todo!("Handle case where we did too many retries")
    }

    pub fn read_page_entry(&self, page: PageId) -> PageReadGuard {
        todo!()
    }

    pub fn write_page_entry(&self, page: PageId) -> PageWriteGuard {
        todo!()
    }
}

pub struct PageId(u64);

pub struct PageReadGuard<'a> {
    map_table: &'a MapTable,
    index: usize,
    pub node: NodeRef,
}

impl<'a> Drop for PageReadGuard<'a> {
    fn drop(&mut self) {
        todo!("decrement writer lock")
    }
}

pub struct PageWriteGuard<'a> {
    map_table: &'a MapTable,
    index: usize,
    pub node: NodeRef,
}

impl<'a> Drop for PageWriteGuard<'a> {
    fn drop(&mut self) {
        todo!("release writer lock")
    }
}

/// | address |
#[repr(transparent)]
struct PageEntry(u64);

impl PageEntry {
    fn new(node: MiniPageIndex) -> PageEntry {
        let repr = node.0 << 16;
        // TODO: add writer bit, so its created in an exlusive locked state
        PageEntry(repr)
    }
}
