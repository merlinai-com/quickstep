use quickstep::{debug, map_table::PageId, QuickStep, QuickStepConfig};
use std::collections::HashSet;
use tempfile::TempDir;

fn new_db() -> QuickStep {
    let temp = TempDir::new().expect("tempdir");
    let config = QuickStepConfig::new(temp.into_path(), 32, 256, 14);
    QuickStep::new(config)
}

fn new_small_cache_db() -> QuickStep {
    let temp = TempDir::new().expect("tempdir");
    let config = QuickStepConfig::new(temp.into_path(), 32, 256, 13);
    QuickStep::new(config)
}

fn drive_root_split(db: &QuickStep) -> Vec<PageId> {
    let payload = vec![0u8; 1024];
    let mut tx = db.tx();
    let mut inserted = 0usize;
    while debug::split_requests() == 0 {
        assert!(inserted < 128, "expected split within 128 inserts");
        let key = format!("key-{inserted:04}");
        tx.put(key.as_bytes(), &payload).expect("insert");
        inserted += 1;
    }
    tx.commit();

    let parent = db
        .debug_root_leaf_parent()
        .expect("root should have promoted to an inner node");
    assert_eq!(parent.children.len(), 2, "expected two children after split");
    parent.children
}

fn assert_sentinel_fences(db: &QuickStep, page_id: PageId) {
    let fences = db
        .debug_leaf_fences(page_id)
        .unwrap_or_else(|_| panic!("missing fences for page {}", page_id.as_u64()));
    assert_eq!(
        fences.lower,
        vec![0x00],
        "lower fence must remain sentinel 0x00 for page {}",
        page_id.as_u64()
    );
    assert_eq!(
        fences.upper,
        vec![0xFF],
        "upper fence must remain sentinel 0xFF for page {}",
        page_id.as_u64()
    );
}

#[test]
fn root_leaf_contains_sentinel_fences() {
    let db = new_db();
    assert_sentinel_fences(&db, PageId::from_u64(0));
}

#[test]
fn split_children_keep_sentinel_fences() {
    debug::reset_debug_counters();
    let db = new_db();
    let children = drive_root_split(&db);
    assert_sentinel_fences(&db, children[0]);
    assert_sentinel_fences(&db, children[1]);
}

#[test]
fn merge_survivor_retains_sentinel_fences() {
    debug::reset_debug_counters();
    let db = new_db();
    let children = drive_root_split(&db);

    db.debug_merge_leaves(children[0], children[1])
        .expect("merge should succeed");

    assert_sentinel_fences(&db, children[0]);
    assert_sentinel_fences(&db, children[1]);
}

#[test]
fn eviction_preserves_sentinel_fences() {
    debug::reset_debug_counters();
    let db = new_small_cache_db();
    let payload = vec![0u8; 1024];

    {
        let mut tx = db.tx();
        for i in 0..256 {
            let key = format!("key-{i:04}");
            tx.put(key.as_bytes(), &payload).expect("insert");
        }
        tx.commit();
    }

    assert!(
        debug::evictions() > 0,
        "small cache should evict at least one leaf"
    );

    let mut pages: HashSet<u64> = HashSet::new();
    pages.insert(0);
    for event in debug::split_events() {
        pages.insert(event.left_page);
        pages.insert(event.right_page);
    }

    for page in pages {
        assert_sentinel_fences(&db, PageId::from_u64(page));
    }
}

