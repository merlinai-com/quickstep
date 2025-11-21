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
   - Root bootstrap, map-table entry, and IO address allocation completed in 1.1 (see above). Next challenge is safely promoting on-disk leaves into mini-pages when we hit `NodeRef::Leaf`.

2. **Implementation plan**  
   - **Chosen approach (Option A)**: handle promotion inside `QuickStepTx::put`.
     - `PageGuard::try_put` only deals with mini-pages. If it encounters a leaf, it returns a new outcome (`TryPutResult::Promote(PageId)`).
     - `QuickStepTx::put` detects `Promote`, performs promotion itself (alloc mini-page via `self.new_mini_page`, copy leaf contents, update map table), then retries the mini-page path.
     - Promotion steps:
       1. Acquire write access to the disk leaf (similar to `get()` path).
       2. Read existing KV pairs, insert them into the new mini-page using `NodeMeta::try_put_with_suffix`.
       3. Update the map table entry to point at the mini-page; release disk guard.
       4. Retry `try_put` on the new mini-page for the incoming key/value.
   - `NodeRef::MiniPage` branch:
     - Use `NodeMeta::try_put`; return `Ok` on success, `Err(SplitNeeded)` when out of space.
   - Split logic remains `todo!()` for Phase 1.3.

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

### Alternative approaches considered (rejected for now)

- **Option B – pass a promotion context into `PageGuard::try_put`**  
  *Pros*: keeps promotion logic local to the guard.  
  *Cons*: requires a complicated helper struct to juggle lifetimes and mutable borrows; still easy to violate Rust’s aliasing rules. Given this is foundation code, we prefer simplicity.

- **Option C – defer promotion (mutate only disk leaves)**  
  *Pros*: quick hack to unblock `put()`.  
  *Cons*: contradicts the Bf-tree design (mini-pages are the core feature) and doesn’t test the code we ultimately need. Not acceptable for a database storage engine we expect to rely on.

Thus Option A (promotion inside `QuickStepTx::put`) is the selected path.

