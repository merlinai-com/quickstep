# Changelog

# Changelog

# Changelog

# Changelog

#### 2025-11-22 17:35 UTC [pending] [main]

##### Phase 1.3 sentinel fence audit

- Added `QuickStep::debug_leaf_fences` plus a helper to extract the lower/upper fence bytes from any leaf (mini-page or on-disk), exposing the disk address alongside the fence keys for debug assertions.
- Extended `map_table::PageId` with `from_u64` so tests can reference concrete page IDs (e.g., the root) without reaching into crate-private internals.
- `tests/quickstep_fence_keys.rs` now covers four scenarios: page 0 at bootstrap, the two children created by the first root split, manual leaf merges via `debug_merge_leaves`, and eviction-driven flushes in the tiny-cache configuration; executed via `cargo test quickstep_fence_keys`.
- Updated the detailed plan and README status table to note the fence instrumentation/coverage.

#### 2025-11-22 17:25 UTC [pending] [main]

##### Phase 1.3 WAL CLI overrides

- `QuickStepConfig::with_cli_overrides` now parses `--quickstep-wal-leaf-threshold`, `--quickstep-wal-global-record-threshold`, and `--quickstep-wal-global-byte-threshold` flags (either `--flag=VALUE` or `--flag VALUE`) so deployments can tune checkpoint policy via command-line args in addition to env vars and builder overrides.
- `QuickStep::new` automatically applies both env and CLI overrides before wiring up the tree/cache, ensuring every call path honours runtime configuration without extra boilerplate.
- Extended `tests/quickstep_config_env.rs` with CLI coverage for equals/space syntax plus invalid input fallbacks; executed via `cargo test quickstep_config_env`.
- Documentation (detailed plan, README) updated to describe the new CLI flags and how they interact with the existing configuration helpers.

#### 2025-11-22 17:15 UTC [pending] [main]

##### Phase 1.3 WAL threshold env overrides

- `QuickStepConfig::with_env_overrides` now honors `QUICKSTEP_WAL_LEAF_THRESHOLD`, `QUICKSTEP_WAL_GLOBAL_RECORD_THRESHOLD`, and `QUICKSTEP_WAL_GLOBAL_BYTE_THRESHOLD`, letting operators tune checkpoint policy without code changes.
- `QuickStep::new` automatically applies the overrides before instantiating the cache/tree, so every caller (tests included) picks up deployment-specific thresholds transparently.
- Added `tests/quickstep_config_env.rs` to cover both valid overrides and invalid inputs that fall back to defaults; ran `cargo test quickstep_config_env`.
- Documentation (`design/detailed-plan.md`, README) now lists the new env vars, and this changelog plus coding history include the testing notes.

#### 2025-11-22 17:05 UTC [pending] [main]

##### Phase 1.3 WAL puts + crash tests

- Extended `WalManager` to encode both put (`{page_id, disk_addr, key, value}`) and tombstone records, track per-leaf counts/bytes, and expose a global “noisiest leaf” candidate so we can automatically trim the WAL when it grows too large.
- `QuickStepTx::put`/`delete` now log WAL entries, run per-leaf checkpoints once a leaf crosses `WAL_LEAF_CHECKPOINT_THRESHOLD`, and invoke the global checkpoint hook when the overall WAL exceeds either the record or byte threshold.
- Added `tests/quickstep_delete_persist.rs::wal_replays_puts_without_manual_flush` plus `wal_auto_checkpoint_trims_entries`; executed via `cargo test quickstep_delete_persist` and `cargo test quickstep_merge`.
- Docs: README, detailed plan, changelog, and coding history now describe the broader WAL coverage, background checkpointing, and new crash tests.
- Configuration/observability: `QuickStepConfig::with_wal_thresholds(...)` lets deployments tune the per-leaf/global record + byte thresholds without code changes, `QuickStep::debug_wal_stats` exposes WAL usage for tests/monitoring, and a lightweight background monitor thread now requests checkpoints when global thresholds are exceeded.

