use std::{
    alloc::{alloc, Layout},
    marker::PhantomData,
    num::{NonZero, NonZeroU16},
    ptr::NonNull,
    sync::{
        atomic::{AtomicU32, AtomicU64, Ordering},
        RwLock,
    },
    u32, u64,
};

use crate::{map_table::PageId, types::NodeSize, SPIN_RETRIES};

/// Max length of key in bytes
const MAX_KEY_LENGTH: usize = 64;

pub struct BPTree {
    slab: NonNull<u8>,
    cap: u32,
    /// level | node id
    root: AtomicU64,
    next_free: AtomicU32,
    free_list: AtomicU32,
}

impl BPTree {
    pub fn new(inner_node_upper_bound: u32) -> BPTree {
        let memory_req = inner_node_upper_bound * 4096;

        let layout = Layout::from_size_align(memory_req as usize, 4096).expect("todo");

        let slab_ptr = unsafe { alloc(layout) };

        let slab = match NonNull::new(slab_ptr) {
            Some(p) => p,
            None => todo!("todo: handle OOM"),
        };

        // TODO initialise first node

        BPTree {
            slab,
            cap: inner_node_upper_bound,
            root: AtomicU64::new(1 << 32),
            next_free: AtomicU32::new(1),
            free_list: AtomicU32::new(u32::MAX),
        }
    }

    pub fn get_root(&self) -> BPRootInfo {
        let info = self.root.load(Ordering::Acquire);
        // let node = BPNodeId(info as u32);
        let level = (info >> 48) as u16;
        // BPRootInfo { level, node }

        const PAGE_MASK: u64 = (1 << 48) - 1;
        match NonZeroU16::new(level) {
            Some(level) => BPRootInfo::Inner {
                level,
                node: BPNodeId(info as u32),
            },
            None => BPRootInfo::Leaf(PageId(info & PAGE_MASK)),
        }
    }

    pub fn read_inner(&self, node: BPNodeId) -> InnerReadGuard {
        todo!()
    }
    pub fn write_inner(&self, node: BPNodeId) -> InnerWriteGuard {
        todo!()
    }

    pub fn read_traverse_leaf(&self, key: &[u8]) -> PageId {
        'restart: for _ in 0..SPIN_RETRIES {
            let (level, node) = match self.get_root() {
                BPRootInfo::Leaf(page_id) => return page_id,
                BPRootInfo::Inner { level, node } => (level.get(), node),
            };

            let mut parent_level = level;
            let mut parent_guard = self.read_inner(node);

            while parent_level > 1 {
                // // SAFETY: level of parent > 1
                let cur_node = unsafe { parent_guard.as_ref().search_for_inner(key) };
                let cur_guard = self.read_inner(cur_node);

                if let Err(BPRestart) = parent_guard.unlock_or_restart() {
                    continue 'restart;
                }

                parent_guard = cur_guard;
                parent_level -= 1;
            }
            debug_assert!(parent_level == 1);

            let leaf_cand = unsafe { parent_guard.as_ref().search_for_leaf(key) };

            if let Err(BPRestart) = parent_guard.unlock_or_restart() {
                continue 'restart;
            }
            return leaf_cand;
        }
        todo!()
    }
}

#[repr(transparent)]
pub struct BPNodeId(u32);

pub enum BPRootInfo {
    Leaf(PageId),
    Inner {
        // TODO: check that u16 is sufficient for level
        level: NonZeroU16,
        node: BPNodeId,
    },
}

pub struct InnerReadGuard<'a> {
    version: u64,
    node: NonNull<BPNode>,
    _marker: PhantomData<&'a BPNode>,
}

impl<'a> InnerReadGuard<'a> {
    pub fn as_ref(&self) -> &BPNode {
        unsafe { self.node.as_ref() }
    }

    pub fn upgrade(&self) -> Result<InnerWriteGuard, BPRestart> {
        let new_version = self.version + 0b10;
        match self.as_ref().vlock.compare_exchange_weak(
            self.version,
            new_version,
            Ordering::Acquire,
            Ordering::Relaxed,
        ) {
            Ok(_) => Ok(InnerWriteGuard {
                node: unsafe { &mut *(self.node.as_ptr()) },
            }),
            Err(_v) => Err(BPRestart),
        }
    }

    pub fn unlock_or_restart(self) -> Result<(), BPRestart> {
        self.check_or_restart()
    }

