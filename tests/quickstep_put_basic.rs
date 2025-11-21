use quickstep::{QuickStep, QuickStepConfig};
use tempfile::TempDir;

fn new_db() -> QuickStep {
    let temp = TempDir::new().expect("tempdir");
    let config = QuickStepConfig::new(temp.into_path(), 32, 128, 12);
    QuickStep::new(config)
}

#[test]
fn insert_and_read_back() {
    let db = new_db();

    {
        let mut tx = db.tx();
        tx.put(b"alpha", b"one").expect("put alpha");
        tx.put(b"beta", b"two").expect("put beta");
        tx.put(b"gamma", b"three").expect("put gamma");
        tx.commit();
    }

    {
        let mut tx = db.tx();
        assert_eq!(tx.get(b"alpha").unwrap(), Some(b"one".as_ref()));
        assert_eq!(tx.get(b"beta").unwrap(), Some(b"two".as_ref()));
        assert_eq!(tx.get(b"gamma").unwrap(), Some(b"three".as_ref()));
        assert_eq!(tx.get(b"delta").unwrap(), None);
        tx.commit();
    }
}
