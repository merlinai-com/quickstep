use std::{collections::HashMap, marker::PhantomData, mem, ptr::NonNull};

use crate::{
    error::QSError,
    io_engine::{DiskLeaf, IoEngine},
    map_table::{MapTable, PageId, PageReadGuard, PageWriteGuard},
};

// TODO: optimise
pub struct LockManager<'a> {
    locks: HashMap<u64, Box<LockSlot<'a>>>,
}

struct LockSlot<'a> {
    guard: PageGuard<'a>,
    borrowed: bool,
}

impl<'a> LockSlot<'a> {
    fn new(guard: PageGuard<'a>) -> LockSlot<'a> {
        LockSlot {
            guard,
            borrowed: false,
        }
    }
}

// Locks need to be held for the length of the transaction

impl<'a> LockManager<'a> {
    pub fn new() -> LockManager<'a> {
        LockManager {
            locks: HashMap::new(),
        }
    }

    pub fn insert_write_lock(&mut self, guard: PageWriteGuard<'a>) -> WriteGuardWrapper<'a> {
        let id = guard.page.0;
        self.locks.insert(
            id,
            Box::new(LockSlot::new(PageGuard {
                guard_inner: GuardWrapper::Write(guard),
                leaf: None,
            })),
        );

        let slot = self
            .locks
            .get_mut(&id)
            .expect("We just inserted this value");

        WriteGuardWrapper::new(PageHandle::acquire(slot))
    }

    pub fn get_or_acquire_read_lock(
        &mut self,
        mapping_table: &'a MapTable,
        page: PageId,
    ) -> Result<&mut PageGuard<'a>, QSError> {
        // I tried entries, but couldn't get the lifetimes to work
        if !self.locks.contains_key(&page.0) {
            let guard: PageReadGuard<'a> = mapping_table.read_page_entry(page)?;

            self.locks.insert(
                page.0,
                Box::new(LockSlot::new(PageGuard {
                    guard_inner: GuardWrapper::Read(guard),
                    leaf: None,
                })),
            );
        }

        let slot = self
            .locks
            .get_mut(&page.0)
            .expect("We just ensured that it exists");

        Ok(&mut slot.guard)
    }

    pub fn get_upgrade_or_acquire_write_lock(
        &mut self,
        mapping_table: &'a MapTable,
        page: PageId,
    ) -> Result<WriteGuardWrapper<'a>, QSError> {
        if !self.locks.contains_key(&page.0) {
            let guard = mapping_table.write_page_entry(page)?;

            self.locks.insert(
                page.0,
                Box::new(LockSlot::new(PageGuard {
                    guard_inner: GuardWrapper::Write(guard),
                    leaf: None,
                })),
            );
        }

        let slot = self
            .locks
            .get_mut(&page.0)
            .expect("we just added it if it didn't exist");

        slot.guard.ensure_write()?;

        Ok(WriteGuardWrapper::new(PageHandle::acquire(slot)))
    }
}

pub enum GuardWrapper<'a> {
    Write(PageWriteGuard<'a>),
    Read(PageReadGuard<'a>),
}

pub struct PageHandle<'a> {
    slot: NonNull<LockSlot<'a>>,
    _marker: PhantomData<&'a mut LockSlot<'a>>,
}

impl<'a> PageHandle<'a> {
    fn acquire(slot: &mut LockSlot<'a>) -> PageHandle<'a> {
        debug_assert!(
            !slot.borrowed,
            "Attempted to borrow the same page guard twice"
        );
        slot.borrowed = true;
        PageHandle {
            slot: NonNull::from(slot),
            _marker: PhantomData,
        }
    }

    fn guard(&self) -> &PageGuard<'a> {
        unsafe { &self.slot.as_ref().guard }
    }

    fn guard_mut(&mut self) -> &mut PageGuard<'a> {
        unsafe { &mut self.slot.as_mut().guard }
    }
}

impl<'a> Drop for PageHandle<'a> {
    fn drop(&mut self) {
        unsafe {
            let slot = self.slot.as_mut();
            debug_assert!(slot.borrowed, "PageHandle dropped twice");
            slot.borrowed = false;
        }
    }
}

/// Inner page guard is guaranteed to be a write
pub struct WriteGuardWrapper<'a>(PageHandle<'a>);

impl<'a> WriteGuardWrapper<'a> {
    fn new(handle: PageHandle<'a>) -> WriteGuardWrapper<'a> {
        WriteGuardWrapper(handle)
    }

    pub fn page_id(&self) -> PageId {
        self.0.guard().page_id()
    }

    fn guard_mut(&mut self) -> &mut PageGuard<'a> {
        self.0.guard_mut()
    }

