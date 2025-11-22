# Coding History

# Coding History

#### 2025-11-22 15:40 UTC [pending] [main]

- Added tombstone-aware delete support: `NodeMeta` can mark entries as tombstones, `flush_dirty_entries` removes them from disk on eviction, and `QuickStep::delete` now relies on tombstones plus auto-merge thresholds rather than immediate physical removal.
- Cascading merge logic now walks the entire parent chain so delete-triggered merges collapse inner nodes and demote the root when necessary.
- Tests: `cargo test quickstep_merge`.

#### 2025-11-22 15:05 UTC [pending] [main]

- Added `LeafMergePlan`/`LeafMergeOutcome` plus `debug::MergeEvent` instrumentation so survivor leaves can rebuild themselves while we log the reclaimed page IDs + merged counts.
- Extended `BPNode`/`BPTree` with `remove_child_after_merge` and `demote_root_after_merge`; `QuickStepTx` now has internal helpers that merge mini-page siblings and rewrite parent pivots, exposing `debug_truncate_leaf`/`debug_merge_leaves` for tests.
- New `tests/quickstep_merge.rs` simulates deletes by truncating leaves, then merges siblings to cover both root-demotion and “root stays inner, child count shrinks” paths.
- Tests: `cargo test quickstep_merge`.

#### 2025-11-22 14:45 UTC [pending] [main]

- Added eviction/liveness bitfields to `NodeMeta`, allowing mini-pages to be marked in-flight, flushed, and reclaimed. `PageWriteGuard` can now rewrite a map-table slot back to `NodeRef::Leaf`.
- Implemented FIFO eviction in `MiniPageBuffer`: dirty entries are flushed via the new `page_op::flush_dirty_entries` helper, the circular buffer’s head advances, and `debug::record_eviction` tracks activity. `QuickStepTx::new_mini_page` now retries allocations by invoking eviction.
- Added `tests/quickstep_eviction.rs::eviction_flushes_dirty_leaf_to_disk`, which constrains the cache, forces a split, asserts an eviction occurred, and re-reads every key afterward.
- Documentation (README, roadmap, detailed plan) now notes the baseline eviction flow.

#### 2025-11-22 14:30 UTC [pending] [main]

- `NodeMeta` gained `set_disk_addr`/`set_page_id_field` helpers plus `set_identity`, allowing us to clone leaf contents during splits without losing their unique `(PageId, disk_addr)` identity.
- `QuickStepTx::apply_leaf_split` now restores the right-hand leaf’s identity immediately after replaying entries, eliminating the “refresh later” hack and preventing future evictions from writing to the wrong disk page.
- `DebugLeafSnapshot` exposes `disk_addr`, and the split integration tests assert every child produced by root and cascading splits lands on a unique disk page; this verifies map-table propagation + NodeRef bookkeeping end-to-end.
- Documentation (roadmap + detailed plan) now marks the map-table propagation task complete, and README notes that the remaining Phase 1.3 work centers on merges/eviction.

#### 2025-11-22 14:15 UTC [pending] [main]

- Introduced `ChildPointer` + `LockedInner` so write-lock bundles retain level + node IDs; `BPNode` now has shared helpers for resetting/appending entries regardless of child type.
- Added `BPTree::split_inner_node`, `promote_inner_root`, and `QuickStepTx::bubble_split_up`, enabling cascading splits that allocate new inner siblings and promote the root when necessary.
- Exposed `QuickStep::debug_root_level` plus richer split events (`pivot_key`, `left_count`, `right_count`) to audit tree height changes.
- Added `tests/quickstep_split.rs::root_parent_splits_and_promotes_new_inner_level` to stress the tree until a level ≥2 root forms; reran `cargo test quickstep_split`.
- Documentation refresh: `design/detailed-plan.md` Parent/Testing sections explain the new plumbing, and README status bullets call out cascading split support.

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
- Instrumentation upgrade: `debug::split_events()` now records the pivot key plus `(left_count, right_count)` so tests can cross-check the recorded metadata against real leaf contents.
- Documented the new routing test + instrumentation in `design/detailed-plan.md`.
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
