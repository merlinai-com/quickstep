# Coding History

# Coding History

#### 2025-11-22 13:05 UTC [pending] [main]

- Added split instrumentation in `src/debug.rs` (`SplitEvent` log + `debug::split_events()`) and rewired `QuickStepTx::put` to record the logical left/right page IDs whenever a leaf split completes.
- Strengthened `tests/quickstep_split.rs`: padded keys for lexicographic ordering, asserted the first split touched page 0, and introduced `second_split_under_root_adds_third_child` to prove parent insertion rebuilds the root with three children once its right child splits again.
- Documentation updates: `design/detailed-plan.md` now marks instrumentation + both split tests complete, `design/roadmap.md` notes that root-level splits are covered by instrumentation-backed tests, and `README.md` calls out the new coverage in the status table.
- Tests: `cargo test quickstep_split` (PASS, existing warnings remain in unfinished modules).

#### 2025-11-22 13:40 UTC [pending] [main]

- Added `QuickStep::debug_leaf_snapshot` + `DebugLeafSnapshot` so integration tests can materialise the user keys for any leaf (mini-page or on-disk) under the current lock manager; this closes the loop between structural split checks and actual key ranges.
- Tightened `tests/quickstep_split.rs`: split events are now matched to the root’s child list, and the new leaf snapshots assert that every pivot cleanly partitions the key space (left < pivot ≤ right, etc.) after the first and second splits.
- Updated `design/detailed-plan.md` (Phase 1.3 Testing/Parent bullets) to capture the new helper + stronger assertions.
- Tests: `cargo test quickstep_split`.

#### 2025-11-22 13:55 UTC [pending] [main]

- Added `tests/quickstep_split.rs::post_split_inserts_route_to_expected_children`, which inserts new keys on both sides of the recorded pivot after the first split and proves (via `debug_leaf_snapshot`) that they land in the expected leaf without triggering extra splits.
- Documented the new routing test in `design/detailed-plan.md`.
- Tests: `cargo test quickstep_split`.

#### 2025-11-22 10:45 UTC [pending] [main]

- Formatted the on-disk root leaf during `QuickStep::new()` so page 0 always contains the sentinel fence keys before any transaction runs; promotion now copies the disk image verbatim into a mini-page and simply re-points the map-table entry.
- Tightened `NodeMeta::try_put` to bump record counts when inserting new user entries, fixed the metadata shift logic, and reworked `binary_search` to exclude fence keys—plus added a unit test (`node::tests::node_try_put_roundtrip`) to lock in the behaviour.
- Updated `design/roadmap.md` with a progress column per phase and noted the resolved bootstrap decisions in `design/detailed-plan.md`.
- Tests: `cargo test node::tests::node_try_put_roundtrip` and `cargo test insert_and_read_back` (PASS, legacy warnings remain in unfinished modules).

#### 2025-11-21 22:54 UTC [pending] [main]

- Implemented Option A for Phase 1.2: `PageGuard::try_put` now returns a `TryPutResult`, `QuickStepTx::put` loops via `try_put_with_promotion`, and `promote_leaf_to_mini_page` copies disk leaves into the cache while re-pointing the existing map-table entry with `PageWriteGuard::set_mini_page`.
- Updated `design/detailed-plan.md` and `design/phase-1-tests.md` to describe the promotion flow and to note that the happy-path test now exercises mini-pages rather than mutating disk leaves directly.
- Tests: `cargo fmt && cargo test quickstep_new_smoke` plus `cargo test quickstep_put_basic`; both succeed with the known warnings from unfinished modules.

#### 2025-11-21 22:40 UTC [pending] [main]

- Reworked `PageGuard::try_put` to mutate on-disk leaves directly (no mini-page promotion yet) and added `tests/quickstep_put_basic.rs`.
- Ran `cargo fmt && cargo test quickstep_new_smoke` (which also executes the new put test); build succeeds with the existing warnings.
- Updated `design/detailed-plan.md` and `design/phase-1-tests.md` to reflect the interim strategy and recorded the passing test results.

#### 2025-11-21 22:15 UTC [pending] [main]

- Captured the promotion options for Phase 1.2 and attempted Option A; build currently fails due to borrow-checker issues (see cargo output). Future commits supersede this attempt.

#### 2025-11-21 21:03 UTC [pending] [main]

#### 2025-11-21 18:41 UTC [pending] [main]

- Implemented `MiniPageBuffer::new` with owned backing storage and initialised freelists/head/tail pointers.
- Added `IoEngine::open` helper to create the data file safely (ensuring parent directories exist).
- Wired up `QuickStep::new` to initialise the B+ tree, map table, cache, and IO engine, plus helper for resolving data path.
- Ignored the local VS Code workspace file so it doesn’t pollute `git status`.

#### 2025-11-21 18:20 UTC [pending] [main]

- Adopted legal-style numbering across the entire roadmap to keep dependencies obvious.
- Recorded the change in README, CHANGELOG, and CODING_HISTORY to comply with `guc`.
- Noted future testing and HelixDB integration phases for upcoming implementation work.
