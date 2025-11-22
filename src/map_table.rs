use std::{
    alloc::{alloc, Layout},
    f64::consts::E,
    iter::Map,
    marker::PhantomData,
    ptr::{self, NonNull},
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        RwLock,
    },
};

use crate::{buffer::MiniPageIndex, error::QSError, types::NodeRef, SPIN_RETRIES};

///Needs to be initialised with at least one
pub struct MapTable {
    indirection_arr: NonNull<AtomicU64>,
    /// first node in the free list,  usize::MAX if none
    next_free: AtomicUsize,
    cap: usize,
}

impl MapTable {
    pub fn new(leaf_upper_bound: u64) -> MapTable {
        let layout = Layout::array::<u64>(leaf_upper_bound as usize).expect("todo");

        let ptr = unsafe { alloc(layout) };

        let arr = match NonNull::new(ptr as *mut AtomicU64) {
            Some(p) => p,
            None => todo!("todo: handle OOM"),
        };

        MapTable {
            indirection_arr: arr,
            next_free: AtomicUsize::new(0),
            cap: leaf_upper_bound as usize,
        }
    }
}

impl MapTable {
    pub fn init_leaf_entry(&self, disk_addr: u64) -> PageId {
        if self.cap == 0 {
            todo!("map table capacity must be > 0");
        }

        let entry = PageEntry::leaf(disk_addr);
        unsafe {
            let ptr = self.indirection_arr.as_ptr();
            ptr.write(AtomicU64::new(entry.to_repr()));
        }

        self.next_free.store(1, Ordering::Release);

        PageId(0)
    }

    pub fn create_page_entry(&self, node: MiniPageIndex) -> PageWriteGuard<'_> {
        let target_idx = self.next_free.fetch_add(1, Ordering::AcqRel);

        if target_idx >= self.cap {
            todo!("handle excessive pages")
        }

        let val = PageEntry::new_write_locked(node);

        // We have exclusive access, as the end pointer has been advanced, but the page id hasn't been returned
        unsafe {
            self.indirection_arr
                .offset(target_idx as isize)
                .write(AtomicU64::new(val.clone().to_repr()));
        }

        PageWriteGuard {
            map_table: self,
            page: PageId(target_idx as u64),
            node: val,
        }
    }

    pub fn read_page_entry(&self, page: PageId) -> Result<PageReadGuard<'_>, QSError> {
        let entry_ref = self.get_ref(page);
        let mut entry = PageEntry::from_repr(entry_ref.load(Ordering::Acquire));

        for _ in 0..SPIN_RETRIES {
            if entry.pending_write() {
                std::hint::spin_loop();
                continue;
            }

            let lock_state = entry.state();

            if lock_state >= WRITE_LOCK_STATE {
                // Write lock is currently held
                std::hint::spin_loop();
                entry = PageEntry(entry_ref.load(Ordering::Acquire));
            } else {
                // Reader locked or unlocked

                let new = entry.clone().set_state(lock_state + 1);

                match entry_ref.compare_exchange_weak(
                    entry.to_repr(),
                    new.to_repr(),
                    Ordering::Acquire,
                    Ordering::Relaxed,
                ) {
                    Ok(e) => {
                        return Ok(PageReadGuard {
                            map_table: self,
                            page,
                            node: PageEntry(e),
                        })
                    }
                    Err(e) => entry = PageEntry(e),
                }
            }
        }

        Err(QSError::PageLockFail)
    }

    // TODO: refactor to take read lock and upgrade
    pub fn write_page_entry(&self, page: PageId) -> Result<PageWriteGuard<'_>, QSError> {
        let entry_ref = self.get_ref(page);
        let mut entry = PageEntry(entry_ref.load(Ordering::Acquire));

        for _ in 0..SPIN_RETRIES {
            let lock_state = entry.state();
            match lock_state {
                0 => {
                    let new = entry
                        .clone()
                        .set_state(WRITE_LOCK_STATE)
                        .set_pending_write(false);
                    match entry_ref.compare_exchange_weak(
                        entry.to_repr(),
                        new.to_repr(),
                        Ordering::Acquire,
                        Ordering::Relaxed,
                    ) {
                        Ok(e) => {
                            return Ok(PageWriteGuard {
                                map_table: self,
                                page,
                                node: PageEntry(e),
                            })
                        }
                        Err(e) => entry = PageEntry(e),
                    }
                }
                _ => {
                    if !entry.pending_write() {
                        let new = entry.clone().set_pending_write(true);
                        let ev = entry_ref
                            .compare_exchange_weak(
                                entry.to_repr(),
                                new.to_repr(),
                                Ordering::Relaxed,
                                Ordering::Relaxed,
                            )
                            .unwrap_or_else(|e| e);
                        entry = PageEntry(ev);
                        continue;
                    }

                    std::hint::spin_loop();
                    entry = PageEntry(entry_ref.load(Ordering::Relaxed));
                }
            }
        }

        Err(QSError::PageLockFail)
    }

    fn get_ref(&self, page: PageId) -> &AtomicU64 {
        // Safety pageid was created pointing to a valid entry
        unsafe { self.indirection_arr.offset(page.0 as isize).as_ref() }
    }
}

