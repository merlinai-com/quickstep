use std::{
    alloc::{alloc, Layout},
    marker::PhantomData,
    mem::size_of,
    num::NonZeroU16,
    ptr::NonNull,
    sync::atomic::{AtomicU32, AtomicU64, Ordering},
    u32, u64,
};

use crate::{
    error::QSError,
    map_table::PageId,
    utils::{extract_u32, extract_u48, store_u48},
    SPIN_RETRIES,
};

/// Max length of key in bytes
const MAX_KEY_LENGTH: usize = 64;

// TODO: prevent race condition when freeing nodes
pub struct BPTree {
    /// The buffer containing all nodes, allocated at initialisation
    slab: NonNull<BPNode>,
    /// The number of nodes we have capacity for in the above buffer
    cap: u32,
    /// The root node and level of the root
    /// If the level is 0 then its a 48bit pageid
    /// otherwise its a 32bit BP Tree index
    /// level | node id
    root: AtomicU64,
    /// The version lock for the pointer to the root node
    root_vlock: AtomicU64,
    /// index of next free node in the buffer
    next_free: AtomicU32,
    /// start of node free list, u32::MAX if empty
    free_list: AtomicU32,
}

impl BPTree {
    pub fn new(inner_node_upper_bound: u32) -> BPTree {
        let memory_req = inner_node_upper_bound * 4096;

        let layout = Layout::from_size_align(memory_req as usize, 4096).expect("todo");

        let slab_ptr = unsafe { alloc(layout) as *mut BPNode };

        let slab = match NonNull::new(slab_ptr) {
            Some(p) => p,
            None => todo!("todo: handle OOM"),
        };

        // TODO initialise first node

        BPTree {
            slab,
            cap: inner_node_upper_bound,
            root: AtomicU64::new(0),
            root_vlock: AtomicU64::new(0),
            next_free: AtomicU32::new(1),
            free_list: AtomicU32::new(u32::MAX),
        }
    }

    pub fn set_leaf_root(&mut self, page: crate::map_table::PageId) {
        self.root.store(page.0, Ordering::Release);
    }

    fn alloc_inner_node(&self) -> Result<BPNodeId, QSError> {
        loop {
            let idx = self.next_free.fetch_add(1, Ordering::AcqRel);
            if idx >= self.cap {
                return Err(QSError::TreeFull);
            }

            // SAFETY: idx < cap, slab points to a buffer of cap nodes
            let node_ptr = unsafe { self.slab.as_ptr().add(idx as usize) };
            // SAFETY: we have exclusive ownership of this slot because idx is unique
            unsafe {
                node_ptr.write(BPNode::blank());
            }
            return Ok(BPNodeId(idx));
        }
    }

    pub fn promote_leaf_root(
        &self,
        root_lock: &mut RootWriteLock<'_>,
        left_child: PageId,
        right_child: PageId,
        pivot_key: &[u8],
    ) -> Result<(), QSError> {
        let node_id = self.alloc_inner_node()?;

        unsafe {
            let node_ptr = self.slab.as_ptr().add(node_id.0 as usize);
            (*node_ptr).init_leaf_parent(left_child, right_child, pivot_key)?;
        }

        root_lock.set_inner(node_id, 1);
        Ok(())
    }

