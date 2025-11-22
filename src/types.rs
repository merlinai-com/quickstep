use std::{
    error::Error,
    mem::{size_of, transmute},
};

use crate::{
    buffer::{MiniPageBuffer, MiniPageIndex},
    lock_manager::{self, LockManager, WriteGuardWrapper},
    map_table::{PageId, PageWriteGuard},
    QuickStepTx,
};

/// | key size | val size | offset | type | fence | ref | look ahead |
///      14b       14b       16b       2b     1b     1b       16b
/// Note: only 12b is needed for the offset, as the maximum page size is 4096 = 2 ^ 12
#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct KVMeta(pub u64);

impl KVMeta {
    /// Arguments must fit within the bits of the repr or will be truncated or corrupt the result
    pub fn new(
        key_size: usize,
        val_size: usize,
        offset: usize,
        typ: KVRecordType,
        is_fence: bool,
        refb: bool,
        lookahead: u16,
    ) -> KVMeta {
        let mut acc = 0;
        acc |= (key_size as u64) << 50;
        acc |= (val_size as u64) << 36;
        acc |= (offset as u64) << 20;
        acc |= (typ as u64) << 18;
        acc |= (is_fence as u64) << 17;
        acc |= (refb as u64) << 16;
        acc |= lookahead as u64;

        KVMeta(acc)
    }

    #[inline]
    pub fn key_size(&self) -> u64 {
        self.0 >> 50
    }

    #[inline]
    #[must_use]
    pub fn set_key_size(&mut self, key_size: u16) {
        todo!()
    }

    #[inline]
    pub fn val_size(&self) -> u64 {
        (self.0 << 14) >> 50
    }

    #[inline]
    #[must_use]
    pub fn set_val_size(&mut self, val_size: u16) {
        todo!()
    }

    #[inline]
    pub fn offset(&self) -> usize {
        ((self.0 << 28) >> 48) as usize
    }

    #[inline]
    #[must_use]
    pub fn set_offset(&mut self, offset: u16) {
        const OFFSET_MAST: u64 = { (u16::MAX as u64) << 20 };

        // clear offset bits
        self.0 &= !OFFSET_MAST;
        self.0 |= (offset as u64) << 20;
    }

    #[inline]
    pub fn typ(&self) -> KVRecordType {
        unsafe { transmute(((self.0 << 44) >> 62) as u8) }
    }

    #[inline]
    pub fn fence(&self) -> bool {
        const MASK: u64 = 1 << 17;
        (MASK & self.0) == MASK
    }

    #[inline]
    pub fn ref_bit(&self) -> bool {
        const MASK: u64 = 1 << 16;
        (MASK & self.0) == MASK
    }

    #[inline]
    #[must_use]
    pub fn set_ref_bit(mut self, val: bool) -> KVMeta {
        const MASK: u64 = 1 << 16;
        self.0 &= !MASK;
        self.0 |= (val as u64) << 16;
        self
    }

    /// get 2 lookahead bytes of the key, after the common page prefix
    #[inline]
    pub fn look_ahead(&self) -> u16 {
        const MASK: u64 = 0xFFFF;
        (MASK & self.0) as u16
    }
}

// +-----------+--------+---------+
// | Record    | Dirty? | Exists? |
// +-----------+--------+---------+
// | INSERT    |  true  |  true   |
// | CACHE     |  false |  true   |
// | TOMBSTONE |  true  |  false  |
// | PHANTOM   |  false |  false  |
// +-----------+--------+---------+
#[repr(u8)]
pub enum KVRecordType {
    Insert = 0b11,
    Cache = 0b01,
    Tombstone = 0b10,
    Phantom = 0b00,
}

impl KVRecordType {
    #[inline]
    pub fn is_dirty(&self) -> bool {
        match self {
            KVRecordType::Insert | KVRecordType::Tombstone => true,
            KVRecordType::Cache | KVRecordType::Phantom => true,
        }
    }

    #[inline]
    pub fn exists(&self) -> bool {
        match self {
            KVRecordType::Insert | KVRecordType::Cache => true,
            KVRecordType::Tombstone | KVRecordType::Phantom => true,
        }
    }
}

/// represents node size/ type
/// if not a Leaf, then for discriminent x, 2^x * 8 is the number of words needed
/// takes 3 bits to store
#[derive(Clone, Copy)]
#[repr(u8)]
pub enum NodeSize {
    N64 = 0,
    N128 = 1,
    N256 = 2,
    N512 = 3,
    N1K = 4,
    N2K = 5,
    LeafPage = 6,
}

impl NodeSize {
    pub const fn index(&self) -> usize {
        *self as usize
    }

    pub fn from_byte_num(bytes: usize) -> Option<NodeSize> {
        let cand = bytes.next_power_of_two() / 64;

        Some(match cand {
            0 => NodeSize::N64,
            1 => NodeSize::N128,
            2 => NodeSize::N256,
            3 => NodeSize::N512,
            4 => NodeSize::N1K,
            5 => NodeSize::N2K,
            6 => NodeSize::LeafPage,
            _ => return None,
        })
    }

    pub const fn size_in_words(&self) -> usize {
        let d = self.index();
        // 1 << (d + 3)
        2usize.pow(d as u32) * 8
    }

