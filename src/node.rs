use std::{
    ptr::copy,
    slice,
    sync::atomic::{AtomicU64, Ordering},
};

use crate::{
    map_table::PageId,
    types::{KVMeta, KVRecordType, NodeMeta, NodeSize},
};

// TODO: need to read node meta atomically

impl NodeMeta {
    /// Drop all non-fence entries while keeping the existing fence keys.
    /// After calling this, the node only contains its lower and upper fence,
    /// so callers can replay a new set of user records using the normal insert path.
    pub fn reset_user_entries(&mut self) {
        if self.record_count() <= 2 {
            return;
        }
        let (lower, upper) = self.fence_bounds();
        self.reset_user_entries_with_fences(&lower, &upper);
    }

    pub fn reset_user_entries_with_fences(&mut self, lower: &[u8], upper: &[u8]) {
        self.install_fences(lower, upper);
    }

    pub fn ensure_fence_keys(&mut self) {
        if self.record_count() >= 2 {
            return;
        }
        const LOWER_FENCE: [u8; 1] = [0x00];
        const UPPER_FENCE: [u8; 1] = [0xFF];
        self.install_fences(&LOWER_FENCE, &UPPER_FENCE);
    }

    pub fn format_leaf(&mut self, page_id: PageId, size: NodeSize, disk_addr: u64) {
        self.reset_header(page_id, size, disk_addr);
        self.ensure_fence_keys();
    }

    /// Reinsert the provided entries (full user keys) using the existing try_put
    /// logic so that prefix compression and bookkeeping remain consistent.
    pub fn replay_entries<'a, I>(&mut self, entries: I) -> Result<(), InsufficientSpace>
    where
        I: IntoIterator<Item = (&'a [u8], &'a [u8])>,
    {
        for (key, value) in entries {
            self.try_put(key, value)?;
        }
        Ok(())
    }

    pub fn get(&self, key: &[u8]) -> Option<&[u8]> {
        let prefix = self.get_node_prefix();
        debug_assert!(key.starts_with(prefix));
        let target_kv = self.binary_search(&key[prefix.len()..]).ok()?;

        let target_kv = self.get_kv_meta(target_kv);

        match target_kv.typ().exists() {
            true => {
                let val = self.get_val_from_meta(target_kv);
                Some(val)
            }
            false => None,
        }
    }

    // TODO: refactor with suffix implementation
    pub fn try_put(&mut self, key: &[u8], val: &[u8]) -> Result<(), InsufficientSpace> {
        debug_assert!(
            self.record_count() >= 2,
            "node missing fence keys before try_put"
        );
        let node_prefix = self.get_node_prefix();
        let node_prefix_len = node_prefix.len();
        let key_suffix = &key[node_prefix_len..];
        debug_assert!(key.starts_with(node_prefix));
        self.try_put_with_suffix(key_suffix, val)
    }

    pub fn user_entry_count(&self) -> usize {
        self.entries()
            .filter(|entry| entry.meta.typ().exists())
            .count()
    }

    pub fn mark_tombstone(&mut self, key: &[u8]) -> bool {
        let prefix = self.get_node_prefix();
        if !key.starts_with(prefix) {
            return false;
        }
        let suffix = &key[prefix.len()..];
        match self.binary_search(suffix) {
            Ok(idx) => self.mark_entry_tombstone(idx),
            Err(_) => false,
        }
    }

    pub fn remove_entry_at(&mut self, idx: usize) {
        self.remove_entry(idx);
    }

    fn mark_entry_tombstone(&mut self, idx: usize) -> bool {
        let mut kv = self.get_kv_meta(idx);
        if kv.fence() || kv.typ() == KVRecordType::Tombstone {
            return false;
        }
        kv = kv.set_record_type(KVRecordType::Tombstone);
        self.set_kv_meta(idx, kv);
        true
    }

    pub fn remove_key_physical(&mut self, key: &[u8]) -> bool {
        let prefix = self.get_node_prefix();
        if !key.starts_with(prefix) {
            return false;
        }
        let suffix = &key[prefix.len()..];
        match self.binary_search(suffix) {
            Ok(idx) => self.remove_entry(idx),
            Err(_) => false,
        }
    }

    fn remove_entry(&mut self, idx: usize) -> bool {
        if idx >= self.record_count() as usize {
            return false;
        }
        let kv = self.get_kv_meta(idx);
        if kv.fence() {
            return false;
        }
        unsafe {
            self.erase_kv_in_buffer(kv);
        }
        self.shift_meta_left(idx);
        self.dec_record_count();
        true
    }