    pub fn read_root(&self) -> Result<RootReadLock<'_>, BPRestart> {
        let version = self.root_vlock.load(Ordering::Acquire);
        if is_locked_or_obsolete(version) {
            return Err(BPRestart);
        }
        Ok(RootReadLock {
            tree: self,
            version,
        })
    }

    /// Must have checked the root lock
    pub unsafe fn get_root(&self) -> BPRootInfo {
        let info = self.root.load(Ordering::Acquire);
        let level = (info >> 48) as u16;

        const PAGE_MASK: u64 = (1 << 48) - 1;
        match NonZeroU16::new(level) {
            Some(level) => BPRootInfo::Inner {
                level,
                node: BPNodeId(info as u32),
            },
            None => BPRootInfo::Leaf(PageId(info & PAGE_MASK)),
        }
    }

    pub fn read_inner(&self, node: BPNodeId) -> Result<InnerReadGuard<'_>, BPRestart> {
        let node = unsafe { self.slab.add(node.0 as usize).as_ref() };

        let version = node.vlock.load(Ordering::Acquire);

        match is_locked_or_obsolete(version) {
            true => Err(BPRestart),
            false => Ok(InnerReadGuard {
                version,
                node: node.into(),
                _marker: PhantomData,
            }),
        }
    }
    pub fn write_inner(&self, node: BPNodeId) -> Result<InnerWriteGuard<'_>, BPRestart> {
        let read = self.read_inner(node)?;
        read.upgrade()
    }

    pub fn read_traverse_leaf(&self, key: &[u8]) -> Result<ReadRes<'_>, QSError> {
        for _ in 0..SPIN_RETRIES {
            if let Ok(leaf) = self.try_read_traverse_leaf(key) {
                return Ok(leaf);
            }
        }
        Err(QSError::OLCRetriesExceeded)
    }

    fn try_read_traverse_leaf(&self, key: &[u8]) -> Result<ReadRes<'_>, BPRestart> {
        let root_guard = self.read_root()?;

        let mut underflow_point = WriteLockPoint::Root;
        let mut overflow_point = WriteLockPoint::Root;

        // SAFETY: we checked its not locked or obsolete
        let (level, node) = match unsafe { self.get_root() } {
            BPRootInfo::Leaf(page) => {
                return Ok(ReadRes {
                    page,
                    overflow_point,
                    underflow_point,
                    upper_fence_key: None,
                    lower_fence_key: None,
                })
            }
            BPRootInfo::Inner { level, node } => (level.get(), node),
        };

        let mut parent_level = level;
        let Ok(mut parent_guard) = self.read_inner(node) else {
            return Err(BPRestart);
        };

        root_guard.unlock_or_restart()?;

        update_lock_points(
            &parent_guard,
            parent_level,
            &mut overflow_point,
            &mut underflow_point,
        );

        while parent_level > 1 {
            // // SAFETY: level of parent > 1
            let cur_node = unsafe { parent_guard.as_ref().search_for_inner(key) };
            let cur_guard = self.read_inner(cur_node)?;

            parent_guard.unlock_or_restart()?;

            parent_guard = cur_guard;
            parent_level -= 1;

            update_lock_points(
                &parent_guard,
                parent_level,
                &mut overflow_point,
                &mut underflow_point,
            );
        }
        debug_assert!(parent_level == 1);

        let leaf_cand = unsafe { parent_guard.as_ref().search_for_leaf(key) };

        parent_guard.unlock_or_restart()?;
        return Ok(ReadRes {
            page: leaf_cand,
            overflow_point,
            underflow_point,
            lower_fence_key: None,
            upper_fence_key: None,
        });
    }

    pub fn write_lock<'a>(
        &'a self,
        point: WriteLockPoint<'a>,
        op_type: OpType,
        key: &[u8],
    ) -> Result<WriteLockBundle<'a>, QSError> {
        // try to lock from the existing point
        if let Ok(l) = self.lock_from_point(point, key) {
            return Ok(l);
        }

        for _ in 0..SPIN_RETRIES {
            let Ok(res) = self.try_read_traverse_leaf(key) else {
                continue;
            };

            let lock_point = match op_type {
                OpType::Split => res.overflow_point,
                OpType::Merge => res.underflow_point,
            };

            if let Ok(res) = self.lock_from_point(lock_point, key) {
                return Ok(res);
            };
        }

        Err(QSError::OLCRetriesExceeded)
    }

    pub fn lock_from_point<'a>(
        &'a self,
        point: WriteLockPoint<'a>,
        key: &[u8],
    ) -> Result<WriteLockBundle<'a>, BPRestart> {
        let mut root_lock: Option<RootWriteLock> = None;
        let mut acc = Vec::new();
        let (guard_opt, mut level) = match point {
            WriteLockPoint::Root => {
                let read_guard = self.read_root()?;
                let write_guard = read_guard.upgrade()?;
                match unsafe { self.get_root() } {
                    BPRootInfo::Leaf(_page_id) => {
                        return Ok(WriteLockBundle {
                            root_lock: Some(write_guard),
                            chain: vec![],
                        })
                    }
                    BPRootInfo::Inner { level, node } => {
                        root_lock = Some(write_guard);
                        let g = self.write_inner(node)?;
                        (Some(g), level.get())
                    }
                }
            }
            WriteLockPoint::Inner { guard, level } => {
                let first = guard.upgrade()?;
                (Some(first), level)
            }
        };

        let mut guard = match guard_opt {
            Some(g) => g,
            None => {
                return Ok(WriteLockBundle {
                    root_lock,
                    chain: acc,
                })
            }
        };

        while level > 1 {
            let next = unsafe { guard.as_ref().search_for_inner(key) };
            let next_guard = self.write_inner(next)?;
            // don't need to check because we have exclusive access to parent
            acc.push(guard);
            guard = next_guard;
            level -= 1;
        }

        acc.push(guard);

        Ok(WriteLockBundle {
            root_lock,
            chain: acc,
        })
    }

    pub fn debug_root_leaf_parent(&self) -> Option<DebugLeafParent> {
        let root_guard = self.read_root().ok()?;
        let info = root_guard.get_root();
        let snapshot = match info {
            BPRootInfo::Inner { level, node } if level.get() == 1 => {
                let inner_guard = self.read_inner(node).ok()?;
                let node_ref = inner_guard.as_ref();
                let mut pivots = Vec::with_capacity(node_ref.count as usize);
                let mut children = Vec::with_capacity(node_ref.count as usize + 1);
                children.push(PageId(node_ref.lowest));
                for idx in 0..node_ref.count {
                    pivots.push(node_ref.get_key(idx).to_vec());
                    children.push(node_ref.get_leaf_child(idx));
                }
                inner_guard.unlock_or_restart().ok()?;
                Some(DebugLeafParent { pivots, children })
            }
            _ => None,
        };
        root_guard.unlock_or_restart().ok()?;
        snapshot
    }
}

