use quickstep::{
    buffer::{MiniPageBuffer, MiniPageIndex},
    map_table::PageId,
    types::NodeSize,
};

fn new_cache() -> MiniPageBuffer {
    // 2^12 bytes = 4 KiB, enough for a single leaf page.
    MiniPageBuffer::new(12)
}

#[test]
fn dealloc_reuses_slot_via_freelist() {
    let cache = new_cache();
    let idx = cache
        .alloc(NodeSize::LeafPage)
        .expect("allocate first leaf page");

    unsafe {
        let meta = cache.get_meta_mut(MiniPageIndex::new(idx));
        meta.reset_header(PageId::from_u64(0), NodeSize::LeafPage, 0);
        meta.set_live(false);
    }

    unsafe {
        cache.dealloc(MiniPageIndex::new(idx));
    }

    let reused = cache
        .alloc(NodeSize::LeafPage)
        .expect("allocate from freelist");
    assert_eq!(reused, idx, "freelist should return the recycled slot");
}

