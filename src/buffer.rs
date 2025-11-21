use std::{
    array,
    marker::PhantomData,
    ptr::NonNull,
    sync::atomic::{AtomicU64, AtomicUsize, Ordering},
    usize,
};

use crate::{
    buffer,
    io_engine::{self, DiskLeaf, IoEngine},
    map_table::MapTable,
    types::{NodeMeta, NodeSize},
    QuickStepTx, SPIN_RETRIES,
};

///         head     2nd chance        tail
///          |          |                |
///    +----------------------------------------------------+
///    |     [  ][][  ][    ][][  ][][][]                   |
///    +----------------------------------------------------+
pub struct MiniPageBuffer {
    buffer: NonNull<u64>,
    backing: Box<[u64]>,
    /// number of words in buffer, must be a power of 2
    buff_size: usize,
    /// u64::MAX represents None
    free_lists: [AtomicUsize; 7],
    /// start of the oldest node not yet fully freed
    head: AtomicUsize,
    /// start of unmanaged memory
    tail: AtomicUsize,
}

impl MiniPageBuffer {
    pub fn new(cache_size_lg: usize) -> MiniPageBuffer {
        assert!(
            cache_size_lg >= 3 && cache_size_lg < usize::BITS as usize,
            "cache_size_lg must be between 3 and {}",
            usize::BITS - 1
        );

        let total_bytes = 1usize
            .checked_shl(cache_size_lg as u32)
            .expect("cache size overflowed usize");
        assert!(
            total_bytes % 8 == 0,
            "cache size must be aligned to 64-bit words"
        );

        let buff_size = total_bytes / 8;
        assert!(
            buff_size.is_power_of_two(),
            "cache size must be a power of two"
        );

        let mut backing = vec![0u64; buff_size].into_boxed_slice();
        let buffer =
            NonNull::new(backing.as_mut_ptr()).expect("backing allocation should never be null");

        MiniPageBuffer {
            buffer,
            backing,
            buff_size,
            free_lists: array::from_fn(|_| AtomicUsize::new(usize::MAX)),
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }

    const fn wrap(&self, index: usize) -> usize {
        index & (self.buff_size - 1)
    }
}

impl MiniPageBuffer {
    pub fn alloc(&self, size: NodeSize) -> Option<usize> {
        if let Some(page) = self.pop_freelist(size) {
            return Some(page);
        }

        let req_size = size.size_in_words();
        let mut tail = self.tail.load(Ordering::Acquire);
        for _ in 0..SPIN_RETRIES {
            let head = self.head.load(Ordering::Acquire);

            match head < tail {
                // barrier is end of buffer
                true => {
                    let free_space_words = self.buff_size - tail;

                    match free_space_words >= req_size {
                        true => {
                            let new_tail = tail + req_size;
                            match self.tail.compare_exchange_weak(
                                tail,
                                new_tail,
                                Ordering::AcqRel,
                                Ordering::Acquire,
                            ) {
                                Ok(_) => return Some(tail),
                                Err(t) => {
                                    tail = t;
                                    continue;
                                }
                            }
                        }
                        false => todo!("How should we handle little chunks near the end?"),
                        // I don't think it would be a good idea to have mini-pages wrapping,
                        // I think the most obvious solution is to add them to the free lists, recursively take the largest size that will fit
                        // create a header and mark it dead and in the free list, then add and progress the tail
                    }
                }
                // barrier is the head
                false => {
                    let free_space_words = head - tail;
                    match free_space_words >= req_size {
                        true => {
                            let new_tail = tail + req_size;
                            match self.tail.compare_exchange_weak(
                                tail,
                                new_tail,
                                Ordering::AcqRel,
                                Ordering::Acquire,
                            ) {
                                Ok(_) => return Some(tail),
                                Err(t) => {
                                    tail = t;
                                    continue;
                                }
                            }
                        }
                        false => return None,
                    }
                }
            }
        }
        None
    }

    fn pop_freelist(&self, size: NodeSize) -> Option<usize> {
        let free_list_head = &self.free_lists[size.index()];
        let mut head_index = free_list_head.load(Ordering::Acquire);
        for _ in 0..SPIN_RETRIES {
            // No items in free list
            if head_index == usize::MAX {
                return None;
            }

            // next pointer should be stored in the word after the meta
            // let next = self.buffer[head_index + 1];
            let next = unsafe { &*(self.buffer.add(head_index + 1).as_ptr() as *const AtomicU64) }
                .load(Ordering::Relaxed);

            match free_list_head.compare_exchange_weak(
                head_index,
                next as usize,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Some(head_index as usize),
                Err(h) => head_index = h,
            }
            std::hint::spin_loop();
        }
        None
    }

