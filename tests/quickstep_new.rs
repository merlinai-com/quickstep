use quickstep::{QuickStep, QuickStepConfig};
use std::fs;
use tempfile::TempDir;

#[test]
fn quickstep_new_smoke() {
    let temp_dir = TempDir::new().expect("tempdir");

    let config = QuickStepConfig::new(temp_dir.path(), 32, 128, 12);
    let quickstep = QuickStep::new(config);

    // verify we can create a tx without panicking
    quickstep.tx();

    // ensure the backing file exists
    let expected_path = temp_dir.path().join("quickstep.db");
    assert!(
        fs::metadata(expected_path).is_ok(),
        "expected quickstep.db to be created"
    );
}
