use std::cell::UnsafeCell;

use crate::error::QSError;
use crate::io_engine::{self, DiskLeaf, IoEngine};
use crate::lock_manager::{GuardWrapper, PageGuard, WriteGuardWrapper};

use crate::map_table::PageWriteGuard;
use crate::node::InsufficientSpace;
use crate::rand::rand_for_cache;
use crate::types::{NodeMeta, NodeSize};
use crate::{buffer::MiniPageBuffer, types::NodeRef, QuickStep, QuickStepTx};

impl<'a> PageGuard<'a> {
    pub fn get<'g>(
        &'g mut self,
        cache: &MiniPageBuffer,
        io: &IoEngine,
        key: &[u8],
    ) -> Result<Option<&'g [u8]>, QSError> {
        let node = match &self.guard_inner {
            GuardWrapper::Write(g) => g.node(),
            GuardWrapper::Read(g) => g.node(),
        };

        let val = match node {
            NodeRef::Leaf(addr) => {
                let leaf = ensure_page(io, &mut self.leaf, addr)?;
                leaf.as_ref().get(key)
            }
            NodeRef::MiniPage(mini_page_index) => {
                // SAFETY: we have either a read or write lock
                let node_meta = unsafe { cache.get_meta_ref(mini_page_index) };
                let prefix = node_meta.get_node_prefix();
                let key_suffix = &key[prefix.len()..];
                match node_meta
                    .binary_search(key_suffix)
                    .map(|i| node_meta.get_kv_meta(i))
                {
                    Ok(kv) => {
                        let val = match kv.typ().exists() {
                            true => Some(node_meta.get_val_from_meta(kv)),
                            false => None,
                        };
                        // Value is already cached, so early return
                        return Ok(val);
                    }
                    Err(_) => {}
                }

                let leaf_addr = node_meta.leaf();
                let leaf = ensure_page(io, &mut self.leaf, leaf_addr)?;

                leaf.as_ref().get(key)
            }
        };

        // if rand_for_cache() {
        //     if let Ok(tmp_write) = self.guard_inner.temp_upgrade() {}
        // }

        // TODO: implement caching
        // if rand_for_cache() {
        //     // let write_guard = self.upgrade();

        //     // TODO: add to cache

        //     // self = write_guard.downgrade()

        //     match &mut self.guard_inner {
        //         GuardWrapper::Write(wg) => {
        //             // Does the mini-page (if any) have enough space?
        //             // If so just insert into that
        //             // If not allocate a new mini-page
        //             todo!();
        //         }
        //         GuardWrapper::Read(page_read_guard) => todo!(),
        //     }
        // };

        Ok(val)
    }
}

impl<'tx, 'a> WriteGuardWrapper<'tx, 'a> {
    pub fn try_put(
        &'tx mut self,
        tx: &QuickStepTx<'a>,
        key: &[u8],
        val: &[u8],
    ) -> Result<(), SplitNeeded> {
        // TODO: pass fence keys as args
        let write_guard = self.get_write_guard();

        let node = write_guard.node();

        match node {
            NodeRef::Leaf(addr) => {
                // Promote the on-disk leaf into a mini-page.
                let mut guard = tx.new_mini_page(NodeSize::LeafPage, Some(addr));

                {
                    let write_guard = guard.get_write_guard();
                    let node_meta = match write_guard.node() {
                        NodeRef::MiniPage(idx) => unsafe { tx.db.cache.get_meta_mut(idx) },
                        NodeRef::Leaf(_) => unreachable!("new mini page should return a mini page"),
                    };

                    node_meta.try_put(key, val).map_err(|_| SplitNeeded)?;
                }

                *self = guard;
                Ok(())
            }
            NodeRef::MiniPage(mini_page_index) => {
                // SAFETY: we have either a read or write lock
                let node_meta = unsafe { tx.db.cache.get_meta_mut(mini_page_index) };

                match node_meta.try_put(key, val) {
                    Ok(_) => Ok(()),
                    Err(_) => Err(SplitNeeded),
                }

                // let prefix = node_meta.get_node_prefix();
                // match node_meta
                //     .binary_search(prefix.len(), key)
                //     .map(|i| node_meta.get_kv_meta(i))
                // {
                //     Ok(kv) => {

                //         // Value is already cached, so early return
                //         return Ok(val);
                //     }
                //     Err(_) => {}
                // }

                // let leaf_addr = node_meta.leaf();
                // let leaf = ensure_page(io, &mut self.leaf, leaf_addr)?;

                // leaf.as_ref().get(key)
            }
        }
    }

    pub fn merge_to_disk(&mut self, buffer: &MiniPageBuffer, io_engine: &IoEngine) {
        let write_guard = self.get_write_guard();
        let node = write_guard.node();
        let index = match node {
            NodeRef::Leaf(_) => {
                panic!("should only be called on mini pages");
            }
            NodeRef::MiniPage(i) => i,
        };

        // SAFETY: we've got a write guard
        // TODO: implement safe method on buffer with page write guard
        let node = unsafe { buffer.get_meta_ref(index) };

        let mut disk_leaf: Option<DiskLeaf> = None;
        let leaf_addr = node.leaf();

        let cnt = node.record_count() as usize;
        // Check all entries to see if any need to be flushed to disk
        for i in 0..cnt {
            let kv = node.get_kv_meta(i);

            if kv.fence() {
                continue;
            }

            if kv.typ().is_dirty() {
                if disk_leaf.is_none() {
                    let leaf = io_engine.get_page(leaf_addr);
                    disk_leaf = Some(leaf);
                }

                let key_suffix = node.get_stored_key_from_meta(kv);
                let val = node.get_val_from_meta(kv);

                disk_leaf
                    .as_mut()
                    .expect("just ensured was Some")
                    .as_mut()
                    .try_put_with_suffix(key_suffix, val)
                    .expect(
                        "We should have already split if there wouldn't be enough space on merge",
                    );
            }
        }

        if let Some(dirty_leaf) = disk_leaf {
            io_engine.write_page(leaf_addr, &dirty_leaf);
        }
    }
}

fn ensure_page<'a>(
    io: &IoEngine,
    cache: &'a mut Option<DiskLeaf>,
    addr: u64,
) -> Result<&'a mut DiskLeaf, QSError> {
    let leaf = match cache {
        Some(l) => l,
        l => {
            let new_leaf = io.get_page(addr);
            *l = Some(new_leaf);
            l.as_mut().expect("We just set this to Some")
        }
    };
    Ok(leaf)
}

pub struct SplitNeeded;
