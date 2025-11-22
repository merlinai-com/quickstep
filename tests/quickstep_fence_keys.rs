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

fn drive_root_split(db: &QuickStep) -> (Vec<PageId>, Vec<u8>, usize) {
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
    (parent.children, parent.pivots[0].clone(), inserted)
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

fn assert_bounds_cover_keys(db: &QuickStep, page_id: PageId) {
    let fences = db
        .debug_leaf_fences(page_id)
        .unwrap_or_else(|_| panic!("missing fences for page {}", page_id.as_u64()));
    let snapshot = db
        .debug_leaf_snapshot(page_id)
        .unwrap_or_else(|_| panic!("missing snapshot for page {}", page_id.as_u64()));
    if snapshot.keys.is_empty() {
        return;
    }
    let min_key = snapshot
        .keys
        .iter()
        .min()
        .expect("snapshot already checked non-empty");
    let max_key = snapshot
        .keys
        .iter()
        .max()
        .expect("snapshot already checked non-empty");
    assert!(
        fences.lower.as_slice() <= min_key.as_slice(),
        "lower fence {:?} must be <= min key {:?} for page {}",
        fences.lower,
        min_key,
        page_id.as_u64()
    );
    assert!(
        max_key.as_slice() < fences.upper.as_slice(),
        "upper fence {:?} must be > max key {:?} for page {}",
        fences.upper,
        max_key,
        page_id.as_u64()
    );
}

#[test]
fn root_leaf_contains_sentinel_fences() {
    let db = new_db();
    assert_sentinel_fences(&db, PageId::from_u64(0));
}

#[test]
fn split_children_receive_parent_bounds() {
    debug::reset_debug_counters();
    let db = new_db();
    let (children, pivot, _) = drive_root_split(&db);

    let left = db.debug_leaf_fences(children[0]).expect("left fences");
    assert_eq!(left.lower, vec![0x00], "left child lower fence should be -inf");
    assert_eq!(
        left.upper, pivot,
        "left child upper fence should equal pivot"
    );
    assert_bounds_cover_keys(&db, children[0]);

    let right = db.debug_leaf_fences(children[1]).expect("right fences");
    assert_eq!(right.upper, vec![0xFF], "right child upper fence should be +inf");
    assert_eq!(
        right.lower, pivot,
        "right child lower fence should equal pivot"
    );
    assert_bounds_cover_keys(&db, children[1]);
}

#[test]
fn merge_survivor_spans_full_bounds() {
    debug::reset_debug_counters();
    let db = new_db();
    let (children, _, _) = drive_root_split(&db);

    db.debug_merge_leaves(children[0], children[1])
        .expect("merge should succeed");

    assert_sentinel_fences(&db, children[0]);
    assert_bounds_cover_keys(&db, children[0]);
}

#[test]
fn eviction_preserves_fence_monotonicity() {
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
        assert_bounds_cover_keys(&db, PageId::from_u64(page));
    }
}

#[test]
fn delete_auto_merge_preserves_fence_monotonicity() {
    debug::reset_debug_counters();
    let db = new_db();
    let payload = vec![0u8; 128];

    let (_children, pivot, inserted) = drive_root_split(&db);

    {
        let mut tx = db.tx();
        for i in inserted..(inserted + 64) {
            let key = format!("key-{i:04}");
            tx.put(key.as_bytes(), &payload)
                .expect("insert after split");
        }
        tx.commit();
    }

    let snapshot = db
        .debug_root_leaf_parent()
        .expect("tree should still have inner root");
    assert_eq!(snapshot.children.len(), 2, "expected exactly two children");

    for i in 0..(inserted + 64) {
        let key = format!("key-{i:04}");
        if key.as_bytes() >= pivot.as_slice() {
            assert!(
                db.delete(key.as_bytes()).expect("delete operation"),
                "expected delete to remove {key}"
            );
        }
    }

    assert!(
        debug::merge_requests() >= 1,
        "delete-driven underflow should trigger auto merge"
    );

    if let Some(parent) = db.debug_root_leaf_parent() {
        for child in parent.children {
            assert_bounds_cover_keys(&db, child);
        }
    } else {
        assert_bounds_cover_keys(&db, PageId::from_u64(0));
    }
}

