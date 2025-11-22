use quickstep::{debug, QuickStep, QuickStepConfig};
use tempfile::TempDir;

fn new_small_cache_db() -> QuickStep {
    let temp = TempDir::new().expect("tempdir");
    let config = QuickStepConfig::new(temp.into_path(), 32, 256, 13);
    QuickStep::new(config)
}

#[test]
fn eviction_flushes_dirty_leaf_to_disk() {
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

    let mut read_tx = db.tx();
    for i in 0..256 {
        let key = format!("key-{i:04}");
        assert!(
            read_tx.get(key.as_bytes()).unwrap().is_some(),
            "key {i} should be readable after eviction"
        );
    }
    read_tx.commit();
}