pub enum OpType {
    Split,
    Merge,
}

pub struct RootReadLock<'a> {
    tree: &'a BPTree,
    version: u64,
}

impl<'a> RootReadLock<'a> {
    pub fn get_root(&self) -> BPRootInfo {
        unsafe { self.tree.get_root() }
    }

    pub fn upgrade(&self) -> Result<RootWriteLock<'a>, BPRestart> {
        let new_version = self.version + 0b10;
        match self.tree.root_vlock.compare_exchange_weak(
            self.version,
            new_version,
            Ordering::Acquire,
            Ordering::Relaxed,
        ) {
            Ok(_) => Ok(RootWriteLock { tree: self.tree }),
            Err(_v) => Err(BPRestart),
        }
    }

    pub fn unlock_or_restart(self) -> Result<(), BPRestart> {
        self.check_or_restart()
    }

    pub fn check_or_restart(&self) -> Result<(), BPRestart> {
        let nv = self.tree.root_vlock.load(Ordering::Acquire);

        match self.version == nv {
            true => Ok(()),
            false => Err(BPRestart),
        }
    }
}

pub struct RootWriteLock<'a> {
    tree: &'a BPTree,
}

impl<'a> RootWriteLock<'a> {
    pub fn get_root(&self) -> BPRootInfo {
        unsafe { self.tree.get_root() }
    }

    fn set_inner(&mut self, node: BPNodeId, level: u16) {
        let encoded = ((level as u64) << 48) | node.0 as u64;
        self.tree.root.store(encoded, Ordering::Release);
    }
}

