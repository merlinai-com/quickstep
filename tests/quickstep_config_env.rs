use quickstep::QuickStepConfig;
use std::env;
use std::path::PathBuf;

fn base_config() -> QuickStepConfig {
    QuickStepConfig::new(PathBuf::from("/tmp"), 32, 256, 14)
}

fn reset_env() {
    for key in [
        "QUICKSTEP_WAL_LEAF_THRESHOLD",
        "QUICKSTEP_WAL_GLOBAL_RECORD_THRESHOLD",
        "QUICKSTEP_WAL_GLOBAL_BYTE_THRESHOLD",
    ] {
        env::remove_var(key);
    }
}

#[test]
fn env_overrides_replace_defaults() {
    reset_env();
    env::set_var("QUICKSTEP_WAL_LEAF_THRESHOLD", "7");
    env::set_var("QUICKSTEP_WAL_GLOBAL_RECORD_THRESHOLD", "13");
    env::set_var("QUICKSTEP_WAL_GLOBAL_BYTE_THRESHOLD", "2048");

    let cfg = base_config().with_env_overrides();
    assert_eq!(cfg.wal_thresholds(), (7, 13, 2048));
    reset_env();
}

#[test]
fn invalid_env_values_are_ignored() {
    reset_env();
    env::set_var("QUICKSTEP_WAL_LEAF_THRESHOLD", "invalid");
    let cfg = base_config().with_env_overrides();
    assert_eq!(
        cfg.wal_thresholds(),
        (32, 1024, 512 * 1024),
        "defaults should remain when env values fail to parse"
    );
    reset_env();
}

#[test]
fn cli_overrides_accept_equals_syntax() {
    let cfg = base_config().with_cli_overrides([
        "--quickstep-wal-leaf-threshold=5",
        "--quickstep-wal-global-record-threshold=11",
        "--quickstep-wal-global-byte-threshold=4096",
    ]);
    assert_eq!(cfg.wal_thresholds(), (5, 11, 4096));
}

#[test]
fn cli_overrides_accept_space_syntax() {
    let cfg = base_config().with_cli_overrides([
        "--other-flag",
        "ignored",
        "--quickstep-wal-leaf-threshold",
        "9",
        "--quickstep-wal-global-record-threshold",
        "15",
        "--quickstep-wal-global-byte-threshold",
        "8192",
    ]);
    assert_eq!(cfg.wal_thresholds(), (9, 15, 8192));
}

#[test]
fn cli_overrides_ignore_invalid_values() {
    let cfg = base_config().with_cli_overrides([
        "--quickstep-wal-leaf-threshold=bad",
        "--quickstep-wal-global-record-threshold",
        "NaN",
        "--quickstep-wal-global-byte-threshold",
        "1024",
    ]);
    assert_eq!(
        cfg.wal_thresholds(),
        (32, 1024, 1024),
        "only valid overrides should apply"
    );
}