#### 2025-11-22 16:25 UTC [pending] [main]

##### Phase 1.3 minimal WAL + delete persistence tests

- Introduced `WalManager` (`src/wal.rs`) plus `.wal` sidecar files: every delete now appends `{page_id, disk_addr, key}` records with fsync-before-return semantics, and eviction/explicit flush checkpoints prune per-leaf log entries.
- `QuickStep::replay_wal()` runs during start-up to reapply queued tombstones to on-disk leaves before the cache/map-table bootstrap completes, then truncates the WAL to zero once replay succeeds.
- `QuickStepTx::delete` logs tombstones before auto-merge evaluation; `MiniPageBuffer::evict` + `QuickStep::debug_flush_root_leaf` checkpoint the WAL once dirty leaves hit disk, ensuring redo-free restarts.
- Tests: `tests/quickstep_delete_persist.rs` now includes `wal_replays_deletes_without_manual_flush`, which deletes keys without flushing, drops the DB handle, and validates that reopening replays the WAL; re-ran `cargo test quickstep_delete_persist` and `cargo test quickstep_merge`.
- Docs: `design/detailed-plan.md` tombstone/WAL section, README status table, and Coding History all describe the minimal WAL flow, checkpoint ordering, and new test coverage.

#### 2025-11-22 15:40 UTC [pending] [main]

##### Phase 1.3 tombstone deletes + cascading merge fixes

- Added tombstone-aware delete path: `QuickStep::delete` marks entries as tombstones, `NodeMeta` exposes `mark_tombstone`/`remove_entry_at`, and flush logic now removes tombstoned keys from disk before compacting the in-memory leaf.
- Parent merges now cascade through every ancestor—`remove_parent_after_merge` walks the lock bundle chain and demotes the root when only one child remains—while auto-merge continues to fire below the occupancy threshold.
- Tests: extended `tests/quickstep_merge.rs` with delete-driven and multi-level cascading cases; re-ran `cargo test quickstep_merge`.
- Docs (README, detailed plan, roadmap, changelog, coding history) note that deletes exist but WAL/range semantics remain future work.

#### 2025-11-22 15:05 UTC [pending] [main]

##### Phase 1.3 leaf merge helpers + tests

- Added `LeafMergePlan`/`LeafMergeOutcome` to rebuild survivor leaves, reset reclaimed siblings, and return merge stats; `debug::MergeEvent`/`merge_events()` expose the instrumentation.
- Extended `BPNode`/`BPTree` with `remove_child_after_merge` and root demotion logic so parent pivots get removed and singleton roots collapse to the surviving child.
- `QuickStep::debug_truncate_leaf` and `QuickStep::debug_merge_leaves` let tests simulate delete-driven underflow; `tests/quickstep_merge.rs` covers both root demotion and “root stays inner” cases.
- Tests: `cargo test quickstep_merge`.

#### 2025-11-22 14:45 UTC [pending] [main]

##### Phase 1.3 cache eviction + write-back

- Added eviction/liveness bitfields to `NodeMeta`, preventing racing evictions and letting us reclaim mini-page slots deterministically.
- Introduced `page_op::flush_dirty_entries` and taught `MiniPageBuffer::evict` to flush dirty records, flip the map-table entry back to a disk leaf via `PageWriteGuard::set_leaf`, advance the circular-buffer head, and emit a new `debug::evictions()` counter.
- `QuickStepTx::new_mini_page` now retries failed allocations by invoking eviction, so root/leaf splits proceed even when the cache only holds a single 4 KiB slot.
- Tests: new `tests/quickstep_eviction.rs::eviction_flushes_dirty_leaf_to_disk` constrains the cache, forces a split, asserts an eviction occurred, and re-reads every key afterward.
- Docs: detailed plan + roadmap + README updated to capture the baseline eviction flow; changelog/coding history include the new test and instrumentation.

#### 2025-11-22 14:30 UTC [pending] [main]

##### Phase 1.3 map-table identity wiring

