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
   - ✅ Added lightweight split instrumentation (`debug::split_events`) that records both the original page ID and the freshly allocated sibling each time `TryPutResult::NeedsSplit` is resolved; integration tests can now assert exact split locations.
   - ✅ Split events now capture the pivot key plus the `(left_count, right_count)` tuple for each split, so tests can cross-check recorded pivots/occupancies without re-reading the tree.

2. **Implementation plan**
   1. Extend `NodeMeta` helpers:
      - ✅ Added record-count setters plus `inc_record_count` / `dec_record_count`, and taught `try_put` to bump the count whenever a new user entry is materialised.
      - ✅ Reworked `NodeMeta::binary_search` to operate on inclusive user-entry bounds (fences excluded), so cache-only lookups succeed immediately after promotion.
      - ✅ Added `LeafEntryIter` iterator to yield `(KVMeta, key_suffix, value)` triples without duplicating pointer arithmetic.
      - ✅ Added `NodeMeta::reset_user_entries` (keeps the two fence keys, drops everything else) plus `NodeMeta::replay_entries` to reinsert owned `(key, value)` pairs through the existing `try_put` path. Leaves can now be rebuilt without bespoke serializers.
   2. Build/apply the split helper:
      - ✅ Implemented `LeafSplitPlan::from_node` (reads the existing mini-page, identifies pivot key, and prepares owned `(key, value)` pairs for both halves).
      - ✅ Implemented `LeafSplitPlan::apply` + `LeafSplitOutcome`:
        * Input: the plan above, a mutable reference to the original (left) mini-page, and a freshly allocated right-hand mini-page that starts as a byte-for-byte copy of the left page.
        * Steps: clone the source page into the destination, call `reset_user_entries` on both leaves, replay the retained entries into the left page and the moved entries into the right page via the existing `try_put` path, then surface the separator key via `LeafSplitOutcome`.
        * Result: Two valid leaves containing disjoint halves of the original user entries plus the pivot key we need for the parent update (child wiring still todo).
      - ✅ Reworked the lock manager to hand out stable write-guard handles so we can keep the original leaf locked while allocating/promoting the new right-hand mini-page.
   3. Parent/root updates:
      - ✅ Taught `BPTree` a `promote_leaf_root` helper that allocates a fresh inner node, installs the pivot + child pointers, and swaps the root pointer under the root write-lock.
      - ✅ Added `ChildPointer` + `LockedInner` tracking so every ancestor write lock knows its tree level and child IDs (leaf vs inner). This keeps wiring unambiguous when a split cascades.
      - ✅ Implemented `BPNode::insert_entry_after_child` and `split_inner_node`, which rebuild the current inner node, allocate a sibling, and return the propagated pivot/right-child pointer.
      - ✅ Added `BPTree::promote_inner_root` so once the highest inner parent overflows we allocate a brand-new root at `level+1`.
      - ✅ `QuickStepTx::insert_into_parents_after_leaf_split` now updates the immediate parent if space is available, otherwise calls `split_inner_node` and bubbles the resulting pivot upward via `bubble_split_up`.
      - ✅ Updated `MapTable`/`NodeMeta` identity plumbing so freshly split right leaves retain their unique `PageId` + disk address immediately after rebuild; this removes the temporary post-split refresh hack.
      - ✅ Added a test-only `debug_root_leaf_parent` hook (exposed via `QuickStep`) so integration tests can inspect root fan-out/pivots after a split.
      - ✅ Added `QuickStep::debug_leaf_snapshot`, a read-only helper that materialises the user keys for any leaf page (cached mini-page or on-disk leaf) so tests can assert exact key ranges per child.
      - ✅ Added `QuickStep::debug_root_level` to expose the current tree height for integration tests that stress multi-level promotions.
   4. Cache eviction + write-back:
      - ✅ Defined eviction/liveness bitfields on `NodeMeta` so mini-pages can be marked in-flight, converted back to disk leaves, and reclaimed deterministically.
      - ✅ Added `page_op::flush_dirty_entries` and taught `MiniPageBuffer::evict` to invoke it, flip the map-table entry back to `NodeRef::Leaf`, advance the circular-buffer head, and log eviction events.
      - ✅ `QuickStepTx::new_mini_page` now retries failed allocations by driving eviction, so splits and cascading inserts can proceed even when the cache is saturated.

   5. Merge planning (next up)
      - ☐ **Trigger semantics**: merges will be initiated after deletes when a leaf drops below a configurable occupancy threshold (default 25%). Until delete exists, we surface an internal helper to exercise the merge machinery via tests.
      - ✅ **Leaf merge plan**: introduced `LeafMergePlan::from_nodes` to snapshot sibling leaves, validate occupancy, and feed the merge apply path.
      - ✅ **Apply merge**: `LeafMergePlan::apply` rewrites the survivor via `replay_entries`, resets the reclaimed leaf to fences-only, and returns a `LeafMergeOutcome` for instrumentation.
      - ✅ **Parent updates**: `BPNode::remove_entry_for_merge` + `BPTree::remove_child_after_merge` drop the pivot/right-child tuple and rebalance the parent; cascading beyond the root remains TODO.
      - ✅ **Root demotion**: `BPTree::demote_root_after_merge` collapses the root to either a leaf or a smaller inner node when the last pivot disappears.
      - ✅ **Instrumentation**: added `debug::MergeEvent` / `debug::merge_requests()` so tests can assert the survivor + reclaimed page IDs and merged counts.
      - ✅ **Delete-trigger**: `QuickStep::delete` and `QuickStepTx::delete` remove keys from leaves, drop record counts, and invoke the auto-merge helper when occupancy falls below the threshold.
      - ✅ **Tests**: `tests/quickstep_merge.rs` simulates delete-driven merges by truncating leaves, calling the delete API, and verifying both root demotion and “root stays inner but loses a child” scenarios via the new debug helpers.

   6. Tombstone + WAL planning (current)
      - ✅ **Tombstone format**: deletes materialise as `KVRecordType::Tombstone` entries that still contain the user-key suffix; iterators skip them, but `flush_dirty_entries` interprets them as physical removes.
      - ✅ **Dirty tracking**: delete paths flag tombstone entries as dirty so cache eviction / manual flush rewrites the disk leaf before reclaiming the cache slot.
      - ✅ **Flush semantics**: `flush_dirty_entries` now removes tombstoned keys from the `DiskLeaf`, rewrites surviving entries, then checkpoints the WAL for that leaf.
      - ✅ **WAL hook**: introduced `WalManager` (length-prefixed binary log) recording `{page_id, disk_addr, key}` for deletes and `{page_id, disk_addr, key, value}` for inserts; append paths fsync every record before returning to the caller.
      - ✅ **Crash protocol**: on startup `QuickStep::replay_wal()` replays pending puts/tombstones into `IoEngine` pages and truncates the `.wal` file once the reapply succeeds; runtime checkpoints prune per-leaf records after eviction/flush.
      - ✅ **Testing**: `tests/quickstep_delete_persist.rs` now covers both crash scenarios—`wal_replays_deletes_without_manual_flush` for deletes and `wal_replays_puts_without_manual_flush` for inserts.
      - ✅ **Global pressure**: background policy monitors total WAL length (records + bytes) and proactively checkpoints the “noisiest” leaves once thresholds are exceeded; per-leaf stats track record counts/bytes so flushes remove the right entries without blocking foreground writes. Configurable thresholds + WAL debug stats (`QuickStep::debug_wal_stats`) keep observability high for tuning, and a lightweight background monitor thread now raises checkpoint requests when limits are exceeded.
   - ✅ **Config overrides**: `QuickStepConfig::with_env_overrides` reads `QUICKSTEP_WAL_LEAF_THRESHOLD`, `QUICKSTEP_WAL_GLOBAL_RECORD_THRESHOLD`, and `QUICKSTEP_WAL_GLOBAL_BYTE_THRESHOLD`, while `with_cli_overrides` understands `--quickstep-wal-{leaf,global-record,global-byte}-threshold[=N]` flags so deployments can tune flush policy via env vars or CLI; `tests/quickstep_config_env.rs` covers positive + invalid input cases.
   - ✅ **Fence invariants**: Added `QuickStep::debug_leaf_fences` + `tests/quickstep_fence_keys.rs` to assert the root leaf, split children, merge survivors, eviction-flushed leaves, and delete-triggered auto-merge survivors all keep consistent fence bounds; splits now derive upper/lower fences from parent pivots rather than relying on hard-coded sentinels so prefix compression stays correct after splits/merges. WAL records now embed the leaf’s current `[lower, upper]` fences so replay can reinstall the same bounds after a crash, and `QuickStep::debug_disk_leaf_fences` exposes on-disk ranges for verification.