/// An id of a leaf page, representing an index into the mapping table
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageId(pub(crate) u64);

impl PageId {
    pub fn as_u64(&self) -> u64 {
        self.0
    }

    pub fn from_u64(id: u64) -> PageId {
        PageId(id)
    }
}

pub struct PageReadGuard<'a> {
    map_table: &'a MapTable,
    pub page: PageId,
    node: PageEntry,
}

impl<'a> PageReadGuard<'a> {
    pub fn node<'g>(&'g self) -> NodeRef<'g> {
        self.node.get_ref()
    }
}

impl<'a> PageReadGuard<'a> {
    pub fn upgrade(self) -> Result<PageWriteGuard<'a>, (PageReadGuard<'a>, QSError)> {
        let map_table = self.map_table;
        let page = self.page;
        let node = self.node.clone();

        std::mem::forget(self);

        let entry_ref = map_table.get_ref(page);
        let mut entry = PageEntry(entry_ref.load(Ordering::Relaxed));
        for _ in 0..SPIN_RETRIES {
            match entry.state() {
                // 1 means that we're the only reader, so we can upgrade to writer
                1 => {
                    let new = entry.clone().set_state(WRITE_LOCK_STATE);
                    // not weak because we don't want someone else to intercept
                    match entry_ref.compare_exchange(
                        entry.to_repr(),
                        new.to_repr(),
                        Ordering::Acquire,
                        Ordering::Relaxed,
                    ) {
                        Ok(_) => {
                            return Ok(PageWriteGuard {
                                map_table,
                                page,
                                node,
                            })
                        }
                        Err(e) => entry = PageEntry(e),
                    }
                }
                // TODO: set writer waiting bit
                _ => {
                    std::hint::spin_loop();
                    entry = PageEntry(entry_ref.load(Ordering::Relaxed));
                }
            }
        }

        // TODO: try to unset writer waiting bit

        let original_guard = PageReadGuard {
            map_table,
            page,
            node,
        };

        Err((original_guard, QSError::PageLockFail))
    }
}

impl<'a> Drop for PageReadGuard<'a> {
    fn drop(&mut self) {
        let entry_ref = self.map_table.get_ref(self.page);
        let mut entry = PageEntry(entry_ref.load(Ordering::Relaxed));

        loop {
            let old_state = entry.state();
            let new = entry.clone().set_state(old_state - 1);
            match entry_ref.compare_exchange_weak(
                entry.to_repr(),
                new.to_repr(),
                Ordering::Release,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(e) => entry = PageEntry(e),
            }
        }
    }
}

pub struct PageWriteGuard<'a> {
    map_table: &'a MapTable,
    pub page: PageId,
    node: PageEntry,
}