    fn shift_meta_left(&mut self, idx: usize) {
        let total = self.record_count() as usize;
        if idx + 1 >= total {
            return;
        }
        let kv_meta_start = unsafe { (self as *const NodeMeta).add(1) as *mut AtomicU64 };
        unsafe {
            kv_meta_start
                .add(idx + 1)
                .copy_to(kv_meta_start.add(idx), total - idx - 1);
        }
    }

    pub fn try_put_with_suffix(
        &mut self,
        key_suffix: &[u8],
        val: &[u8],
    ) -> Result<(), InsufficientSpace> {
        // TODO: copy old value for abort
        match self.binary_search(key_suffix) {
            // Value already exists, so update with kv meta in place
            Ok(idx) => {
                let mut target_kv = self.get_kv_meta(idx);
                match target_kv.val_size() as usize == val.len() {
                    true => {
                        // Don't need to change layout, just rewrite
                        let val_slice = self.get_val_mut_from_meta(target_kv);
                        val_slice.copy_from_slice(val);
                    }
                    false => {
                        // different length: shift other entries, then rewrite

                        let alloc_ptr = unsafe { self.erase_kv_in_buffer(target_kv) };

                        let new_size = key_suffix.len() + val.len();
                        let new_offset = alloc_ptr - new_size;

                        // Add 1 to account for Node meta
                        let meta_end = (self.record_count() as usize + 1) * size_of::<KVMeta>();

                        if new_offset < meta_end {
                            return Err(InsufficientSpace);
                        }

                        // update metadata
                        let _ = target_kv.set_offset(new_offset as u16);
                        let _ = target_kv.set_val_size(val.iter().len() as u16);
                        target_kv = target_kv.set_ref_bit(true);
                        self.set_kv_meta(idx, target_kv);

                        self.get_key_mut_from_meta(target_kv)
                            .copy_from_slice(key_suffix);
                        self.get_val_mut_from_meta(target_kv).copy_from_slice(val);
                    }
                }
            }
            Err(idx) => {
                // check there's enough space, then move the kvmetas then add value

                let size = key_suffix.len() + val.len();
                let min_offset = self.find_min_offset();
                let new_offset = min_offset.checked_sub(size).ok_or(InsufficientSpace)?;

                // add 1 for NodeMeta and one for new KVMeta
                let meta_end = (self.record_count() as usize + 2) * size_of::<KVMeta>();

                if new_offset < meta_end {
                    return Err(InsufficientSpace);
                }

                debug_assert!(idx <= self.record_count() as usize);
                let kv_meta_start = unsafe { (self as *const NodeMeta).add(1) as *const AtomicU64 };
                let from_ptr = unsafe { kv_meta_start.add(idx) };
                let to_ptr = unsafe { kv_meta_start.add(idx + 1) as *mut AtomicU64 };
                // TODO: check for off by 1
                // TODO: switch to atomic loop, to account for evicting threads that will come and clear ref bits
                // Though this is unlikely as copy-on-access should make it unlikely that this will be in second chance region
                unsafe {
                    from_ptr.copy_to(to_ptr, self.record_count() as usize - idx);
                }

                let new_meta = KVMeta::new(
                    key_suffix.len(),
                    val.len(),
                    new_offset,
                    KVRecordType::Insert,
                    false,
                    true,
                    get_lookahead(key_suffix),
                );

                self.set_kv_meta(idx, new_meta);
                self.inc_record_count();

                self.get_key_mut_from_meta(new_meta)
                    .copy_from_slice(key_suffix);
                self.get_val_mut_from_meta(new_meta).copy_from_slice(val);
            }
        }
        Ok(())
    }
}

impl NodeMeta {
    #[inline]
    pub fn get_kv_meta_ref(&self, kv_index: usize) -> &AtomicU64 {
        let kv_meta_start = unsafe { (self as *const NodeMeta).add(1) as *const AtomicU64 };
        debug_assert!(kv_index < self.record_count() as usize);
        unsafe { &*kv_meta_start.add(kv_index) }
    }

    #[inline]
    pub fn get_kv_meta(&self, kv_index: usize) -> KVMeta {
        KVMeta(self.get_kv_meta_ref(kv_index).load(Ordering::Relaxed))
    }

