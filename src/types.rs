use std::mem::transmute;

use crate::buffer::MiniPageIndex;

/// | key size | val size | offset | type | fence | ref | look ahead |
///      14b       14b       16b       2b     1b     1b       16b
/// Note: only 12b is needed for the offset, as the maximum page size is 4096 = 2 ^ 12
#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct KVMeta(u64);

impl KVMeta {
    #[inline]
    pub fn key_size(&self) -> u64 {
        self.0 >> 50
    }

    #[inline]
    pub fn set_key_size(&mut self, key_size: u16) {
        todo!()
    }

    #[inline]
    pub fn val_size(&self) -> u64 {
        (self.0 << 14) >> 50
    }

    #[inline]
    pub fn set_val_size(&mut self, val_size: u16) {
        todo!()
    }

    #[inline]
    pub fn offset(&self) -> usize {
        ((self.0 << 28) >> 48) as usize
    }

    #[inline]
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
    N4K = 6,
    LeafPage = 7,
}

impl NodeSize {
    pub const fn index(&self) -> usize {
        match self {
            // Leaf page is the same size as a 4k node, and should use the same free list
            NodeSize::LeafPage => NodeSize::N4K.index(),
            s => *s as usize,
        }
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
/// | Leaf | size | padding | live | split | record count
///   48b  |  3b  |   2b    | 1b   |   1b  |      9b
/// Note: each record must take up at least 8 bytes, owing to the metadata, so there can only be 512/page
///     this means that 9b is sufficient to encode the record count
#[repr(transparent)]
pub struct NodeMeta(u64);

impl NodeMeta {
    // pub unsafe fn from_repr(repr: u64) -> NodeMeta {
    //     NodeMeta(repr)
    // }

    // pub fn to_repr(self) -> u64 {
    //     self.0
    // }
}

impl NodeMeta {
    #[inline]
    pub fn size(&self) -> NodeSize {
        let size_byte = ((self.0 >> 13) & 0b111) as u8;
        // SAFETY: this was just masked to 3 bits and all 3bit values are valid
        unsafe { transmute(size_byte) }
    }

    #[inline]
    pub fn record_count(&self) -> u16 {
        const RECORD_COUNT_MASK: u64 = 0x0000_0000_0000_01FF;
        (self.0 & RECORD_COUNT_MASK) as u16
    }

    pub fn leaf(&self) -> u64 {
        self.0 >> 16
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