- Fixed `QuickStepTx::apply_leaf_split` so the freshly allocated right leaf keeps its unique `PageId` + disk address: copy-before-rebuild no longer clobbers the header thanks to the new `NodeMeta::set_identity` helper.
- `DebugLeafSnapshot` now surfaces each leaf’s disk address; split tests assert that every child produced during root and cascading splits persists to a distinct page immediately.
- Strengthened `tests/quickstep_split.rs` (`root_split_occurs_and_is_readable`, `post_split_inserts_route_to_expected_children`, `second_split_under_root_adds_third_child`) with disk-address checks; map-table wiring is now validated alongside key distribution.
- Docs: roadmap + detailed plan updated to mark map-table propagation complete, and README reflects that remaining work focuses on merge/eviction plumbing.

#### 2025-11-22 14:15 UTC [pending] [main]

##### Phase 1.3 cascading parent splits + root promotion test

- Added `ChildPointer`, `LockedInner`, and node-ID tracking so each ancestor lock knows its tree level; `BPNode::insert_entry_after_child` now works for both leaf and inner children.
- Implemented `BPTree::split_inner_node`, `promote_inner_root`, and `QuickStepTx::bubble_split_up`, enabling cascading splits to rebuild ancestors and promote a taller root.
- Recorded split metadata with pivot key plus `(left_count, right_count)` in `debug::SplitEvent`, and added `QuickStep::debug_root_level` to expose current tree height.
- Extended `tests/quickstep_split.rs` with `root_parent_splits_and_promotes_new_inner_level`, ensuring multi-level trees form correctly; re-ran `cargo test quickstep_split`.
- Documentation: `design/detailed-plan.md` Parent/Testing sections now describe cascading splits, and the README status table reflects the new capabilities.

#### 2025-11-22 13:05 UTC [pending] [main]

##### Phase 1.3 split instrumentation + depth-1 cascading test

- Added `debug::SplitEvent` logging so every successful leaf split records the logical left/right page IDs; `debug::split_events()` now complements the counter and lets tests assert which leaf actually split.
- `QuickStepTx::put` logs split events after applying the split plan, ensuring instrumentation captures the final page IDs even when parent updates follow.
- Extended `tests/quickstep_split.rs`: padded keys to preserve lexical ordering, asserted the first split always touches page 0, and introduced `second_split_under_root_adds_third_child` to confirm the root parent rebuilds itself with three children after a second split under the same inner node.
- Docs: `design/detailed-plan.md` now marks the instrumentation + second split scenario as complete, `design/roadmap.md` highlights the new coverage, and the README status table mentions the instrumentation-backed tests.
- Tests: `cargo test quickstep_split`.

#### 2025-11-22 13:40 UTC [pending] [main]

##### Phase 1.3 leaf snapshots + pivot-range assertions

- Added `QuickStep::debug_leaf_snapshot`/`DebugLeafSnapshot` so tests can materialise the exact user keys resident in any leaf (either cached or still on disk) via the existing map-table locks.
- Strengthened `tests/quickstep_split.rs`: each split event is matched to the root’s current child list, and the new snapshots assert that every pivot cleanly partitions the key ranges (left < pivot ≤ right) after the first and second splits.
- Updated `design/detailed-plan.md` Testing/Parent sections to call out the new helper + data validation step.
- Tests: `cargo test quickstep_split`.

#### 2025-11-22 13:55 UTC [pending] [main]

##### Phase 1.3 post-split routing test

- Added `tests/quickstep_split.rs::post_split_inserts_route_to_expected_children`, which inserts fresh keys on either side of the recorded pivot after the first split and proves (via `debug_leaf_snapshot`) that they land in the expected child without triggering another split.
- Instrumentation: `debug::SplitEvent` now carries the pivot key plus `(left_count, right_count)` so tests can cross-check the recorded metadata against actual leaf contents at split time.
- Documented the new negative-routing coverage + richer instrumentation in `design/detailed-plan.md`.
- Tests: `cargo test quickstep_split`.

#### 2025-11-22 10:45 UTC [pending] [main]

