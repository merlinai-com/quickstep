use quickstep::{debug, QuickStep, QuickStepConfig};
use tempfile::TempDir;

fn new_db() -> QuickStep {
    let temp = TempDir::new().expect("tempdir");
    let config = QuickStepConfig::new(temp.into_path(), 32, 256, 14);
    QuickStep::new(config)
}

fn fill_until_split(db: &QuickStep, inserts: usize, payload: &[u8]) {
    let mut tx = db.tx();
    let mut i = 0usize;
    while debug::split_requests() == 0 && i < inserts {
        let key = format!("key-{i:04}");
        tx.put(key.as_bytes(), payload).expect("insert");
        i += 1;
    }
    tx.commit();
}

fn fill_until_children(db: &QuickStep, target_children: usize, payload: &[u8]) {
    while db
        .debug_root_leaf_parent()
        .map(|snap| snap.children.len())
        .unwrap_or(1)
        < target_children
    {
        let mut tx = db.tx();
        for i in 0..32 {
            let key = format!("grow-{i:04}-{}", debug::split_requests());
            tx.put(key.as_bytes(), payload).expect("insert");
        }
        tx.commit();
    }
}

#[test]
fn root_merge_demotes_to_leaf() {
    debug::reset_debug_counters();
    let db = new_db();
    let payload = vec![0u8; 64];

    fill_until_split(&db, 256, &payload);

    let snapshot = db
        .debug_root_leaf_parent()
        .expect("root should be inner after split");
    assert_eq!(snapshot.children.len(), 2);
    let left = snapshot.children[0];
    let right = snapshot.children[1];

    db.debug_truncate_leaf(left, 3, false).expect("shrink left");
    db.debug_truncate_leaf(right, 2, false)
        .expect("shrink right");

    db.debug_merge_leaves(left, right)
        .expect("merge siblings under root");

    assert!(
        db.debug_root_leaf_parent().is_none(),
        "root should demote back to a single leaf"
    );
    assert_eq!(debug::merge_requests(), 1);
    let events = debug::merge_events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].removed_page, right.as_u64());
    assert_eq!(events[0].survivor_page, left.as_u64());
}

#[test]
fn merge_under_root_reduces_children_without_demotion() {
    debug::reset_debug_counters();
    let db = new_db();
    let payload = vec![0u8; 64];

    fill_until_children(&db, 3, &payload);

    let snapshot = db
        .debug_root_leaf_parent()
        .expect("root should be inner with >=3 children");
    assert!(
        snapshot.children.len() >= 3,
        "expected at least three children"
    );
    let left = snapshot.children[0];
    let middle = snapshot.children[1];

    db.debug_truncate_leaf(left, 2, false)
        .expect("shrink left child");
    db.debug_truncate_leaf(middle, 2, false)
        .expect("shrink middle child");

    db.debug_merge_leaves(left, middle)
        .expect("merge first two children");

    let snapshot = db
        .debug_root_leaf_parent()
        .expect("root should remain inner");
    assert_eq!(snapshot.children.len(), 2);
    assert_eq!(debug::merge_requests(), 1);
    let events = debug::merge_events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].removed_page, middle.as_u64());
}

#[test]
fn auto_merge_triggers_below_threshold() {
    debug::reset_debug_counters();
    let db = new_db();
    let payload = vec![0u8; 64];

    fill_until_split(&db, 256, &payload);

    let snapshot = db
        .debug_root_leaf_parent()
        .expect("root should be inner after split");
    assert_eq!(snapshot.children.len(), 2);
    let left = snapshot.children[0];
    let right = snapshot.children[1];

    db.debug_truncate_leaf(left, 3, false)
        .expect("prepare left underflow");
    db.debug_truncate_leaf(right, 2, true)
        .expect("auto-merge trigger right");

    assert!(
        debug::merge_requests() >= 1,
        "auto-merge should have been recorded"
    );
    let events = debug::merge_events();
    assert!(
        events
            .iter()
            .any(|event| event.removed_page == right.as_u64())
            || events
                .iter()
                .any(|event| event.removed_page == left.as_u64()),
        "merge event should mention one of the siblings"
    );
}

#[test]
fn delete_api_triggers_auto_merge() {
    debug::reset_debug_counters();
    let db = new_db();
    let payload = vec![0u8; 128];

    fill_until_split(&db, 256, &payload);

    let snapshot = db
        .debug_root_leaf_parent()
        .expect("root should be inner after split");
    let pivot = snapshot.pivots[0].clone();

    for i in 0..256 {
        let key = format!("key-{i:04}");
        if key.as_bytes() >= pivot.as_slice() {
            assert!(db.delete(key.as_bytes()).expect("delete attempt"));
        }
    }

    assert!(
        debug::merge_requests() >= 1,
        "delete-driven underflow should trigger auto-merge"
    );
    assert!(
        db.debug_root_leaf_parent().is_none(),
        "root should demote once right leaf is reclaimed"
    );
}

#[test]
fn cascading_merge_reduces_deeper_tree() {
    debug::reset_debug_counters();
    let db = new_db();
    let payload = vec![0u8; 64];

    fill_until_children(&db, 4, &payload);

    let snapshot = db
        .debug_root_leaf_parent()
        .expect("root should be inner with multiple children");
    assert!(snapshot.children.len() >= 4);
    let left = snapshot.children[0];
    let mid_left = snapshot.children[1];
    let mid_right = snapshot.children[2];

    db.debug_truncate_leaf(left, 2, true)
        .expect("auto-truncate left");
    db.debug_truncate_leaf(mid_left, 2, true)
        .expect("auto-truncate mid-left");
    db.debug_truncate_leaf(mid_right, 2, true)
        .expect("auto-truncate mid-right");

    assert!(
        db.debug_root_leaf_parent()
            .map(|snap| snap.children.len())
            .unwrap_or(1)
            <= 2,
        "root should have collapsed after cascading merges"
    );
    assert!(
        debug::merge_requests() >= 2,
        "expect multiple merges to cascade"
    );
}
