use std::{
    marker::PhantomData,
    mem::transmute,
    sync::atomic::{AtomicU64, AtomicUsize, Ordering},
};

use crate::{
    buffer,
    types::{NodeMeta, NodeSize},
    SPIN_RETRIES,
};

///         head     2nd chance        tail
///          |          |                |
///    +----------------------------------------------------+
///    |     [  ][][  ][    ][][  ][][][]                   |
///    +----------------------------------------------------+
pub struct MiniPageBuffer {
    buffer: Box<[u64]>,
    /// u64::MAX represents None
    free_lists: [AtomicUsize; 7],
    /// start of the oldest node not yet fully freed
    head: AtomicUsize,
    /// start of unmanaged memory
    tail: AtomicUsize,
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
                    let free_space_words = self.buffer.len() - tail;

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
            let next = self.buffer[head_index + 1];

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

    pub fn evict(&self) {}

    /// Deallocate a mini-page, this mini-page must be unused, ie. not appear in the mapping table
    pub fn dealloc(&self, node: MiniPageIndex) {
        let node_meta: &NodeMeta = self.get_meta_ref(node);

        // if its in the second chance region, there's no point adding it to a free list
        if todo!("Is in the second chance region") {
            // node_meta.
        } else {
        }
        let size = node_meta.size();
    }

    pub fn get_meta_ref<'g>(&self, node: MiniPageIndex<'g>) -> &'g NodeMeta {
        // SAFETY: MiniPageIndex was created as an index to the metadata of a valid NodeMeta
        unsafe { transmute(self.buffer.get_unchecked(node.index as usize)) }
    }
}

#[derive(Clone, Copy)]
// pub(crate) struct MiniPageIndex(pub(crate) u64);
pub struct MiniPageIndex<'g> {
    //only 48 bits used
    pub(crate) index: u64,
    pub(crate) _marker: PhantomData<&'g ()>,
}
