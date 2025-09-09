use std::{mem::transmute, path::Prefix, ptr::slice_from_raw_parts, slice};

use crate::types::{KVMeta, NodeMeta};

impl NodeMeta {
    pub fn get(&self, key: &[u8]) -> Option<&[u8]> {
        let target_kv = self.binary_search(self.get_node_prefix().len(), key).ok()?;

        let target_kv = self.get_kv_meta(target_kv);

        match target_kv.typ().exists() {
            true => {
                let val = self.get_val_from_meta(target_kv);
                Some(val)
            }
            false => None,
        }
    }
}

impl NodeMeta {
    #[inline]
    pub fn get_kv_meta(&self, kv_index: usize) -> KVMeta {
        let kv_meta_start = unsafe { transmute::<_, *const KVMeta>(self).offset(1) };
        debug_assert!(kv_index < self.record_count() as usize);
        unsafe { kv_meta_start.offset(kv_index as isize).read() }
    }

    pub fn get_node_prefix(&self) -> &[u8] {
        let low_fence_meta = self.get_kv_meta(0);
        let low_fence_key = self.get_key_from_meta(low_fence_meta);

        let high_fence_meta = self.get_kv_meta(self.record_count() as usize - 1);
        let high_fence_key = self.get_key_from_meta(high_fence_meta);

        let prefix_len = low_fence_key
            .iter()
            .zip(high_fence_key.iter())
            .take_while(|(a, b)| a == b)
            .count();

        let prefix = &low_fence_key[0..prefix_len];
        prefix
    }

    #[inline]
    ///Find the location a key would be in the
    pub fn binary_search(&self, prefix_len: usize, key: &[u8]) -> Result<usize, usize> {
        let target_lookahead = &key[prefix_len];

        // TODO: check how this interacts with endianness
        let target_lookahead = unsafe { transmute::<_, &u16>(target_lookahead) };

        let mut lower = 1usize;
        let mut upper = self.record_count() as usize - 1;
        while upper > lower {
            let mid = lower.midpoint(upper);
            let mid_kv = self.get_kv_meta(mid);

            let mid_lookahead = mid_kv.look_ahead();

            match target_lookahead.cmp(&mid_lookahead) {
                std::cmp::Ordering::Less => {
                    //target is less than mid, so mid is a new upper bound
                    upper = mid - 1
                }
                std::cmp::Ordering::Equal => {
                    // lookahead is not enough

                    let mid_key = self.get_key_from_meta(mid_kv);

                    match key.cmp(mid_key) {
                        std::cmp::Ordering::Less => upper = mid - 1,
                        std::cmp::Ordering::Equal => return Ok(mid),
                        std::cmp::Ordering::Greater => lower = mid + 1,
                    }
                }
                std::cmp::Ordering::Greater => {
                    // target is greater than mid, so mid is a lower bound
                    lower = mid + 1
                }
            }
        }
        Err(lower)
    }

    #[inline]
    pub fn get_key_from_meta(&self, kv: KVMeta) -> &[u8] {
        let base_ptr = self.get_base_ptr();

        let offset = kv.offset() as isize;
        let len = kv.key_size() as usize;

        unsafe { slice::from_raw_parts(base_ptr.offset(offset), len) }
    }

    #[inline]
    pub fn get_val_from_meta(&self, kv: KVMeta) -> &[u8] {
        let base_ptr = self.get_base_ptr();

        let offset = kv.offset() as isize;
        let key_len = kv.key_size() as isize;
        let val_len = kv.val_size() as usize;

        unsafe { slice::from_raw_parts(base_ptr.offset(offset + key_len), val_len) }
    }

    #[inline]
    fn get_base_ptr(&self) -> *const u8 {
        self as *const NodeMeta as *const u8
    }
}