impl<'a> Drop for RootWriteLock<'a> {
    fn drop(&mut self) {
        self.tree.root_vlock.fetch_add(0b10, Ordering::Release);
    }
}

fn update_lock_points<'a>(
    guard: &InnerReadGuard<'a>,
    level: u16,
    overflow: &mut WriteLockPoint<'a>,
    underflow: &mut WriteLockPoint<'a>,
) {
    // If node can't split update overflow
    if !guard.as_ref().can_overflow(level) {
        *overflow = WriteLockPoint::Inner {
            guard: guard.clone(),
            level,
        }
    }
    // If node can't underflow update that
    if !guard.as_ref().will_underflow() {
        *underflow = WriteLockPoint::Inner {
            guard: guard.clone(),
            level,
        }
    }
}

#[repr(transparent)]
pub struct BPNodeId(u32);

pub struct ReadRes<'a> {
    /// Page where the target would be located
    pub page: PageId,
    /// Page to write lock from if a split is needed
    pub overflow_point: WriteLockPoint<'a>,
    /// Page to write lock from if a merge is needed
    pub underflow_point: WriteLockPoint<'a>,
    /// Lower fence key: a key that is less than or equal everything in the target page
    pub lower_fence_key: Option<(Box<[u8]>, PageId)>,
    /// Upper fence key: a key that is strictly greater than everything in target page
    pub upper_fence_key: Option<(Box<[u8]>, PageId)>,
}

pub enum WriteLockPoint<'a> {
    /// The operation requires locking from the root pointer
    Root,
    Inner {
        guard: InnerReadGuard<'a>,
        level: u16,
    },
}

pub struct WriteLockBundle<'a> {
    pub root_lock: Option<RootWriteLock<'a>>,
    pub chain: Vec<InnerWriteGuard<'a>>,
}

pub enum BPRootInfo {
    Leaf(PageId),
    Inner {
        // TODO: check that u16 is sufficient for level
        level: NonZeroU16,
        node: BPNodeId,
    },
}

#[derive(Debug, Clone)]
pub struct DebugLeafParent {
    pub pivots: Vec<Vec<u8>>,
    pub children: Vec<PageId>,
}

#[derive(Clone)]
pub struct InnerReadGuard<'a> {
    version: u64,
    node: NonNull<BPNode>,
    _marker: PhantomData<&'a BPNode>,
}

impl<'a> InnerReadGuard<'a> {
    pub fn as_ref(&self) -> &BPNode {
        unsafe { self.node.as_ref() }
        // &self.node
    }

