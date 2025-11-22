use crate::buffer::MiniPageBuffer;
use crate::error::QSError;
use crate::io_engine::{DiskLeaf, IoEngine};
use crate::lock_manager::{GuardWrapper, PageGuard, WriteGuardWrapper};
use crate::node::InsufficientSpace;
use crate::types::{LeafEntry, NodeMeta, NodeRef};

#[allow(dead_code)]
#[derive(Debug)]
pub struct LeafSplitPlan {
    pub prefix: Vec<u8>,
    pub pivot_key: Vec<u8>,
    pub left_entries: Vec<LeafEntryOwned>,
    pub right_entries: Vec<LeafEntryOwned>,
}

#[allow(dead_code)]
#[derive(Debug)]
pub struct LeafEntryOwned {
    pub key: Vec<u8>,
    pub value: Vec<u8>,
}

impl LeafEntryOwned {
    fn from_entry(prefix: &[u8], entry: &LeafEntry<'_>) -> LeafEntryOwned {
        let mut key = Vec::with_capacity(prefix.len() + entry.key_suffix.len());
        key.extend_from_slice(prefix);
        key.extend_from_slice(entry.key_suffix);
        LeafEntryOwned {
            key,
            value: entry.value.to_vec(),
        }
    }
}

#[allow(dead_code)]
impl LeafSplitPlan {
    pub fn from_node(meta: &NodeMeta) -> LeafSplitPlan {
        let prefix = meta.get_node_prefix();
        let mut prefix_buf = Vec::with_capacity(prefix.len());
        prefix_buf.extend_from_slice(prefix);

        let mut live_entries = Vec::new();

        for entry in meta.entries() {
            if entry.meta.fence() {
                continue;
            }
            live_entries.push(entry);
        }

        assert!(
            !live_entries.is_empty(),
            "Leaf must contain at least one non-fence entry for a split"
        );

        let move_start = live_entries.len() / 2;
        let pivot_entry = &live_entries[move_start];

        let mut pivot_key = Vec::with_capacity(prefix.len() + pivot_entry.key_suffix.len());
        pivot_key.extend_from_slice(prefix);
        pivot_key.extend_from_slice(pivot_entry.key_suffix);

        let left_entries = live_entries[..move_start]
            .iter()
            .map(|entry| LeafEntryOwned::from_entry(prefix, entry))
            .collect();

        let right_entries = live_entries[move_start..]
            .iter()
            .map(|entry| LeafEntryOwned::from_entry(prefix, entry))
            .collect();

        LeafSplitPlan {
            prefix: prefix_buf,
            pivot_key,
            left_entries,
            right_entries,
        }
    }

    pub fn apply(
        &self,
        left: &mut NodeMeta,
        right: &mut NodeMeta,
    ) -> Result<LeafSplitOutcome, InsufficientSpace> {
        left.reset_user_entries();
        left.replay_entries(
            self.left_entries
                .iter()
                .map(|entry| (entry.key.as_slice(), entry.value.as_slice())),
        )?;

        right.reset_user_entries();
        right.replay_entries(
            self.right_entries
                .iter()
                .map(|entry| (entry.key.as_slice(), entry.value.as_slice())),
        )?;

        Ok(LeafSplitOutcome {
            pivot_key: self.pivot_key.clone(),
            left_count: self.left_entries.len(),
            right_count: self.right_entries.len(),
        })
    }
}

#[allow(dead_code)]
#[derive(Debug)]
pub struct LeafSplitOutcome {
    pub pivot_key: Vec<u8>,
    pub left_count: usize,
    pub right_count: usize,
}

#[allow(dead_code)]
#[derive(Debug)]
pub struct LeafMergePlan {
    pub entries: Vec<LeafEntryOwned>,
}

#[allow(dead_code)]
#[derive(Debug)]
pub struct LeafMergeOutcome {
    pub merged_count: usize,
}

impl LeafMergePlan {
    pub fn from_nodes(left: &NodeMeta, right: &NodeMeta) -> LeafMergePlan {
        let mut entries = Vec::new();
        entries.extend(owned_entries(left));
        entries.extend(owned_entries(right));
        LeafMergePlan { entries }
    }

    pub fn apply(
        &self,
        survivor: &mut NodeMeta,
        removed: &mut NodeMeta,
    ) -> Result<LeafMergeOutcome, InsufficientSpace> {
        survivor.reset_user_entries();
        survivor.replay_entries(
            self.entries
                .iter()
                .map(|entry| (entry.key.as_slice(), entry.value.as_slice())),
        )?;

        removed.reset_user_entries();
        Ok(LeafMergeOutcome {
            merged_count: self.entries.len(),
        })
    }
}

pub fn flush_dirty_entries(node_meta: &mut NodeMeta, io_engine: &IoEngine) {
    let mut disk_leaf: Option<DiskLeaf> = None;
    let leaf_addr = node_meta.leaf();
    let mut tombstones = Vec::new();

    let cnt = node_meta.record_count() as usize;
    for i in 0..cnt {
        let kv = node_meta.get_kv_meta(i);

        if kv.fence() {
            continue;
        }

        match kv.typ() {
            crate::types::KVRecordType::Tombstone => {
                let entry = disk_leaf.get_or_insert_with(|| io_engine.get_page(leaf_addr));
                let prefix = node_meta.get_node_prefix();
                let suffix = node_meta.get_stored_key_from_meta(kv);
                let mut key = Vec::with_capacity(prefix.len() + suffix.len());
                key.extend_from_slice(prefix);
                key.extend_from_slice(suffix);
                entry.as_mut().remove_key_physical(&key);
                tombstones.push(i);
            }
            typ if typ.is_dirty() => {
                let entry = disk_leaf.get_or_insert_with(|| io_engine.get_page(leaf_addr));
                let key_suffix = node_meta.get_stored_key_from_meta(kv);
                let val = node_meta.get_val_from_meta(kv);

                entry
                    .as_mut()
                    .try_put_with_suffix(key_suffix, val)
                    .expect("disk leaf should have room for cached entry");
            }
            _ => {}
        }
    }

    if let Some(dirty_leaf) = disk_leaf {
        io_engine.write_page(leaf_addr, &dirty_leaf);
    }

    for idx in tombstones.into_iter().rev() {
        node_meta.remove_entry_at(idx);
    }
}

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

impl<'a> WriteGuardWrapper<'a> {
    pub fn try_put(&mut self, cache: &MiniPageBuffer, key: &[u8], val: &[u8]) -> TryPutResult {
        let write_guard = self.get_write_guard();

        match write_guard.node() {
            NodeRef::Leaf(addr) => TryPutResult::NeedsPromotion(addr),
            NodeRef::MiniPage(mini_page_index) => {
                // SAFETY: we hold the write lock for this node
                let node_meta = unsafe { cache.get_meta_mut(mini_page_index) };
                match node_meta.try_put(key, val) {
                    Ok(_) => TryPutResult::Success,
                    Err(_) => TryPutResult::NeedsSplit,
                }
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
        let node_meta = unsafe { buffer.get_meta_mut(index) };

        flush_dirty_entries(node_meta, io_engine);
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

pub enum TryPutResult {
    Success,
    NeedsPromotion(u64),
    NeedsSplit,
}

fn owned_entries(meta: &NodeMeta) -> Vec<LeafEntryOwned> {
    let prefix = meta.get_node_prefix();
    meta.entries()
        .filter(|entry| !entry.meta.fence())
        .map(|entry| LeafEntryOwned::from_entry(prefix, &entry))
        .collect()
}
