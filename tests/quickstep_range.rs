use quickstep::{QuickStep, QuickStepConfig};
use tempfile::TempDir;

fn new_db() -> QuickStep {
    let temp = TempDir::new().expect("tempdir");
    QuickStep::new(QuickStepConfig::new(temp.into_path(), 32, 256, 14))
}

#[test]
fn range_scan_single_leaf() {
    let db = new_db();
    {
        let mut tx = db.tx();
        tx.put(b"alpha", b"one").expect("insert alpha");
        tx.put(b"beta", b"two").expect("insert beta");
        tx.put(b"delta", b"four").expect("insert delta");
        tx.commit();
    }

    let range = db.range_scan(b"alpha", b"delta").expect("range scan");
    assert_eq!(
        range,
        vec![
            (b"alpha".to_vec(), b"one".to_vec()),
            (b"beta".to_vec(), b"two".to_vec())
        ]
    );
}

#[test]
fn range_scan_across_split_leaves() {
    let db = new_db();
    let payload = vec![0u8; 1024];
    {
        let mut tx = db.tx();
        for i in 0..200 {
            let key = format!("key-{i:04}");
            tx.put(key.as_bytes(), &payload).expect("insert");
        }
        tx.commit();
    }

    let results = db
        .range_scan(b"key-0050", b"key-0100")
        .expect("range scan");
    assert_eq!(results.len(), 50);
    assert_eq!(results.first().unwrap().0, b"key-0050");
    assert_eq!(results.last().unwrap().0, b"key-0099");
}

