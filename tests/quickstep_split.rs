use quickstep::{debug, QuickStep, QuickStepConfig};
use tempfile::TempDir;

fn new_db() -> QuickStep {
    let temp = TempDir::new().expect("tempdir");
    // keep the same parameters as other integration tests
    let config = QuickStepConfig::new(temp.into_path(), 32, 256, 14);
    QuickStep::new(config)
}

#[test]
fn root_split_occurs_and_is_readable() {
    debug::reset_debug_counters();
    let db = new_db();

    let mut tx = db.tx();
    // Payload large enough to trigger a split within a few dozen inserts.
    let payload = vec![0u8; 1024];
    let mut inserted = 0usize;
    while debug::split_requests() == 0 {
        assert!(inserted < 128, "expected a root split within 128 inserts");
        let key = format!("key-{inserted:04}");
        tx.put(key.as_bytes(), &payload).expect("insert");
        inserted += 1;
    }
    tx.commit();

    assert_eq!(
        debug::split_requests(),
        1,
        "expected exactly one split while filling the root"
    );

    let snapshot = db
        .debug_root_leaf_parent()
        .expect("root should have been promoted to an inner node");
    assert_eq!(
        snapshot.children.len(),
        snapshot.pivots.len() + 1,
        "children should be pivots + 1"
    );
    assert_eq!(
        snapshot.children.len(),
        2,
        "expect exactly two children after first split"
    );

    let events = debug::split_events();
    assert_eq!(events.len(), 1, "expected exactly one split event recorded");
    assert_eq!(
        snapshot.children[0].as_u64(),
        events[0].left_page,
        "left child should match recorded split origin"
    );
    assert_eq!(
        snapshot.children[1].as_u64(),
        events[0].right_page,
        "right child should match recorded split sibling"
    );
    let pivot = snapshot.pivots[0].clone();
    let left_snapshot = db
        .debug_leaf_snapshot(snapshot.children[0])
        .expect("left child snapshot");
    let right_snapshot = db
        .debug_leaf_snapshot(snapshot.children[1])
        .expect("right child snapshot");
    assert!(
        left_snapshot
            .keys
            .iter()
            .all(|key| key.as_slice() < pivot.as_slice()),
        "all left-child keys must be < pivot"
    );
    assert!(
        right_snapshot
            .keys
            .iter()
            .all(|key| key.as_slice() >= pivot.as_slice()),
        "all right-child keys must be >= pivot"
    );

    let mut read_tx = db.tx();
    for i in 0..inserted {
        let key = format!("key-{i:04}");
        assert!(
            read_tx.get(key.as_bytes()).unwrap().is_some(),
            "missing key {key}"
        );
    }
    read_tx.commit();
}

#[test]
fn post_split_inserts_route_to_expected_children() {
    debug::reset_debug_counters();
    let db = new_db();
    let payload = vec![0u8; 1024];
    let mut inserted = 0usize;

    {
        let mut tx = db.tx();
        while debug::split_requests() == 0 {
            assert!(inserted < 128, "expected a root split within 128 inserts");
            let key = format!("key-{inserted:04}");
            tx.put(key.as_bytes(), &payload)
                .expect("insert before split");
            inserted += 1;
        }
        tx.commit();
    }

    assert_eq!(debug::split_requests(), 1);
    let snapshot = db
        .debug_root_leaf_parent()
        .expect("root should have been promoted");
    let pivot = snapshot.pivots[0].clone();
    let pivot_idx = parse_key_index(&pivot);
    assert!(
        pivot_idx > 0,
        "split pivot must be greater than zero for range tests"
    );
    let left_key = format!("key-{:04}-lo", pivot_idx - 1);
    let right_key = format!("key-{:04}-hi", pivot_idx + 1);

    {
        let mut tx = db.tx();
        tx.put(left_key.as_bytes(), &payload)
            .expect("left-side insert after split");
        tx.put(right_key.as_bytes(), &payload)
            .expect("right-side insert after split");
        tx.commit();
    }
    assert_eq!(
        debug::split_requests(),
        1,
        "follow-up inserts should not trigger extra splits"
    );

    let snapshot = db
        .debug_root_leaf_parent()
        .expect("root should remain an inner node");
    assert_eq!(snapshot.children.len(), 2);
    let left_child = snapshot.children[0];
    let right_child = snapshot.children[1];

    let left_snapshot = db
        .debug_leaf_snapshot(left_child)
        .expect("left child snapshot");
    let right_snapshot = db
        .debug_leaf_snapshot(right_child)
        .expect("right child snapshot");

    assert!(
        left_snapshot
            .keys
            .iter()
            .any(|key| key.as_slice() == left_key.as_bytes()),
        "left child should contain the left-side insert"
    );
    assert!(
        right_snapshot
            .keys
            .iter()
            .any(|key| key.as_slice() == right_key.as_bytes()),
        "right child should contain the right-side insert"
    );

    let mut read_tx = db.tx();
    assert!(
        read_tx.get(left_key.as_bytes()).unwrap().is_some(),
        "left-side key should be readable after routing"
    );
    assert!(
        read_tx.get(right_key.as_bytes()).unwrap().is_some(),
        "right-side key should be readable after routing"
    );
    read_tx.commit();
}