    pub const fn size_in_bytes(&self) -> usize {
        self.size_in_words() * 8
    }
}

// TODO: if there are bits spare, seperate size from mini-node/ leaf
/// Metadata of a leaf or mini-page
/// INVARIANT: a reference to this must be part of a valid mini-page or leaf
/// | Leaf | size | evicting | free-listed | live | split | record count
///   48b  |  3b  |   1b     |      1b     | 1b   |   1b  |      9b
///
/// | NodeId | padding | free on disk
///     48b  |    4b   |      12b
/// Note: each record must take up at least 8 bytes, owing to the metadata, so there can only be 512/page
///     this means that 9b is sufficient to encode the record count
#[repr(C)]
// pub struct NodeMeta(AtomicU64, AtomicU64);
pub struct NodeMeta(u64, u64);

impl NodeMeta {
    // pub unsafe fn from_repr(repr: u64) -> NodeMeta {
    //     NodeMeta(repr)
    // }

    // pub fn to_repr(self) -> u64 {
    //     self.0
    // }

    pub unsafe fn init<'db>(
        tx: &mut QuickStepTx<'db>,
        index: usize,
        size: NodeSize,
        disk_addr: Option<u64>,
    ) -> WriteGuardWrapper<'db> {
        let node_ptr = tx.db.cache.get_meta_ptr(index);
        let disk_addr = disk_addr.unwrap_or_else(|| tx.db.io_engine.get_new_addr());
        let guard = tx.db.map_table.create_page_entry(MiniPageIndex::new(index));

        let mut w0 = (disk_addr as u64) << 16;
        w0 |= (size as u64) << 13;
        // This node is live
        w0 |= 1 << 10;

        let mut w1 = guard.page.0 << 16;
        let free = 4096 - size_of::<NodeMeta>();
        w1 |= free as u64;

        node_ptr.write(NodeMeta(w0, w1));

        tx.lock_manager.insert_write_lock(guard)
    }
}

impl NodeMeta {
    #[inline]
    pub fn leaf(&self) -> u64 {
        self.0 >> 16
    }

    #[inline]
    pub fn size(&self) -> NodeSize {
        let size_byte = ((self.0 >> 13) & 0b111) as u8;
        // SAFETY: this was just masked to 3 bits and all 3bit values are valid
        unsafe { transmute(size_byte) }
    }

    pub fn is_being_evicted(&self) -> bool {
        todo!()
    }

    // TODO: this needs to basically do a version check, so we know that the head pointer hasn't been moved past it and it hasn't been messed with
    // since we called, is being evicted
    pub fn mark_for_eviction(&self) -> Result<(), ()> {
        todo!()
    }

    #[inline]
    pub fn record_count(&self) -> u16 {
        const RECORD_COUNT_MASK: u64 = 0x0000_0000_0000_01FF;
        (self.0 & RECORD_COUNT_MASK) as u16
    }

    #[inline]
    pub fn set_record_count(&mut self, count: u16) {
        const RECORD_COUNT_MASK: u64 = 0x0000_0000_0000_01FF;
        self.0 &= !RECORD_COUNT_MASK;
        self.0 |= (count as u64) & RECORD_COUNT_MASK;
    }

    #[inline]
    pub fn inc_record_count(&mut self) {
        let next = self
            .record_count()
            .checked_add(1)
            .expect("record count overflow");
        self.set_record_count(next);
    }

    #[inline]
    pub fn dec_record_count(&mut self) {
        let next = self
            .record_count()
            .checked_sub(1)
            .expect("record count underflow");
        self.set_record_count(next);
    }

    #[inline]
    pub fn page_id(&self) -> PageId {
        PageId(self.1 >> 16)
    }

    pub fn reset_header(&mut self, page_id: PageId, size: NodeSize, disk_addr: u64) {
        let mut w0 = (disk_addr as u64) << 16;
        w0 |= (size as u64) << 13;
        w0 |= 1 << 10;
        self.0 = w0;

        let free = size.size_in_bytes() - size_of::<NodeMeta>();
        let mut w1 = (page_id.0) << 16;
        w1 |= (free as u64) & 0xFFFF;
        self.1 = w1;
    }

    #[inline]
    pub fn entries(&self) -> LeafEntryIter<'_> {
        LeafEntryIter {
            node: self,
            idx: 0,
            end: self.record_count() as usize,
        }
    }
}

// Idea: use this layout, and use a macro for match, a la congee
// | padding | address | type |
// |   15b   |   48b   |  1b  |
#[derive(Clone)]
pub enum NodeRef<'g> {
    Leaf(u64),
    MiniPage(MiniPageIndex<'g>),
}

pub struct LeafEntry<'a> {
    pub meta: KVMeta,
    pub key_suffix: &'a [u8],
    pub value: &'a [u8],
}

pub struct LeafEntryIter<'a> {
    node: &'a NodeMeta,
    idx: usize,
    end: usize,
}

impl<'a> Iterator for LeafEntryIter<'a> {
    type Item = LeafEntry<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.idx >= self.end {
            return None;
        }

        let meta = self.node.get_kv_meta(self.idx);
        self.idx += 1;

        Some(LeafEntry {
            meta,
            key_suffix: self.node.get_stored_key_from_meta(meta),
            value: self.node.get_val_from_meta(meta),
        })
    }
}
