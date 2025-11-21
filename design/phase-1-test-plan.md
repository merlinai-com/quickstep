# Phase 1 Test Plan

These checks map directly to the Phase 1 roadmap items. Each task is **done** only when the corresponding tests pass under the current Rust toolchain.

## 1.1 – `QuickStep::new()` initialisation

1. Create a smoke test (`quickstep/tests/quickstep_new.rs` or similar) that:
   - Builds a temporary directory (e.g., via `tempfile` or `tempdir`).
   - Constructs `QuickStepConfig` with small bounds.
   - Calls `QuickStep::new()` and asserts the returned struct has:
     - Non-null `inner_nodes` root
     - `map_table` capacity matching `leaf_upper_bound`
     - Cache allocation succeeds for a tiny `NodeSize`
     - Data file exists on disk (check `config.path.join("quickstep.db")`)
2. Run `cargo test quickstep_new_smoke`.

## 1.2 – `put()` happy path (no splits)

1. Add an integration test that:
   - Calls `QuickStep::new()`.
   - Starts a transaction and inserts several key/value pairs that fit within a single mini-page.
   - Commits the transaction.
   - Starts a new transaction and asserts `get()` returns the expected values.
2. Optional property test: insert `n` random unique keys (all small) and assert round-trip equality.
3. Verify the test never hits `SplitNeeded` (e.g., by instrumenting a debug flag or asserting the page count).

## 1.3 – Split/Merge validation

1. Extend the integration tests to insert enough records to force a split:
   - After `put()`, confirm the tree now routes lookups to the new leaf.
   - Check that fence keys in the parent reflect the new page boundaries.
2. Add a deletion test that drives a merge:
   - Insert > split threshold records.
   - Delete until the mini-pages should merge.
   - Ensure lookups still succeed and no empty pages remain.
3. Consider a stress helper that alternates inserts/deletes and checks the tree structure via debug hooks.

## 1.4 – Fence-key handling in `get()`

1. Craft a targeted test where:
   - Keys sit exactly on the lower/upper fence boundaries.
   - `get()` uses both fence keys to determine the correct leaf without restarting.
2. Verify `lower_fence_key` / `upper_fence_key` tuples returned by `read_traverse_leaf` are correct (even if the function still returns `todo!()`, the new test should assert their values once implemented).

## 1.5 – Transactions (`abort` / `commit`)

1. `commit` test:
   - Begin tx → put → commit → new tx → get should succeed.
2. `abort` test:
   - Begin tx → put → abort → new tx → get should return `None`.
3. Concurrency sanity test:
   - Two transactions writing disjoint keys in parallel threads should both commit successfully.
4. Stress/regression test:
   - Loop over random operations (`put`, `get`, `abort`, `commit`) and verify final state against an in-memory reference map.

## Colour Key / Exit Criteria

- **Passing tests**: run `cargo test` (and `cargo check`) after each subtask.
- **Documentation**: update CHANGELOG + CODING_HISTORY when the corresponding tests are added/passed.
- **CI-ready**: once Phase 1 is complete, consider gating merges on `cargo test`.