3. **Testing**
   - ✅ Added `tests/quickstep_split.rs::root_split_occurs_and_is_readable`:
     1. Inserts large payloads until the first split occurs, asserting `debug::split_requests() == 1`.
     2. Uses `debug_root_leaf_parent()` to verify the root now has two children and the recorded pivot matches the inserted key distribution.
     3. Runs a fresh transaction that reads back every inserted key to ensure routing follows the new pivot.
   - ✅ Added `tests/quickstep_split.rs::second_split_under_root_adds_third_child`:
     1. Fills the tree until the second split fires under the promoted root, ensuring parent insertion rebuilds the inner node with three children.
     2. Asserts the split log recorded distinct left-page IDs for the first and second splits (page 0 vs the right child) and that `debug_root_leaf_parent()` now shows three children / two pivots.
     3. Re-reads every inserted key to prove the new routing logic is stable.
   - ✅ Added `tests/quickstep_split.rs::post_split_inserts_route_to_expected_children`, which inserts new keys on both sides of the recorded pivot after the first split and proves they land in the correct leaf (via `debug_leaf_snapshot`) without triggering extra splits.
   - ✅ Instrumented pivots/counts (see Pre-flight) are now asserted in the split tests to guarantee the recorded metadata matches the actual leaf contents during and after each split.
   - ✅ Split instrumentation is exposed via `debug::split_events()` so cascading tests can assert exactly which logical leaf split; additional scenarios can build atop this without new hooks.
   - ✅ Leaf snapshots + pivot assertions now verify that every child’s key range is consistent with the recorded pivots after each split, closing the gap between structural and data validation. Snapshots also expose each leaf’s disk address so tests can assert newly created siblings persist to distinct pages immediately after splits.
   - ✅ Added `tests/quickstep_split.rs::root_parent_splits_and_promotes_new_inner_level`, which bulk-loads keys until the root must promote to level ≥2 and asserts `debug_root_level()` reflects the taller tree.
   - ✅ Added `tests/quickstep_eviction.rs::eviction_flushes_dirty_leaf_to_disk`, which constrains the cache to ~8 KiB, forces a split, asserts `debug::evictions() > 0`, and proves every inserted key remains readable afterward.

