use quickstep::{QuickStep, QuickStepConfig};
use tempfile::TempDir;

fn wal_record_count(db: &QuickStep, disk_addr: Option<u64>) -> usize {
    db.debug_wal_stats(disk_addr).leaf_records.unwrap_or(0)
}

#[test]
fn deletes_persist_after_flush_and_restart() {
    let temp = TempDir::new().expect("tempdir");
    let db_path = temp.path().join("db");
    let config = QuickStepConfig::new(db_path.clone(), 32, 256, 14);
    let db = QuickStep::new(config);

    let payload = vec![0u8; 16];
    {
        let mut tx = db.tx();
        for idx in 0..16 {
            let key = format!("key-{idx:04}");
            tx.put(key.as_bytes(), &payload).expect("insert");
        }
        tx.commit();
    }

    assert!(db.delete(b"key-0003").unwrap());
    assert!(db.delete(b"key-0007").unwrap());
    db.debug_flush_root_leaf().expect("flush root leaf");

    drop(db);

    let reopened = QuickStep::new(QuickStepConfig::new(db_path, 32, 256, 14));
    {
        let mut tx = reopened.tx();
        assert!(tx.get(b"key-0003").unwrap().is_none());
        assert!(tx.get(b"key-0007").unwrap().is_none());
        assert!(tx.get(b"key-0005").unwrap().is_some());
        tx.commit();
    }
}

#[test]
fn wal_replays_deletes_without_manual_flush() {
    let temp = TempDir::new().expect("tempdir");
    let db_path = temp.path().join("db");
    {
        let db = QuickStep::new(QuickStepConfig::new(db_path.clone(), 32, 256, 14));
        let payload = vec![0u8; 16];
        {
            let mut tx = db.tx();
            for idx in 0..24 {
                let key = format!("key-{idx:04}");
                tx.put(key.as_bytes(), &payload).expect("insert");
            }
            tx.commit();
        }
        assert!(db.delete(b"key-0004").unwrap());
        assert!(db.delete(b"key-0015").unwrap());
        // intentionally skip flush; WAL should capture deletes
    }

    let reopened = QuickStep::new(QuickStepConfig::new(db_path, 32, 256, 14));
    {
        let mut tx = reopened.tx();
        assert!(
            tx.get(b"key-0004").unwrap().is_none(),
            "delete should be replayed from WAL for key-0004"
        );
        assert!(
            tx.get(b"key-0015").unwrap().is_none(),
            "delete should be replayed from WAL for key-0015"
        );
        assert!(
            tx.get(b"key-0003").unwrap().is_some(),
            "neighbouring keys should remain"
        );
        tx.commit();
    }
}

#[test]
fn wal_replays_puts_without_manual_flush() {
    let temp = TempDir::new().expect("tempdir");
    let db_path = temp.path().join("db");
    {
        let db = QuickStep::new(QuickStepConfig::new(db_path.clone(), 32, 256, 14));
        let payload = vec![42u8; 64];
        let mut tx = db.tx();
        for idx in 0..20 {
            let key = format!("key-{idx:04}");
            tx.put(key.as_bytes(), &payload).expect("insert");
        }
        tx.commit();
        // drop db without flushing; inserts should survive via WAL replay
    }

    let reopened = QuickStep::new(QuickStepConfig::new(db_path, 32, 256, 14));
    let mut tx = reopened.tx();
    for idx in 0..20 {
        let key = format!("key-{idx:04}");
        assert!(
            tx.get(key.as_bytes()).unwrap().is_some(),
            "key {idx} should be replayed from WAL"
        );
    }
    tx.commit();
}

#[test]
fn wal_auto_checkpoint_trims_entries() {
    let temp = TempDir::new().expect("tempdir");
    let db_path = temp.path().join("db");
    let db = QuickStep::new(QuickStepConfig::new(db_path, 32, 256, 14));
    let payload = vec![5u8; 32];
    {
        let mut tx = db.tx();
        for idx in 0..48 {
            let key = format!("key-{idx:04}");
            tx.put(key.as_bytes(), &payload).expect("insert");
        }
        tx.commit();
    }
    assert!(
        wal_record_count(&db, Some(0)) < 8,
        "auto checkpoint should prune per-leaf WAL entries"
    );
}

#[test]
fn wal_byte_threshold_triggers_checkpoint() {
    let temp = TempDir::new().expect("tempdir");
    let db_path = temp.path().join("db");
    {
        let db = QuickStep::new(QuickStepConfig::new(db_path.clone(), 32, 256, 14));
        let payload = vec![0u8; 128 * 1024];
        let mut tx = db.tx();
        for idx in 0..8 {
            let key = format!("key-large-{idx:04}");
            tx.put(key.as_bytes(), &payload).expect("large insert");
        }
        tx.commit();
        assert!(
            wal_record_count(&db, Some(0)) < 4,
            "byte-based threshold should trigger global checkpoint"
        );
    }
}

#[test]
fn wal_respects_custom_thresholds() {
    let temp = TempDir::new().expect("tempdir");
    let db_path = temp.path().join("db");
    let config = QuickStepConfig::new(db_path.clone(), 32, 256, 14).with_wal_thresholds(
        128,
        10_000,
        usize::MAX,
    );
    let db = QuickStep::new(config);
    let payload = vec![3u8; 2048];
    {
        let mut tx = db.tx();
        for idx in 0..96 {
            let key = format!("key-custom-{idx:04}");
            tx.put(key.as_bytes(), &payload).expect("insert");
        }
        tx.commit();
    }
    assert!(
        wal_record_count(&db, Some(0)) >= 48,
        "custom thresholds should delay automatic pruning"
    );
}
