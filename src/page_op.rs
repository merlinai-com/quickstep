use crate::error::QSError;
use crate::io_engine::{DiskLeaf, IoEngine};
use crate::lock_manager::{GuardWrapper, PageGuard};
use crate::map_table::PageReadGuard;
use crate::rand::rand_for_cache;
use crate::types::NodeMeta;
use crate::utils::HackLifetime;
use crate::{buffer::MiniPageBuffer, types::NodeRef};

impl<'a> PageGuard<'a> {
    pub fn get<'g>(
        &'g mut self,
        cache: &MiniPageBuffer,
        io: &IoEngine,
        key: &[u8],
    ) -> Result<Option<&'g [u8]>, QSError> {
        let node = match &mut self.guard_inner {
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
                match node_meta
                    .binary_search(prefix.len(), key)
                    .map(|i| node_meta.get_kv_meta(i))
                {
                    Ok(kv) => {
                        let val = match kv.typ().exists() {
                            true => Some(node_meta.get_key_from_meta(kv)),
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