4. **Open questions**
   - ✅ Resolved 22 Nov 2025: `QuickStep::new` now formats page 0 on disk (header + sentinel fence keys) before bootstrapping the map table, and every subsequent mini-page allocation calls `ensure_fence_keys` so promotion no longer needs a bootstrap path.
   - Inner-node serialization helpers are not implemented yet (`BPNode` currently only supports searching). We will implement just enough (key insertion + child pointer storage) for the root case in this phase.

---

## 1.4 – WAL replay hardening (Planned)

**Goal:** decouple crash recovery from stale disk addresses by logging and replaying mutations per logical `PageId`, reinstalling real fence bounds before rehydrating user entries, and documenting any remaining crash-time limitations.

### 1.4.1 – Format & plumbing updates
1. Rework `WalRecord` to encode the logical `PageId`, fence bounds, and an opaque payload that explicitly references the survivor leaf (no more physical `disk_addr` coupling). We will keep the current fences in the record so replay can install them deterministically.
2. Update the serializer/deserializer to write a compact header `{page_id, lower_len, upper_len, payload_len}` followed by the fence blobs and key/value record payload. While here, teach the WAL writer to batch records per page (length-prefixed group) so replay knows exactly when a leaf’s slice ends.
3. Extend `WalManager` with helpers to stream grouped records: `records_by_page()` returns an iterator of `(PageId, Vec<WalRecord>)`, avoiding the current ad-hoc `Vec` rebuild in `QuickStep::replay_wal`.
4. Modify `QuickStepTx::append_wal_put/delete` to pass the logical `PageId`, fences, and user payload to the new writer. Disk addresses are still needed for checkpoints, but they no longer appear in non-checkpoint records.

### 1.4.2 – Replay redesign
1. During startup, call `records_by_page()` and, for each `PageId`, resolve the latest binding through the map table. If the page is already cached, borrow its `NodeMeta`; otherwise, fetch the disk leaf, promote it into a temporary `DiskLeaf`, and install the logged fences via `reset_user_entries_with_fences`.
2. Apply the grouped WAL payloads in key-sorted order via `NodeMeta::replay_entries`, so replayed leaves stay within page size limits even if earlier on-disk images were fuller. After every page group is applied, write the leaf back to disk and, if the page is cached, refresh the cache copy.
3. Teach replay to install fence metadata before user entries, ensuring delete-triggered auto-merges, evictions, and WAL recovery all agree on the survivor’s key range.
4. Once a page’s WAL group finishes successfully, drop those records from the log (`checkpoint_page`), so the WAL never accumulates stale page bindings again.

### 1.4.3 – Testing & docs
1. Re-enable the merge-crash regression using only public ops (`put`/`delete`/auto-merge). Add a complementary eviction test that drives a WAL replay after evicting one sibling to disk and keeping the other cached.
2. Extend `tests/quickstep_fence_keys.rs` to assert fence monotonicity after WAL replay plus delete-triggered merges, ensuring fence metadata survives splits, merges, evictions, and crashes.
3. Document the new replay pipeline and any remaining crash constraints in `README.md` + this plan. Call out that WAL recovery now trusts logical `PageId`s rather than physical disk addresses, eliminating the `InsufficientSpace` failures we saw when reapplying stale layouts.

### 1.4.4 – Rollout
1. Land the format + replay changes behind a feature branch, run the full `quickstep_delete_persist` suite, and explicitly record the before/after behaviour in `CODING_HISTORY.md`.
2. Once stable, trim the old fence-sentinel code and update the operator docs with the new guarantees (fence metadata logged per leaf, WAL grouped per page). Finish by marking 1.4 as “Complete” in this plan.

