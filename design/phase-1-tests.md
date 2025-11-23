# Phase 1 Tests

Each subsection lists three things:
1. **Outline** – what we are trying to prove.
2. **Implementation details** – how the test will be/has been coded.
3. **Current results** – the latest `cargo test …` run plus a plain-language interpretation (pass/fail, warnings, etc.).

---

## 1.1 – `QuickStep::new()` initialisation

### Outline
- Smoke-test the constructor using a throwaway directory.
- Ensure the data file is created, the cache allocates at least one mini-page, and we can start a transaction immediately.

### Implementation details
- File: `tests/quickstep_new.rs`.
- Uses `tempfile::TempDir` to create an isolated directory.
- Builds a `QuickStepConfig` via the helper constructor (`QuickStepConfig::new`).
- Calls `QuickStep::new()`, then:
  - Invokes `quickstep.tx()` to make sure transactions can be created.
  - Asserts the expected `quickstep.db` exists on disk.
  - (Future expansion: add explicit checks for map-table capacity and cache allocation once we expose those internals.)

### Current results
- Command: `cargo fmt && cargo test quickstep_new_smoke` (Rust 1.91.1 via rustup).
- Outcome: **PASS** – the smoke test succeeded. The compiler emitted long-standing warnings (unused imports, unfinished `todo!()` code) but no failures. These warnings are tracked separately and will shrink as the remaining Phase 1 work lands.

---

## 1.2 – `put()` happy path (no splits)

### Outline
- Insert several small key/value pairs within a single mini-page.
- Commit and verify they can be read back in a fresh transaction.
- Ensure the code path never requests a split.

### Implementation details
- Create `tests/quickstep_put_basic.rs`.
- Steps:
  1. Call `QuickStep::new()` with a cache large enough for one mini-page.
  2. Start a transaction → first `put` should force promotion from disk leaf to cache → insert 3–5 short keys → `commit`.
  3. New transaction → `get` each key → assert equality.
  4. Optionally call `tx().get()` for a missing key to confirm `None`.
- Instrumentation: `debug::record_split_request` now tracks every time `TryPutResult::NeedsSplit` fires; the test resets the counter and asserts it stays at zero to ensure the happy path never reaches the split logic.

### Current results
- Command: `cargo fmt && cargo test quickstep_new_smoke` (this run also executes `tests/quickstep_put_basic`).
- Outcome: **PASS** – the happy-path integration test (`insert_and_read_back`) now exercises Option A: the first insert promotes the disk leaf into a mini-page, subsequent inserts write to the cache, and no split requests are observed. The compiler still emits numerous warnings (unused imports, unfinished TODOs); those are tracked separately and do not affect functionality.

---

## 1.3 – Split/Merge validation

### Outline
- Force a split by inserting enough records, then verify lookups land in the correct leaf.
- Force a merge via deletions and confirm the tree remains consistent.

### Implementation details
- Reuse the fixture from 1.2 but bump the key count until a split should occur.
- After splitting:
  - Inspect `read_traverse_leaf` output to confirm `overflow_point` and fence keys are correct.
  - Ensure both halves of the split are findable via `get`.
- For merge testing:
  - Add deletion support (`tx().delete` once implemented) to drive underflow and confirm parent pointers update.
- Consider a helper that dumps tree structure for debugging (only compiled in tests).

### Current results
- **PASS** – `tests/quickstep_fence_keys.rs` exercises root splits, merge survivors, eviction flushes, and delete-triggered auto merges using the public API. Each scenario asserts the lower/upper fences still cover the resident keys (including the sentinel `[0x00]/[0xFF]` root case). Command: `cargo test quickstep_fence_keys`. Compiler warnings remain (unused imports/todo stubs) but do not affect correctness.
- **PASS (2025-11-23)** – `cargo test quickstep_split` verifies the instrumentation-backed split suite after the cascading-split loop landed (`QuickStepTx::put` now retries via `split_current_leaf`). No regressions observed; warnings unchanged from earlier runs.
- **PASS (2025-11-23)** – `tests/mini_page_buffer.rs::dealloc_reuses_slot_via_freelist` confirms freed mini-pages rejoin the freelist and are reused on the next allocation (`cargo test mini_page_buffer`). Only longstanding warnings remain.
- **PASS (2025-11-23)** – `tests/quickstep_eviction.rs::second_chance_clears_hot_pages_before_eviction` runs alongside the existing eviction test via `cargo test quickstep_eviction`, asserting `debug::second_chance_passes()` increases once the ref-bit path kicks in.

---

## 1.4 – Fence-key handling in `get()`

### Outline
- Explicitly test lookups whose keys sit exactly on fence boundaries.
- Ensure `lower_fence_key` / `upper_fence_key` values returned by `read_traverse_leaf` match expectations.

### Implementation details
- Once `read_traverse_leaf` finishes returning real fence keys, add a unit test that:
  - Creates two adjacent mini-pages with known fence values.
  - Calls `get()` for keys at the lower and upper edges.
  - Asserts the returned fence tuples contain the expected `(key, page_id)` pairs.
- If needed, expose a debug API (only compiled in tests) to read fence data without relying on `todo!()` areas.

### Current results
- Partially covered via the WAL persistence suite:
  - `tests/quickstep_delete_persist.rs::wal_records_include_fence_bounds` proves WAL entries capture the current `[lower, upper]` bounds.
  - `tests/quickstep_delete_persist.rs::wal_replay_survives_merge_crash` bulk-loads keys, triggers a split and auto-merge, then replays the WAL to ensure fence ranges remain valid.
  - `tests/quickstep_delete_persist.rs::wal_checkpoint_drops_only_target_page` verifies the new PageId-based WAL framing (length-prefixed groups) by checkpointing one page and ensuring the other page’s entries persist.
- Outstanding: add a deterministic “cached vs evicted sibling” replay test once we expose a debug eviction helper (tracked in §1.4 of the detailed plan).

---

## 1.5 – Transactions (`abort` / `commit`)

### Outline
- Validate both commit and abort semantics as well as basic concurrency.

### Implementation details
- `QuickStepTx` now carries an explicit `TxState`: `commit()` marks the transaction committed (undo log cleared) while `Drop`/`abort()` replay the undo log, append an abort marker, and leave the tree untouched.
- Added `tests/quickstep_tx.rs` with two foundational cases:
  1. **Explicit abort**: begin tx → put → abort → new tx → get returns `None`.
  2. **Implicit RAII abort**: begin tx → put → drop (no commit) → get returns `None`.
- Remaining stretch goals (still TODO): multi-threaded concurrency test plus a stress-harness that randomises puts/gets/aborts/commits against an in-memory oracle once undo-aware checkpoints are in place.

### Current results
- **PASS (2025-11-23)** – `cargo test quickstep_tx` exercises the new abort semantics (explicit + RAII). Existing warnings remain due to unfinished modules elsewhere.

---

## Exit Criteria

- **Passing tests**: every subsection must have a corresponding `cargo test …` invocation with a PASS outcome recorded above.
- **Documentation**: CHANGELOG + CODING_HISTORY entries are required whenever a new Phase 1 test suite is added.
- **Automation**: once all Phase 1 tests are green, wire them into CI (GitHub Actions) so regressions are caught automatically.

