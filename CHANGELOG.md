# Changelog

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