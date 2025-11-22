use quickstep::{
    debug,
    map_table::PageId,
    wal::{WalEntryKind, WalManager},
    QuickStep, QuickStepConfig,
};
use tempfile::TempDir;

fn wal_record_count(db: &QuickStep, page_id: Option<PageId>) -> usize {
    db.debug_wal_stats(page_id).leaf_records.unwrap_or(0)
}

#[test]
fn wal_records_include_fence_bounds() {
    let temp = TempDir::new().expect("tempdir");
    let wal_path = temp.path().join("quickstep.wal");
    {
        let wal = WalManager::open(&wal_path).expect("open wal");
        wal.append_put(
            PageId::from_u64(7),
            b"key-0001",
            b"value",
            &[0x00],
            &[0x7F],
            WalEntryKind::Redo,
            1,
        )
        .expect("append put");
        wal.append_tombstone(
            PageId::from_u64(7),
            b"key-0002",
            &[0x10],
            &[0x80],
            WalEntryKind::Redo,
            1,
        )
        .expect("append tombstone");
    }

    let wal = WalManager::open(&wal_path).expect("reopen wal");
    let records = wal.records();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].lower_fence, vec![0x00]);
    assert_eq!(records[0].upper_fence, vec![0x7F]);
    assert_eq!(records[1].lower_fence, vec![0x10]);
    assert_eq!(records[1].upper_fence, vec![0x80]);
}

#[test]
fn wal_replay_survives_merge_crash() {
    let temp = TempDir::new().expect("tempdir");
    let db_path = temp.path().join("db");
    let pivot_key: Vec<u8>;
    {
        debug::reset_debug_counters();
        let cfg = QuickStepConfig::new(db_path.clone(), 32, 256, 14).with_wal_thresholds(
            usize::MAX,
            usize::MAX,
            usize::MAX,
        );
        let db = QuickStep::new(cfg);
        let payload = vec![0u8; 64];
        const TOTAL_KEYS: usize = 96;
        const KEPT_PREFIX: usize = 4;

        {
            let mut tx = db.tx();
            for idx in 0..TOTAL_KEYS {
                let key = format!("key-{idx:04}");
                tx.put(key.as_bytes(), &payload).expect("insert");
            }
            tx.commit();
        }

        let parent = db
            .debug_root_leaf_parent()
            .expect("root should have promoted");
        pivot_key = parent.pivots[0].clone();
        let pivot_idx = String::from_utf8(pivot_key.clone())
            .expect("pivot is valid utf8")
            .rsplit_once('-')
            .and_then(|(_, suffix)| suffix.parse::<usize>().ok())
            .expect("pivot suffix");
        assert!(
            KEPT_PREFIX < pivot_idx,
            "pivot should reference right-hand leaf"
        );

        {
            let mut tx = db.tx();
            for idx in 0..TOTAL_KEYS {
                let key = format!("key-{idx:04}");
                if idx >= KEPT_PREFIX {
                    tx.delete(key.as_bytes()).expect("delete");
                }
            }
            tx.commit();
        }

        if let Some(parent) = db.debug_root_leaf_parent() {
            if parent.children.len() == 2 {
                db.debug_merge_leaves(parent.children[0], parent.children[1])
                    .expect("manual merge");
            }
        }
        // drop db without flushing to force WAL replay on restart
    }

    let reopened = QuickStep::new(
        QuickStepConfig::new(db_path, 32, 256, 14).with_wal_thresholds(
            usize::MAX,
            usize::MAX,
            usize::MAX,
        ),
    );
    let snapshot = reopened
        .debug_leaf_snapshot(PageId::from_u64(0))
        .expect("root snapshot");
    assert!(
        snapshot
            .keys
            .iter()
            .all(|k| k.as_slice() < pivot_key.as_slice()),
        "right-side keys should not survive merge replay"
    );
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
        wal_record_count(&db, Some(PageId::from_u64(0))) < 16,
        "auto checkpoint should prune per-leaf WAL entries even with redo/undo logging"
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
            wal_record_count(&db, Some(PageId::from_u64(0))) < 8,
            "byte-based threshold should trigger global checkpoint even with redo/undo logging"
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
        wal_record_count(&db, Some(PageId::from_u64(0))) >= 96,
        "custom thresholds should delay automatic pruning, counting redo+undo entries"
    );
}

#[test]
fn wal_checkpoint_drops_only_target_page() {
    let temp = TempDir::new().expect("tempdir");
    let wal_path = temp.path().join("checkpoint.wal");
    let wal = WalManager::open(&wal_path).expect("open wal");

    let lower = &[0x00];
    let upper = &[0xFF];
    wal.append_put(
        PageId::from_u64(5),
        b"alpha",
        b"v1",
        lower,
        upper,
        WalEntryKind::Redo,
        1,
    )
    .expect("append put");
    wal.append_tombstone(
        PageId::from_u64(5),
        b"beta",
        lower,
        upper,
        WalEntryKind::Redo,
        1,
    )
    .expect("append tombstone");
    wal.append_put(
        PageId::from_u64(9),
        b"gamma",
        b"v2",
        lower,
        upper,
        WalEntryKind::Redo,
        2,
    )
    .expect("append put");

    let grouped = wal.records_grouped();
    assert_eq!(
        grouped.get(&5).map(|records| records.len()),
        Some(2),
        "page 5 should have two records before checkpoint"
    );
    assert_eq!(
        grouped.get(&9).map(|records| records.len()),
        Some(1),
        "page 9 should have one record before checkpoint"
    );

    wal.checkpoint_page(PageId::from_u64(5))
        .expect("checkpoint page 5");

    let grouped = wal.records_grouped();
    assert!(
        grouped.get(&5).is_none(),
        "page 5 entries should be removed after checkpoint"
    );
    assert_eq!(
        grouped.get(&9).map(|records| records.len()),
        Some(1),
        "page 9 entries should remain after checkpoint"
    );

    drop(wal);
    let reopened = WalManager::open(&wal_path).expect("reopen wal");
    let grouped = reopened.records_grouped();
    assert!(
        grouped.get(&5).is_none(),
        "page 5 entries should stay removed after reopen"
    );
    assert_eq!(
        grouped.get(&9).map(|records| records.len()),
        Some(1),
        "page 9 entries should persist after reopen"
    );
}