    pub fn evict(&self, map_table: &MapTable, io_engine: &IoEngine) {
        // scan through items in the last chance zone
        // for each:
        // de mark ref bit,

        // TODO: deal with race condition where I read the head pointer, but someone else advances the head pointer and allocates a different node
        let mut eviction_cand = self.head.load(Ordering::Relaxed);

        let (guard, node) = loop {
            // Need to be careful with the operations we do on this
            let cand_meta = unsafe { &*self.get_meta_ptr(eviction_cand) };

            match cand_meta.is_being_evicted() {
                true => {}
                false => {
                    match cand_meta.mark_for_eviction() {
                        Ok(_) => match map_table.write_page_entry(cand_meta.page_id()) {
                            Ok(g) => {
                                // TODO: add assertion that we've got the right lock
                                break (g, cand_meta);
                            }
                            Err(_) => {
                                todo!("Should we skip to the next one?")
                            }
                        },
                        Err(_) => {}
                    };
                }
            }
            let offset = cand_meta.size().size_in_bytes();
            eviction_cand = self.wrap(eviction_cand + offset);
        };

        // Check if its in free list, if so remove from free list

        // let mut disk_leaf: Option<DiskLeaf> = None;
        // let leaf_addr = node.leaf();

        // let cnt = node.record_count() as usize;
        // for i in 0..cnt {
        //     let kv = node.get_kv_meta(i);

        //     if kv.fence() {
        //         continue;
        //     }

        //     if kv.typ().is_dirty() {
        //         match disk_leaf.as_mut() {
        //             Some(leaf) => {
        //                 todo!("merge logic")
        //             }
        //             leaf_ref => {
        //                 let leaf = io_engine.get_page(leaf_addr);
        //                 leaf_ref = Some(())
        //                 todo!("merge logic")
        //             }
        //         }
        //     }
        // }

        // if let Some(dirty_leaf) = disk_leaf {
        //     io_engine.write_page(leaf_addr, dirty_leaf);
        // }

        // guard.
    }

    /// Deallocate a mini-page, this mini-page must be unused, ie. not appear in the mapping table
    pub unsafe fn dealloc(&self, node: MiniPageIndex) {
        let node_meta: &NodeMeta = self.get_meta_ref(node);

        // if its in the second chance region, there's no point adding it to a free list
        if todo!("Is in the second chance region") {
            // node_meta.
        } else {
        }
        let size = node_meta.size();
    }

    pub unsafe fn get_meta_ptr(&self, index: usize) -> *mut NodeMeta {
        unsafe { self.buffer.add(index).as_ptr() as *mut NodeMeta }
    }

    /// SAFETY: caller must guarentee that a mutable reference does not exist eg. hold a lock
    pub unsafe fn get_meta_ref<'g>(&self, node: MiniPageIndex<'g>) -> &'g NodeMeta {
        // SAFETY: MiniPageIndex was created as an index to the metadata of a valid NodeMeta
        unsafe { &*self.get_meta_ptr(node.index) }
    }

    /// SAFETY: caller must guarentee that no other references exist
    pub unsafe fn get_meta_mut<'g>(&self, node: MiniPageIndex<'g>) -> &'g mut NodeMeta {
        // SAFETY: MiniPageIndex was created as an index to the metadata of a valid NodeMeta
        unsafe { &mut *self.get_meta_ptr(node.index) }
    }
}

#[derive(Clone, Copy)]
// pub(crate) struct MiniPageIndex(pub(crate) u64);
pub struct MiniPageIndex<'g> {
    //only 48 bits used
    pub(crate) index: usize,
    pub(crate) _marker: PhantomData<&'g ()>,
}

impl<'g> MiniPageIndex<'g> {
    /// # SAFETY
    /// index must be valid allocated index into the minipage buffer
    /// the metadata must be initialised and valid
    pub unsafe fn new(index: usize) -> MiniPageIndex<'g> {
        MiniPageIndex {
            index,
            _marker: PhantomData,
        }
    }
}