impl<'a> PageWriteGuard<'a> {
    pub fn node<'g>(&'g self) -> NodeRef<'g> {
        self.node.get_ref()
    }

    pub fn set_mini_page(&mut self, mini_page: MiniPageIndex<'a>) {
        let entry = PageEntry::new_write_locked(mini_page);
        let entry_ref = self.map_table.get_ref(self.page);
        entry_ref.store(entry.to_repr(), Ordering::Release);
        self.node = entry;
    }

    pub fn set_leaf(&mut self, disk_addr: u64) {
        let entry = PageEntry::leaf(disk_addr);
        let entry_ref = self.map_table.get_ref(self.page);
        entry_ref.store(entry.to_repr(), Ordering::Release);
        self.node = entry;
    }

    // pub fn node_mut<'g>(&'g mut self)
}

impl<'a> PageWriteGuard<'a> {
    /// Cache the given key and value, without doing any resizing
    /// This should not invalidate any existing slices into the Node
    pub fn cache_no_alloc(&mut self, key: &[u8], value: &[u8]) {
        todo!()
    }
}

impl<'a> PageWriteGuard<'a> {
    /// Downgrade a write guard to a readguard, this should only be used after copy-on-access or
    pub fn downgrade(self) -> PageReadGuard<'a> {
        let map_table = self.map_table;
        let page = self.page;
        let node = self.node.clone();

        std::mem::forget(self);

        let entry_ref = map_table.get_ref(page);
        let entry = PageEntry(entry_ref.load(Ordering::Relaxed));
        let entry = entry.set_state(1);

        // Blind write is fine because we had write lock
        // the only concurrent modification could be setting writer pending
        entry_ref.store(entry.to_repr(), Ordering::Release);

        PageReadGuard {
            map_table,
            page,
            node,
        }
    }
}

impl<'a> Drop for PageWriteGuard<'a> {
    fn drop(&mut self) {
        let entry_ref = self.map_table.get_ref(self.page);
        let mut entry = PageEntry(entry_ref.load(Ordering::Relaxed));

        loop {
            let new = entry.clone().set_state(0);
            match entry_ref.compare_exchange_weak(
                entry.to_repr(),
                new.to_repr(),
                Ordering::Release,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(e) => entry = PageEntry(e),
            }
        }
    }
}

/// | address | is_leaf | write pending | lock state
///     48b      1b           1b            14b
// TODO: option to wait on two 32bit parts using futex
#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct PageEntry(u64);

const WRITE_LOCK_STATE: u16 = (1 << 14) - 1;
const _: () = assert!(WRITE_LOCK_STATE.count_ones() == 14);

impl PageEntry {
    fn new_write_locked<'g>(node: MiniPageIndex<'g>) -> PageEntry {
        let repr = node.index << 16;
        PageEntry(repr as u64).set_state(WRITE_LOCK_STATE)
    }

    fn leaf(addr: u64) -> PageEntry {
        let repr = (addr << 16) | (1 << 15);
        PageEntry(repr)
    }

    fn to_repr(self) -> u64 {
        self.0
    }

    fn from_repr(val: u64) -> PageEntry {
        PageEntry(val)
    }

    pub fn get_ref(&self) -> NodeRef<'_> {
        let repr = self.0;

        let is_leaf = (repr >> 15) & 1 == 1;
        let addr = repr >> 16;

        match is_leaf {
            true => NodeRef::Leaf(addr),
            false => NodeRef::MiniPage(MiniPageIndex {
                index: addr as usize,
                _marker: std::marker::PhantomData,
            }),
        }
    }

    pub fn state(&self) -> u16 {
        self.0 as u16 & WRITE_LOCK_STATE
    }

    fn set_state(mut self, new_state: u16) -> PageEntry {
        const CLEAR_MASK: u64 = !(WRITE_LOCK_STATE as u64);
        self.0 = (self.0 & CLEAR_MASK) | new_state as u64;
        self
    }

    fn pending_write(&self) -> bool {
        ((self.0 >> 14) & 1) == 1
    }

    fn set_pending_write(mut self, new: bool) -> PageEntry {
        self.0 &= !(1 << 14);
        self.0 |= (new as u64) << 14;
        self
    }
}