    pub fn upgrade(self) -> Result<InnerWriteGuard<'a>, BPRestart> {
        let new_version = self.version + 0b10;
        match self.as_ref().vlock.compare_exchange_weak(
            self.version,
            new_version,
            Ordering::Acquire,
            Ordering::Relaxed,
        ) {
            Ok(_) => Ok(InnerWriteGuard {
                node: unsafe { &mut *self.node.as_ptr() },
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
#[repr(C)]
pub struct BPNode {
    vlock: AtomicU64,
    count: u32,
    /// index of the last allocated byte in the rest buffer
    /// a la a stack pointer
    alloc_idx: u32,
    // all 1s for None
    lowest: u64,
    rest: [u8; INLINE_BUFFER_LEN],
}

const INLINE_BUFFER_LEN: usize = 4072;
const LEAF_CHILD_BYTES: usize = 6;

const _: () = assert!(size_of::<BPNode>() == 4096);

impl BPNode {
    fn reset_leaf_parent(&mut self, lowest_child: PageId) {
        self.count = 0;
        self.alloc_idx = INLINE_BUFFER_LEN as u32 - 1;
        self.lowest = lowest_child.0;
    }

    fn append_leaf_entry(&mut self, key: &[u8], child: PageId) -> Result<(), QSError> {
        if key.len() > MAX_KEY_LENGTH {
            return Err(QSError::KeyTooLarge);
        }

        let needed = key.len() + LEAF_CHILD_BYTES;
        let meta_cost = size_of::<BPKVMeta>();
        if self.space_left() < needed + meta_cost {
            return Err(QSError::NodeFull);
        }

        self.alloc_idx = self
            .alloc_idx
            .checked_sub(needed as u32)
            .ok_or(QSError::NodeFull)?;

        let key_start = self.alloc_idx as usize + 1;
        let child_start = key_start + key.len();

        self.rest[key_start..key_start + key.len()].copy_from_slice(key);
        let child_bytes = store_u48(child.0);
        self.rest[child_start..child_start + LEAF_CHILD_BYTES].copy_from_slice(&child_bytes);

        let meta = BPKVMeta {
            start_offset: key_start as u16,
            key_len: key.len() as u16,
        };

        let meta_ptr = self.rest.as_mut_ptr() as *mut BPKVMeta;
        unsafe {
            meta_ptr.add(self.count as usize).write(meta);
        }

        self.count += 1;
        Ok(())
    }

    fn get_leaf_child(&self, idx: u32) -> PageId {
        let meta = self.get_meta(idx);
        let child_offset = meta.start_offset as usize + meta.key_len as usize;
        let child_ptr = unsafe { self.rest.as_ptr().add(child_offset) };
        PageId(extract_u48(child_ptr))
    }

    pub fn insert_leaf_entry_after_child(
        &mut self,
        left_child: PageId,
        pivot_key: &[u8],
        right_child: PageId,
    ) -> Result<(), QSError> {
        let lowest_child = PageId(self.lowest);
        let mut entries = Vec::with_capacity(self.count as usize + 1);
        for idx in 0..self.count {
            let key = self.get_key(idx).to_vec();
            let child = self.get_leaf_child(idx);
            entries.push((key, child));
        }

        let insert_idx = if left_child.0 == lowest_child.0 {
            0
        } else {
            entries
                .iter()
                .position(|(_, child)| child.0 == left_child.0)
                .map(|pos| pos + 1)
                .ok_or(QSError::ParentChildMissing)?
        };

        entries.insert(insert_idx, (pivot_key.to_vec(), right_child));

        self.reset_leaf_parent(lowest_child);
        for (key, child) in entries {
            self.append_leaf_entry(&key, child)?;
        }

        Ok(())
    }

    fn blank() -> BPNode {
        BPNode {
            vlock: AtomicU64::new(0),
            count: 0,
            alloc_idx: INLINE_BUFFER_LEN as u32 - 1,
            lowest: u64::MAX,
            rest: [0; INLINE_BUFFER_LEN],
        }
    }

    fn init_leaf_parent(
        &mut self,
        left_child: PageId,
        right_child: PageId,
        pivot_key: &[u8],
    ) -> Result<(), QSError> {
        if pivot_key.len() > MAX_KEY_LENGTH {
            return Err(QSError::KeyTooLarge);
        }

        self.count = 0;
        self.alloc_idx = INLINE_BUFFER_LEN as u32 - 1;
        self.lowest = left_child.0;

        let total = pivot_key.len() + LEAF_CHILD_BYTES;
        if total + size_of::<BPKVMeta>() > INLINE_BUFFER_LEN {
            return Err(QSError::KeyTooLarge);
        }

        self.alloc_idx = self
            .alloc_idx
            .checked_sub(total as u32)
            .ok_or(QSError::KeyTooLarge)?;

        let key_start = self.alloc_idx as usize + 1;
        let child_start = key_start + pivot_key.len();

        self.rest[key_start..key_start + pivot_key.len()].copy_from_slice(pivot_key);
        let child_bytes = store_u48(right_child.0);
        self.rest[child_start..child_start + LEAF_CHILD_BYTES].copy_from_slice(&child_bytes);

        let meta = BPKVMeta {
            start_offset: key_start as u16,
            key_len: pivot_key.len() as u16,
        };

        let meta_ptr = self.rest.as_mut_ptr() as *mut BPKVMeta;
        unsafe {
            meta_ptr.write(meta);
        }

        self.count = 1;

        Ok(())
    }

    unsafe fn init(mem: NonNull<[u8; 4096]>) {
        let node_ptr = mem.as_ptr() as *mut BPNode;

        node_ptr.write(BPNode {
            vlock: AtomicU64::new(0),
            count: 0,
            alloc_idx: INLINE_BUFFER_LEN as u32 - 1,
            lowest: u64::MAX,
            rest: [0; INLINE_BUFFER_LEN],
        });
    }

    /// calculate how much space is left in the node
    pub fn space_left(&self) -> usize {
        let kv_meta_size = size_of::<BPKVMeta>() * self.count as usize;

        self.alloc_idx as usize - kv_meta_size + 1
    }

    /// The node can overflow when a key is added to it
    pub fn can_overflow(&self, level: u16) -> bool {
        let child_size = match level {
            // If we are pointing to leafs we need 48 bit (6B)
            1 => LEAF_CHILD_BYTES,
            // If pointing to inner nodes then 32bit (4B)
            _ => 4,
        };
        // If we have more space than the metadata, child, and max key then we can't overflow
        self.space_left() < size_of::<BPKVMeta>() + MAX_KEY_LENGTH + child_size
    }

    /// The node will be underfull if a key is removed
    pub fn will_underflow(&self) -> bool {
        // This is just a heuristic, experimentation needed
        self.space_left() <= INLINE_BUFFER_LEN / 2
    }

    /// SAFETY: This method should only be called on nodes with height > 1
    pub unsafe fn search_for_inner(&self, key: &[u8]) -> BPNodeId {
        let idx = self.binary_search(key);
        let pivot_key = self.get_key(idx);
        if key < pivot_key {
            BPNodeId(self.lowest as u32)
        } else {
            let m = self.get_meta(idx);
            let child_offset = m.start_offset as usize + m.key_len as usize;
            let child_ptr = self.rest.as_ptr().add(child_offset);
            let child = extract_u32(child_ptr);
            BPNodeId(child)
        }
    }

    // SAFETY: This method should only be called on nodes with height = 1
    pub unsafe fn search_for_leaf(&self, key: &[u8]) -> PageId {
        let idx = self.binary_search(key);
        let pivot_key = self.get_key(idx);
        if key < pivot_key {
            PageId(self.lowest)
        } else {
            let m = self.get_meta(idx);
            let child_offset = m.start_offset as usize + m.key_len as usize;
            let child_ptr = self.rest.as_ptr().add(child_offset);
            let child = extract_u48(child_ptr);
            PageId(child)
        }
    }

    // find the index of the largest key smaller than or equal to the target
    #[inline]
    fn binary_search(&self, key: &[u8]) -> u32 {
        let mut low = 0;
        let mut high = self.count;

        while low < high {
            let mid = low + (high - low) / 2;
            let mid_key = self.get_key(mid);
            if mid_key <= key {
                low = mid + 1;
            } else {
                high = mid;
            }
        }

        low.saturating_sub(1)
    }

    fn get_meta(&self, idx: u32) -> BPKVMeta {
        let start_ptr = self.rest.as_ptr() as *const BPKVMeta;
        unsafe { start_ptr.add(idx as usize).read() }
    }

    fn get_key(&self, idx: u32) -> &[u8] {
        let meta = self.get_meta(idx);
        let start = meta.start_offset as usize;
        let end = start + meta.key_len as usize;
        &self.rest[start..end]
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

fn set_locked_bit(version: u64) -> u64 {
    version + 2
}

fn is_obsolete(version: u64) -> bool {
    (version & 1) == 1
}

fn is_locked_or_obsolete(version: u64) -> bool {
    (version & 0b11) != 0
}
// }

// TODO: add lookahead bytes
#[repr(C)]
struct BPKVMeta {
    /// offset from the start of the rest buffer, of the start of the key
    start_offset: u16,
    key_len: u16,
}

pub struct BPRestart;
