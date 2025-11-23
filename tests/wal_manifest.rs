use quickstep::{QuickStep, QuickStepConfig};
use std::{
    convert::TryInto,
    fs::File,
    io::{Read, Seek, SeekFrom},
    mem,
    path::Path,
};
use tempfile::TempDir;

const MANIFEST_LEN: usize = 32;

#[test]
fn wal_manifest_tracks_checkpoint_len_after_flush() {
    let temp = TempDir::new().expect("tempdir");
    let data_path = temp.path().join("data.qs");
    let wal_path = data_path.with_extension("wal");

    let (cp_len_before, _) = {
        let db = QuickStep::new(QuickStepConfig::new(&data_path, 32, 256, 14));
        {
            let mut tx = db.tx();
            tx.put(b"alpha", b"one").expect("insert alpha");
            tx.commit();
        }

        let (cp_before, file_before) = read_manifest(&wal_path);
        assert!(
            cp_before <= file_before,
            "checkpoint len should not exceed WAL length"
        );

        db.debug_flush_root_leaf()
            .expect("flush root leaf to force checkpoint");
        (cp_before, file_before)
    };

    let (cp_len_after, file_len_after) = read_manifest(&wal_path);
    assert!(
        cp_len_after <= file_len_after,
        "checkpoint len should never exceed WAL length"
    );
    assert!(
        cp_len_after >= cp_len_before,
        "checkpoint len should advance after flush"
    );
}

fn read_manifest(path: &Path) -> (u64, u64) {
    let mut file = File::open(path).expect("open wal file");
    let mut header = [0u8; MANIFEST_LEN];
    file.seek(SeekFrom::Start(0)).expect("seek manifest");
    file.read_exact(&mut header).expect("read manifest");
    assert_eq!(&header[0..4], b"WALM");
    let checkpoint_len = u64::from_le_bytes(header[8..16].try_into().unwrap());
    let file_len = file.metadata().expect("metadata").len();
    (checkpoint_len, file_len)
}

#[test]
fn wal_replay_discards_uncommitted_transactions() {
    let temp = TempDir::new().expect("tempdir");
    let db_path = temp.path().join("undo");

    {
        let db = QuickStep::new(QuickStepConfig::new(&db_path, 32, 256, 14));
        {
            let mut tx = db.tx();
            tx.put(b"stable", b"yes").expect("insert committed");
            tx.commit();
        }
        {
            let mut tx = db.tx();
            tx.put(b"inflight", b"temp").expect("insert pending");
            mem::forget(tx);
        }
    }

    let reopened = QuickStep::new(QuickStepConfig::new(&db_path, 32, 256, 14));
    let mut tx = reopened.tx();
    assert_eq!(tx.get(b"stable").unwrap(), Some(b"yes".as_ref()));
    assert!(
        tx.get(b"inflight").unwrap().is_none(),
        "pending transaction should be rolled back"
    );
    tx.commit();
}

