# Phase 1 Detailed Plan
21 Nov 2025

This document tracks the step-by-step work for each Phase 1 task. For now it covers items 1.1 and 1.2.

---

## 1.1 – `QuickStep::new()` initialisation (Completed)

1. **Review existing structures**  
   - Read `lib.rs`, `buffer.rs`, `map_table.rs`, and `io_engine.rs` to map out what the constructor must instantiate.

2. **Implementation steps**  
   - Add `QuickStepConfig::new` helper for ergonomic construction in tests.
   - Implement `MiniPageBuffer::new` with heap-backed storage and freelist initialisation.
   - Implement `IoEngine::open` (creates parent directories, opens/creates the data file).
   - Wire up `QuickStep::new` to instantiate `BPTree`, `MiniPageBuffer`, `MapTable`, and `IoEngine`.
   - Add helper `resolve_data_path` to normalise path vs. directory inputs.

3. **Testing**  
   - Create `tests/quickstep_new.rs` with `quickstep_new_smoke` using `tempfile::TempDir`.
   - Assertions: transaction creation succeeds; `quickstep.db` exists.
   - Command: `cargo fmt && cargo test quickstep_new_smoke` (Rust 1.91.1 via rustup).
   - Result: **PASS** (warnings remain due to unfinished code; noted separately).

4. **Documentation & logging**  
   - Update `CHANGELOG.md` + `CODING_HISTORY.md` with summary and test outcomes.
   - Expand `design/phase-1-tests.md` to include current test status.

---

## 1.2 – `put()` happy path (No splits)

1. **Review current state / prerequisites**  
   - `BPTree` root currently has no initialised leaf node; we need a bootstrap path (e.g., allocate the first leaf page during `QuickStep::new`).
   - `NodeRef::Leaf` → `NodeRef::MiniPage` promotion requires calling `QuickStepTx::new_mini_page`, but `PageGuard::try_put` doesn’t yet have access to the transaction context.
   - `IoEngine::get_new_addr` is `todo!()`; we need at least a monotonic counter so map-table entries have disk addresses even before persistence is finished.

2. **Implementation plan**  
   - Bootstrap root leaf:
     - Initialise the root to point at a “dummy” empty leaf so inserts have somewhere to land.
   - Complete the `NodeRef::Leaf` branch:
     - Allocate a mini-page via the cache.
     - Initialise it with fence keys + first key/value.
     - Update the map table so future lookups resolve to the mini-page.
   - Complete the `NodeRef::MiniPage` branch for the happy path:
     - Use `NodeMeta::try_put`; handle success without touching the split logic.
     - On `InsufficientSpace`, return `Err(SplitNeeded)` (still unimplemented).
   - Ensure `QuickStepTx::put` returns `Ok(())` when the mini-page path succeeds.

3. **Testing**  
   - Add `tests/quickstep_put_basic.rs` covering:
     - Insert 3–5 small key/value pairs in a transaction → commit → new transaction → verify `get` results.
     - Negative test for missing key returns `None`.
     - (Optional) instrumentation to assert we did not hit the split path.

4. **Execution**  
   - `cargo fmt && cargo test quickstep_put_basic`.
   - Record test results + interpretation in `design/phase-1-tests.md`, CHANGELOG, CODING_HISTORY.
   - Only commit after tests pass (Rule 10) and after “guc”.

---

