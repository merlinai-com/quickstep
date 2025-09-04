use std::{mem::transmute, path::Prefix, ptr::slice_from_raw_parts, slice};

use crate::types::{KVMeta, NodeMeta};

impl NodeMeta {
    /// This must be called on a NodeMeta at the start of a correctly sized mini-page or leaf
    pub unsafe fn get(&self, key: &[u8]) -> Option<&[u8]> {
        let record_cnt = self.record_count();

        debug_assert!(
            record_cnt > 3,
            "There should always be two fences and at least one real entry"
        );

        let kv_meta_start = transmute::<_, *const KVMeta>(self).offset(1);
        // let kv_meta_arr = slice_from_raw_parts(kv_meta_start, record_cnt as usize);

        let low_fence_meta = kv_meta_start.read();
        let low_fence_key = self.get_key_from_meta(low_fence_meta);

        let high_fence_meta = kv_meta_start.offset(record_cnt as isize - 1).read();
        let high_fence_key = self.get_key_from_meta(high_fence_meta);

        let prefix_len = low_fence_key
            .iter()
            .zip(high_fence_key.iter())
            .take_while(|(a, b)| a == b)
            .count();

        let prefix = &low_fence_key[0..prefix_len];
        debug_assert!(
            key.starts_with(prefix),
            "the target key should share the common prefix, if its made it this far"
        );

        let target_lookahead = &key[prefix_len];
        let target_lookahead = transmute::<_, &u16>(target_lookahead);

        let mut lower = 1usize;
        let mut upper = record_cnt as usize - 1;

        while upper > lower {
            let mid = lower.midpoint(upper);
            let mid_kv = kv_meta_start.offset(mid as isize).read();

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
                        std::cmp::Ordering::Equal => return Some(self.get_key_from_meta(mid_kv)),
                        std::cmp::Ordering::Greater => lower = mid + 1,
                    }
                }
                std::cmp::Ordering::Greater => {
                    // target is greater than mid, so mid is a lower bound
                    lower = mid + 1
                }
            }
        }

        None
    }

    unsafe fn get_key_from_meta(&self, kv: KVMeta) -> &[u8] {
        let base_ptr = transmute::<_, *const u8>(self);

        let offset = kv.offset() as isize;
        let len = kv.key_size() as usize;

        slice::from_raw_parts(base_ptr.offset(offset), len)
    }

    unsafe fn get_val_from_meta(&self, kv: KVMeta) -> &[u8] {
        let base_ptr = transmute::<_, *const u8>(self);

        let offset = kv.offset() as isize;
        let key_len = kv.key_size() as isize;
        let val_len = kv.val_size() as usize;

        slice::from_raw_parts(base_ptr.offset(offset + key_len), val_len)
    }
}