    pub fn get_write_guard<'b>(&'b mut self) -> &'b mut PageWriteGuard<'a> {
        match self.guard_mut().guard_inner {
            GuardWrapper::Write(ref mut g) => g,
            GuardWrapper::Read(_) => {
                unreachable!("WritePageGuard guarantees that we hold a write guard")
            }
        }
    }

    pub fn load_leaf<'b>(
        &'b mut self,
        io: &IoEngine,
        addr: u64,
    ) -> Result<&'b mut DiskLeaf, QSError> {
        self.guard_mut().load_leaf(io, addr)
    }
}

pub struct PageGuard<'a> {
    pub guard_inner: GuardWrapper<'a>,
    pub leaf: Option<DiskLeaf>,
}

impl<'a> PageGuard<'a> {
    pub fn is_write(&self) -> bool {
        matches!(self.guard_inner, GuardWrapper::Write(_))
    }

    pub fn page_id(&self) -> PageId {
        match &self.guard_inner {
            GuardWrapper::Write(w) => w.page,
            GuardWrapper::Read(r) => r.page,
        }
    }

    pub fn load_leaf<'g>(
        &'g mut self,
        io: &IoEngine,
        addr: u64,
    ) -> Result<&'g mut DiskLeaf, QSError> {
        let leaf = match self.leaf {
            Some(ref mut l) => l,
            None => {
                let new_leaf = io.get_page(addr);
                self.leaf = Some(new_leaf);
                self.leaf.as_mut().expect("just set leaf to Some")
            }
        };
        Ok(leaf)
    }

    /// Upgrade to a write transaction, if not already
    /// If it fails it will
    pub fn ensure_write(&mut self) -> Result<(), QSError> {
        let write = match &mut self.guard_inner {
            GuardWrapper::Write(_) => return Ok(()),
            GuardWrapper::Read(g) => {
                let ptr = g as *mut PageReadGuard<'a>;
                // SAFETY: we have a mutable reference, and aren't going to touch the old value
                let read_guard = unsafe { ptr.read() };

                match read_guard.upgrade() {
                    Ok(w) => w,
                    Err((r, e)) => {
                        mem::forget(r);
                        // Just leave existing read guard in place
                        return Err(e);
                    }
                }
            }
        };
        self.guard_inner = GuardWrapper::Write(write);
        Ok(())
    }
}
impl<'a> GuardWrapper<'a> {
    // TODO: do this in a more elegant way
    /// Temporarily upgrade to write lock
    /// when this is dropped it will revert the guard to its original state
    pub fn temp_upgrade<'tx>(&'tx mut self) -> Result<TmpPageWrite<'tx, 'a>, QSError> {
        if let GuardWrapper::Write(ref mut w) = self {
            return Ok(TmpPageWrite::WriteOriginal(w));
        }

        let wrapper_ptr = self as *mut GuardWrapper<'a>;

        let GuardWrapper::Read(read_ref) = self else {
            unreachable!("We just checked for the write case")
        };

        let read_guard = unsafe { (read_ref as *const PageReadGuard<'a>).read() };
        match read_guard.upgrade() {
            Ok(w) => {
                unsafe { wrapper_ptr.write(GuardWrapper::Write(w)) };

                let GuardWrapper::Write(guard) = self else {
                    unreachable!("We just wrote as a Write")
                };

                return Ok(TmpPageWrite::ReadOriginal {
                    guard,
                    original_location: wrapper_ptr,
                });
            }
            Err((r, e)) => {
                mem::forget(r);
                return Err(e);
            }
        }
    }
}

pub enum TmpPageWrite<'tx, 'a> {
    WriteOriginal(&'tx mut PageWriteGuard<'a>),
    ReadOriginal {
        guard: &'tx mut PageWriteGuard<'a>,
        original_location: *mut GuardWrapper<'a>,
    },
}

impl<'tx, 'a> TmpPageWrite<'tx, 'a> {
    pub fn as_guard(&mut self) -> &mut PageWriteGuard<'a> {
        match self {
            TmpPageWrite::WriteOriginal(g) => g,
            TmpPageWrite::ReadOriginal { guard, .. } => guard,
        }
    }
}

impl<'tx, 'a> Drop for TmpPageWrite<'tx, 'a> {
    fn drop(&mut self) {
        let TmpPageWrite::ReadOriginal {
            guard: guard_ref,
            original_location,
        } = self
        else {
            return;
        };
        // SAFETY: we have a mutable reference, and this memory won't be read again
        let write_guard = unsafe { (*guard_ref as *mut PageWriteGuard<'a>).read() };
        let read_guard = write_guard.downgrade();

        unsafe {
            original_location.write(GuardWrapper::Read(read_guard));
        }
    }
}