    // Gets the requested KVMeta, ensuring that the ref bit is set
    #[inline]
    pub fn get_kv_meta_ensure_ref(&self, kv_index: usize) -> KVMeta {
        let kv_ref = self.get_kv_meta_ref(kv_index);
        let mut out = KVMeta(kv_ref.load(Ordering::Relaxed));
        if out.ref_bit() {
            let new = out.clone().set_ref_bit(true);
            match kv_ref.compare_exchange(out.0, new.0, Ordering::Relaxed, Ordering::Relaxed) {
                Ok(_) => out = new,
                Err(act) => {
                    // There might be a race if node is read and is in the in place sector, then another thread might intervene and start eviction
                    // but this should still only affect the ref bit
                    debug_assert_eq!(
                        KVMeta(act).set_ref_bit(true).0,
                        new.0,
                        "Concurrent operations on KVMeta should only modify ref bit"
                    );
                    out = KVMeta(act);
                }
            }
        }
        out
    }

    #[inline]
    pub fn set_kv_meta(&self, kv_index: usize, val: KVMeta) {
        self.get_kv_meta_ref(kv_index)
            .store(val.0, Ordering::Relaxed)
    }

    pub fn get_node_prefix(&self) -> &[u8] {
        let low_fence_meta = self.get_kv_meta(0);
        let low_fence_key = self.get_stored_key_from_meta(low_fence_meta);

        let high_fence_meta = self.get_kv_meta(self.record_count() as usize - 1);
        let high_fence_key = self.get_stored_key_from_meta(high_fence_meta);

        let prefix_len = low_fence_key
            .iter()
            .zip(high_fence_key.iter())
            .take_while(|(a, b)| a == b)
            .count();

        let prefix = &low_fence_key[0..prefix_len];
        prefix
    }

    #[inline]
    ///Find the location a key would be in the node, by the suffix, accounting for prefix compresion
    pub fn binary_search(&self, key_suffix: &[u8]) -> Result<usize, usize> {
        let target_lookahead = get_lookahead(key_suffix);

        if self.record_count() <= 2 {
            return Err(1);
        }

        let mut lower = if self.get_kv_meta(0).fence() { 1 } else { 0 };
        let mut upper = self.record_count() as usize - 1;
        if self.get_kv_meta(upper).fence() {
            upper = upper.saturating_sub(1);
        }

        if lower > upper {
            return Err(lower);
        }

        while lower <= upper {
            let mid = lower + ((upper - lower) / 2);
            let mid_kv = self.get_kv_meta(mid);

            let mid_lookahead = mid_kv.look_ahead();

            match target_lookahead.cmp(&mid_lookahead) {
                std::cmp::Ordering::Less => {
                    if mid == 0 {
                        break;
                    }
                    upper = mid - 1;
                }
                std::cmp::Ordering::Equal => {
                    // lookahead is not enough

                    let mid_key_suffix = self.get_stored_key_from_meta(mid_kv);

                    match key_suffix.cmp(mid_key_suffix) {
                        std::cmp::Ordering::Less => {
                            if mid == 0 {
                                break;
                            }
                            upper = mid - 1;
                        }
                        std::cmp::Ordering::Equal => return Ok(mid),
                        std::cmp::Ordering::Greater => lower = mid + 1,
                    }
                }
                std::cmp::Ordering::Greater => {
                    // target is greater than mid, so mid is a lower bound
                    lower = mid + 1;
                }
            }
        }
        Err(lower)
    }

    /// Erase the key value data in a buffer, while keeping the kvmeta
    /// Returns the new min offset
    unsafe fn erase_kv_in_buffer(&mut self, kv: KVMeta) -> usize {
        let base_ptr = self.get_base_ptr() as *mut u8;
        let len = (kv.key_size() + kv.val_size()) as usize;
        let target_offset = kv.offset();
        let mut min_offset = target_offset;

        for i in 0..self.record_count() as usize {
            let mut kv = self.get_kv_meta(i);
            let cur_offset = kv.offset();

            if cur_offset < target_offset {
                min_offset = min_offset.min(cur_offset);
                let new_offset = cur_offset + len;
                let _ = kv.set_offset(new_offset as u16);
                let _ = kv.set_offset(new_offset as u16);
                self.set_kv_meta(i, kv);
            }
        }

        if min_offset == target_offset {
            return target_offset + len;
        }

        let src_ptr = base_ptr.add(min_offset);

        let dst_ptr = base_ptr.add(min_offset + len);

        copy(src_ptr, dst_ptr, len);

        min_offset + len
    }

