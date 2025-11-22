use quickstep::{debug, QuickStep, QuickStepConfig};
use tempfile::TempDir;

fn new_db() -> QuickStep {
    let temp = TempDir::new().expect("tempdir");
    // keep the same parameters as other integration tests
    let config = QuickStepConfig::new(temp.into_path(), 32, 256, 12);
    QuickStep::new(config)
}

#[test]
fn root_split_occurs_and_is_readable() {
    debug::reset_debug_counters();
    let db = new_db();

    let mut tx = db.tx();
    // Large payload so that only a handful of inserts fill the mini-page.
    let payload = vec![0u8; 3072];
    let mut inserted = 0usize;
    while debug::split_requests() == 0 {
        assert!(
            inserted < 32,
            "expected a root split after a handful of large inserts"
        );
        let key = format!("key-{inserted:02}");
        tx.put(key.as_bytes(), &payload).expect("insert");
        inserted += 1;
    }
    tx.commit();

    assert_eq!(
        debug::split_requests(),
        1,
        "expected exactly one split while filling the root"
    );

    let mut read_tx = db.tx();
    for i in 0..inserted {
        let key = format!("key-{i:02}");
        assert!(
            read_tx.get(key.as_bytes()).unwrap().is_some(),
            "missing key {key}"
        );
    }
    read_tx.commit();
}