    pub fn check_or_restart(&self) -> Result<(), BPRestart> {
        let nv = self.as_ref().vlock.load(Ordering::Acquire);

        match self.version == nv {
            true => Ok(()),
            false => Err(BPRestart),
        }
    }
}

pub struct InnerWriteGuard<'a> {
    node: &'a mut BPNode,
}

impl<'a> InnerWriteGuard<'a> {
    pub fn as_ref(&self) -> &BPNode {
        &self.node
    }

    pub fn as_mut(&mut self) -> &mut BPNode {
        self.node
    }
}

impl<'a> Drop for InnerWriteGuard<'a> {
    fn drop(&mut self) {
        self.node.vlock.fetch_add(0b10, Ordering::Release);
    }
}

/// | vlock | count | alloc idx |lowest child | KVMeta ...   ... Full keys |
///    8B      4B          4B           8B             8B   ...
///                                             4072B
// NOTE: this is inefficient use of memory, but I want to keep everything word aligned
// so this is easier, but more information can easily be squeesed in, (at least 32 bit)
pub struct BPNode {
    vlock: AtomicU64,
    count: u32,
    alloc_idx: u32,
    // all 1s for None
    lowest: u64,
    rest: [u8; INLINE_BUFFER_LEN],
}

const INLINE_BUFFER_LEN: usize = 4072;

const _: () = assert!(size_of::<BPNode>() == 4096);

impl BPNode {
    unsafe fn init(mem: NonNull<[u8; 4096]>) {
        let node_ptr = mem.as_ptr() as *mut BPNode;

        node_ptr.write(BPNode {
            vlock: AtomicU64::new(0),
            count: 0,
            alloc_idx: 4095,
            lowest: u64::MAX,
            rest: [0; INLINE_BUFFER_LEN],
        });
    }

    pub fn capacity(&self) -> usize {
        todo!()
    }

    // SAFETY: This method should only be called on nodes with height > 1
    pub unsafe fn search_for_inner(&self, key: &[u8]) -> BPNodeId {
        todo!()
    }

    // SAFETY: This method should only be called on nodes with height = 1
    pub unsafe fn search_for_leaf(&self, key: &[u8]) -> PageId {
        todo!()
    }
}

// impl BPNode {
//     fn read_lock_or_restart(&self) -> Result<u64, BPRestart> {
//         let version = self.await_node_unlock();
//         if BPNode::is_obsolete(version) {
//             return Err(BPRestart);
//         }

//         return Ok(version);
//     }

//     fn read_unlock_or_restart(&self, version: u64) -> Result<(), BPRestart> {
//         self.check_or_restart(version)
//     }

//     fn check_or_restart(&self, version: u64) -> Result<(), BPRestart> {
//         let new_version = self.vlock.load(Ordering::Acquire);

//         match new_version == version {
//             true => Ok(()),
//             false => Err(BPRestart),
//         }
//     }

//     fn upgrade_to_write_or_restart(&self, version: u64) -> Result<(), BPRestart> {
//         match self.vlock.compare_exchange(
//             version,
//             Self::set_locked_bit(version),
//             Ordering::Acquire,
//             Ordering::Relaxed,
//         ) {
//             Ok(_) => Ok(()),
//             Err(_) => Err(BPRestart),
//         }
//     }

//     fn write_lock_or_restart(&self) -> Result<(), BPRestart> {
//         let mut version = self.read_lock_or_restart()?;
//         while self.upgrade_to_write_or_restart(version).is_err() {
//             version = self.read_lock_or_restart()?;
//         }
//         Ok(())
//     }

//     fn write_unlock(&self) {
//         self.vlock.fetch_add(1, Ordering::Release);
//     }

//     fn write_unlock_obselete(&self) {
//         self.vlock.fetch_add(3, Ordering::Release);
//     }

//     // Helper functions

//     fn await_node_unlock(&self) -> u64 {
//         let mut version = self.vlock.load(Ordering::Acquire);
//         while (version & 2) == 2 {
//             std::hint::spin_loop();
//             version = self.vlock.load(Ordering::Acquire);
//         }
//         return version;
//     }

//     fn set_locked_bit(version: u64) -> u64 {
//         version + 2
//     }

//     fn is_obsolete(version: u64) -> bool {
//         (version & 1) == 1
//     }

//     fn is_locked_or_obsolete(version: u64) -> bool {
//         (version & 0b11) != 0
//     }
// }

// struct BPKVMeta {
//     lookahead: u16,
//     offset:
// }

pub struct BPRestart;
