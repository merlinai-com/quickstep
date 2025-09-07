use crate::io_engine::IoEngine;
use crate::map_table::PageReadGuard;
use crate::rand::rand_for_cache;
use crate::types::NodeMeta;
use crate::utils::HackLifetime;
use crate::{buffer::MiniPageBuffer, types::NodeRef};

impl<'a> PageReadGuard<'a> {
    pub fn get<'g>(
        &'g self,
        cache: &MiniPageBuffer,
        io: &IoEngine,
        key: &[u8],
    ) -> Option<&'g [u8]> {
        match self.node() {
            NodeRef::Leaf(_) => todo!("read, mini page"),
            NodeRef::MiniPage(mini_page_index) => {
                let node_meta = cache.get_meta_ref(mini_page_index);
                let prefix = node_meta.get_node_prefix();
                match node_meta
                    .binary_search(prefix.len(), key)
                    .map(|i| node_meta.get_kv_meta(i))
                {
                    Ok(kv) => match kv.typ().exists() {
                        true => return Some(node_meta.get_key_from_meta(kv)),
                        false => return None,
                    },
                    Err(_) => {}
                }

                let leaf_ref = node_meta.leaf();

                let leaf = io.get_page(leaf_ref);
                let leaf_meta = leaf.as_ref();

                // reusing prefix because fence keys should be the same

                // Leaf doesn't have non-existant entries so check is not needed
                let target_kv = leaf_meta
                    .binary_search(leaf_meta.get_node_prefix().len(), key)
                    .ok()?;

                let target_kv = leaf_meta.get_kv_meta(target_kv);

                let val = leaf_meta.get_val_from_meta(target_kv);

                // TODO: move copy-on-access and caching elsewhere
                // if rand_for_cache() {
                //     let write_guard = self.upgrade();

                //     // TODO: add to cache

                //     self = write_guard.downgrade()
                // };

                Some(val)
            }
        }
    }
}
