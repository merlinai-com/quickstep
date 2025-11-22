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
   - **Option A – promote before inserting (implemented 21 Nov 2025)**:
     - `PageGuard::try_put` now only operates on mini-pages and returns `TryPutResult` (`Success`, `NeedsPromotion`, `NeedsSplit`).
     - `QuickStepTx::put` loops on `try_put`, promoting `NodeRef::Leaf` pages via `promote_leaf_to_mini_page` (copy disk leaf → allocate cache slot → update map-table entry) before retrying the insert.
     - Split handling still returns `TryPutResult::NeedsSplit`; actual split logic remains `todo!()` for Phase 1.3.

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

---

## 1.3 – Leaf splits (In progress)

1. **Pre-flight checks**
   - ✅ Added a debug assertion in `promote_leaf_to_mini_page` to guarantee every on-disk leaf we copy already contains at least the two required fence keys. This confirms the initial disk formatting assumptions before we start moving records around.
   - TODO: Add lightweight instrumentation (feature-gated) so tests can assert how often `TryPutResult::NeedsSplit` is triggered and which page IDs are involved.

2. **Implementation plan**
   1. Extend `NodeMeta` helpers:
      - ✅ Added record-count setters plus `inc_record_count` / `dec_record_count`.
      - ✅ Added `LeafEntryIter` iterator to yield `(KVMeta, key_suffix, value)` triples without duplicating pointer arithmetic.
      - 🔜 Add a `reset_contents()` + `replay_entries()` pair so the left-hand mini-page can be zeroed and repopulated via the existing insertion path (no bespoke serializer). This is the first actionable task before we touch the split flow again.
   2. Build/apply the split helper:
      - ✅ Implemented `LeafSplitPlan::from_node` (reads the existing mini-page, identifies pivot key, and prepares owned `(key, value)` pairs for the right-hand page).
      - 🔜 Implement `LeafSplitPlan::apply`:
        * Input: the plan above, a mutable reference to the original (left) mini-page, and a freshly allocated right-hand mini-page (`NodeMeta`).
        * Steps: wipe the left page, replay only the retained entries, populate the right page from the owned `(key, value)` pairs, and expose the separator key.
        * Result: Two valid leaves, plus a `LeafSplitOutcome` struct carrying the pivot key and child page IDs for the parent update.
   3. Parent/root updates:
      - 🔜 Teach `BPTree` a `promote_leaf_root` helper that installs a minimal inner node with two children when the current root splits.
      - 🔜 For non-root leaves, plumb a `ParentInsert` struct that captures the separator key + right child ID and hands it to the (future) inner-node insert path. Inner splits can stay `todo!()` if we clearly mark the missing recursive step.
      - 🔜 Update `MapTable` + `NodeRef` bookkeeping so the new right leaf becomes reachable immediately after the left leaf is rebuilt.

3. **Testing**
   - Extend `tests/quickstep_put_basic.rs` (or add `tests/quickstep_split.rs`) to insert enough keys to trigger a split, then:
     1. Assert that all keys are still readable.
     2. Assert that the parent/root now references two leaves (via a test-only debug hook exposing the current root structure).
     3. Confirm via instrumentation counters that exactly one split occurred.
   - Add negative test ensuring that after the split, inserting additional keys routes to the correct leaf.

4. **Open questions**
   - Need to confirm the expected initial fence-key layout for brand-new leaves (currently inferred via debug assertion). If the format differs from Raphael’s reference implementation, we may need to add an explicit “format_leaf” step during bootstrap.
   - Inner-node serialization helpers are not implemented yet (`BPNode` currently only supports searching). We will implement just enough (key insertion + child pointer storage) for the root case in this phase.

