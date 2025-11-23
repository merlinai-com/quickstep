use quickstep::{QuickStep, QuickStepConfig};
use tempfile::TempDir;

fn new_db() -> QuickStep {
    let temp = TempDir::new().expect("tempdir");
    let config = QuickStepConfig::new(temp.into_path(), 32, 256, 14);
    QuickStep::new(config)
}

#[test]
fn explicit_abort_rolls_back_changes() {
    let db = new_db();

    {
        let mut tx = db.tx();
        tx.put(b"alpha", b"one").expect("put alpha");
        tx.abort();
    }

    let mut verify = db.tx();
    assert!(verify.get(b"alpha").unwrap().is_none());
    verify.commit();
}

#[test]
fn drop_without_commit_auto_aborts() {
    let db = new_db();

    {
        let mut tx = db.tx();
        tx.put(b"beta", b"two").expect("put beta");
        // drop without commit
    }

    let mut verify = db.tx();
    assert!(verify.get(b"beta").unwrap().is_none());
    verify.commit();
}