#[test]
fn second_split_under_root_adds_third_child() {
    debug::reset_debug_counters();
    let db = new_db();

    let payload = vec![0u8; 1024];
    let mut inserted = 0usize;

    {
        let mut tx = db.tx();
        while debug::split_requests() == 0 {
            assert!(
                inserted < 128,
                "expected the first split within the first 128 inserts"
            );
            let key = format!("key-{inserted:04}");
            tx.put(key.as_bytes(), &payload).expect("insert");
            inserted += 1;
        }
        tx.commit();
    }

    assert_eq!(debug::split_requests(), 1);

    {
        let mut tx = db.tx();
        while debug::split_requests() == 1 {
            assert!(
                inserted < 512,
                "expected the second split before 512 inserts"
            );
            let key = format!("key-{inserted:04}");
            tx.put(key.as_bytes(), &payload).expect("insert");
            inserted += 1;
        }
        tx.commit();
    }

    assert_eq!(debug::split_requests(), 2);

    let events = debug::split_events();
    assert_eq!(events.len(), 2, "expected two split events logged");
    assert_eq!(
        events[1].left_page, events[0].right_page,
        "second split should occur on the right sibling created by the first split"
    );

    let snapshot = db
        .debug_root_leaf_parent()
        .expect("root should remain an inner node");
    assert_eq!(
        snapshot.children.len(),
        3,
        "root should now reference three children after second split"
    );
    assert_eq!(
        snapshot.pivots.len(),
        2,
        "root pivots should be the number of children minus one"
    );
    assert_eq!(
        snapshot.children[0].as_u64(),
        events[0].left_page,
        "leftmost child should remain the original root page"
    );
    assert_eq!(
        snapshot.children[1].as_u64(),
        events[0].right_page,
        "middle child should be the sibling created by the first split"
    );
    assert_eq!(
        snapshot.children[2].as_u64(),
        events[1].right_page,
        "new rightmost child should match the second split output"
    );

    let low_pivot = snapshot.pivots[0].clone();
    let high_pivot = snapshot.pivots[1].clone();
    let left_snapshot = db
        .debug_leaf_snapshot(snapshot.children[0])
        .expect("left snapshot");
    let middle_snapshot = db
        .debug_leaf_snapshot(snapshot.children[1])
        .expect("middle snapshot");
    let right_snapshot = db
        .debug_leaf_snapshot(snapshot.children[2])
        .expect("right snapshot");
    assert!(
        left_snapshot
            .keys
            .iter()
            .all(|key| key.as_slice() < low_pivot.as_slice()),
        "left child must stay below the first pivot"
    );
    assert!(
        middle_snapshot.keys.iter().all(|key| {
            let ks = key.as_slice();
            ks >= low_pivot.as_slice() && ks < high_pivot.as_slice()
        }),
        "middle child must stay between the first and second pivots"
    );
    assert!(
        right_snapshot
            .keys
            .iter()
            .all(|key| key.as_slice() >= high_pivot.as_slice()),
        "right child must stay at or above the second pivot"
    );

    let mut read_tx = db.tx();
    for i in 0..inserted {
        let key = format!("key-{i:04}");
        assert!(
            read_tx.get(key.as_bytes()).unwrap().is_some(),
            "missing key {key}"
        );
    }
    read_tx.commit();
}

fn parse_key_index(key: &[u8]) -> u32 {
    let key_str = std::str::from_utf8(key).expect("utf8 key");
    let digits = key_str
        .strip_prefix("key-")
        .expect("key should start with 'key-'");
    let numeric = &digits[..4];
    numeric.parse().expect("parse key digits")
}