##### Phase 1.3 bootstrap hardening + node fixes

- `QuickStep::new` now formats page 0 on disk (header + sentinel fence keys) before inserting the root into the map table, ensuring every promotion sees a well-defined leaf image.
- `QuickStepTx::promote_leaf_to_mini_page` is copy-only: we allocate a cache slot, memcpy the on-disk leaf, and assert the fence invariants instead of patching headers on the fly.
- `NodeMeta::try_put` increments the record count when adding user entries, its metadata shift logic no longer overruns the array, and `binary_search` now excludes the two fence slots. Added `node::tests::node_try_put_roundtrip` to cover the path.
- Documentation updates: roadmap includes a progress column, and `design/detailed-plan.md` captures the disk-format decision.
- Tests: `cargo test node::tests::node_try_put_roundtrip` and `cargo test insert_and_read_back` (PASS, known warnings remain).

#### 2025-11-21 22:54 UTC [pending] [main]

##### Phase 1.2 Option A – promote leaves into mini-pages

- Refactored `PageGuard::try_put` to emit a `TryPutResult` (`Success`, `NeedsPromotion`, `NeedsSplit`) and limited it to mini-pages; `QuickStepTx::put` now loops via `try_put_with_promotion`, promoting on-disk leaves with `PageWriteGuard::set_mini_page` before retrying the insert.
- Added `LockManager`/`MapTable` glue so promotions copy disk leaves into the cache, update the map-table entry in place, and keep the existing page ID; documented the new flow in `design/detailed-plan.md` and `design/phase-1-tests.md`.
- Tests: `cargo fmt && cargo test quickstep_new_smoke` and `cargo test quickstep_put_basic` (both PASS, legacy warnings remain due to unfinished modules).

#### 2025-11-21 22:40 UTC [pending] [main]

##### Phase 1.2 happy-path put test (disk leaf mutation)

- `PageGuard::try_put` now mutates the on-disk leaf directly when map entries still reference `NodeRef::Leaf`; mini-page promotion remains TODO for a later phase.
- Added `tests/quickstep_put_basic.rs::insert_and_read_back`, executed via `cargo fmt && cargo test quickstep_new_smoke` (same command also runs the new test). Compiler warnings remain unchanged.
- Updated `design/detailed-plan.md` and `design/phase-1-tests.md` to document the interim approach and current test results.

#### 2025-11-21 22:15 UTC [pending] [main]

##### WIP phase 1.2 promotion sketch (tests failing)

- Documented promotion options in design/detailed-plan.md and began implementing Option A (promotion handled in QuickStepTx::put).
- Added preliminary quickstep_put_basic integration test; build currently fails due to borrow-checker issues (see cargo test output).

#### 2025-11-21 21:03 UTC [pending] [main]

##### add QuickStep::new smoke tests

- Added `tests/quickstep_new.rs` verifying:

  * `QuickStep::new` succeeds with a temporary directory config
  * Transactions can be created immediately after initialisation
  * The expected `quickstep.db` backing file is created on disk

- Supporting changes:

  * `QuickStepConfig::new` constructor for easier config creation in tests
  * `tempfile` dev-dependency for temporary directories

#### 2025-11-21 18:41 UTC [pending] [main]

##### initialise QuickStep::new and support code

- Core initialisation path:

  * Added `MiniPageBuffer::new` with managed backing storage
  * Introduced `IoEngine::open` for safe file creation
  * Wired up `QuickStep::new` to create the buffer, map table, tree, and IO engine

- Housekeeping:

  * Ignored `quickstep.code-workspace` and removed stray notebook metadata from this changelog

#### 2025-11-21 18:20 UTC [pending] [main]

##### roadmap tasks renumbered + documentation updates

- Roadmap legal-style numbering:

  * Renumbered every phase/task entry to `phase.task`
  * Documented HelixDB testing/integration phases

- Repository documentation touch-ups:

  * README now notes the legal-style numbering scheme
  * Added changelog & coding history scaffolding for future guc runs