    fn find_min_offset(&self) -> usize {
        // let mut min = self.get_kv_meta(0).offset();
        // for 1..self.record_count() {}
        (0..self.record_count())
            .map(|i| self.get_kv_meta(i as usize).offset())
            .min()
            .expect("There should always be at least 2 fence keys") as usize
    }

    /// Gets the key, not including the prefix
    #[inline]
    pub fn get_stored_key_from_meta(&self, kv: KVMeta) -> &[u8] {
        let base_ptr = self.get_base_ptr();

        let offset = kv.offset() as isize;
        let len = kv.key_size() as usize;

        unsafe { slice::from_raw_parts(base_ptr.offset(offset), len) }
    }

    #[inline]
    pub fn get_key_mut_from_meta(&self, kv: KVMeta) -> &mut [u8] {
        let base_ptr = self.get_base_ptr() as *mut u8;

        let offset = kv.offset() as isize;
        let len = kv.key_size() as usize;

        unsafe { slice::from_raw_parts_mut(base_ptr.offset(offset), len) }
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
    pub fn get_val_mut_from_meta(&mut self, kv: KVMeta) -> &mut [u8] {
        let base_ptr = self.get_base_ptr() as *mut u8;

        let offset = kv.offset() as isize;
        let key_len = kv.key_size() as isize;
        let val_len = kv.val_size() as usize;

        unsafe { slice::from_raw_parts_mut(base_ptr.offset(offset + key_len), val_len) }
    }

    #[inline]
    fn get_base_ptr(&self) -> *const u8 {
        self as *const NodeMeta as *const u8
    }

    pub fn fence_bounds(&self) -> (Vec<u8>, Vec<u8>) {
        let lower_meta = self.get_kv_meta(0);
        let upper_meta = self.get_kv_meta(self.record_count() as usize - 1);
        (
            self.get_stored_key_from_meta(lower_meta).to_vec(),
            self.get_stored_key_from_meta(upper_meta).to_vec(),
        )
    }

    fn install_fences(&mut self, lower: &[u8], upper: &[u8]) {
        let mut cursor = self.size().size_in_bytes();
        let base_ptr = self.get_base_ptr() as *mut u8;

        cursor -= upper.len();
        unsafe {
            base_ptr
                .add(cursor)
                .copy_from_nonoverlapping(upper.as_ptr(), upper.len());
        }
        let upper_offset = cursor as u16;

        cursor -= lower.len();
        unsafe {
            base_ptr
                .add(cursor)
                .copy_from_nonoverlapping(lower.as_ptr(), lower.len());
        }
        let lower_offset = cursor as u16;

        self.set_record_count(2);

        let mut lower_meta = KVMeta::new(lower.len(), 0, 0, KVRecordType::Cache, true, true, 0);
        let _ = lower_meta.set_offset(lower_offset);
        self.set_kv_meta(0, lower_meta);

        let mut upper_meta = KVMeta::new(upper.len(), 0, 0, KVRecordType::Cache, true, true, 0);
        let _ = upper_meta.set_offset(upper_offset);
        self.set_kv_meta(1, upper_meta);
    }
}

#[inline]
fn get_lookahead(key_suffix: &[u8]) -> u16 {
    // allow default if key is the prefix (not sure if this is possible), or only 1 byte longer
    let b0 = key_suffix.get(0).copied().unwrap_or_default();
    let b1 = key_suffix.get(1).copied().unwrap_or_default();
    u16::from_be_bytes([b0, b1])
}

#[derive(Debug)]
pub struct InsufficientSpace;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_try_put_roundtrip() {
        let mut buf = vec![0u8; NodeSize::LeafPage.size_in_bytes()];
        let meta = unsafe { &mut *(buf.as_mut_ptr() as *mut NodeMeta) };
        meta.format_leaf(PageId(0), NodeSize::LeafPage, 0);

        meta.try_put(b"alpha", b"one").expect("insert alpha");
        meta.try_put(b"beta", b"two").expect("insert beta");
        meta.try_put(b"gamma", b"three").expect("insert gamma");

        assert_eq!(meta.get(b"alpha"), Some(b"one".as_ref()));
        assert_eq!(meta.get(b"beta"), Some(b"two".as_ref()));
        assert_eq!(meta.get(b"gamma"), Some(b"three".as_ref()));
        assert_eq!(meta.get(b"delta"), None);
    }
}
