# Quickstep Technical Status Report for Raphael Darley

**Timestamp:** 2025-11-22 23:50 UTC  
**Prepared by:** MerlinAI fork maintainers (Cursor agent)  
**Scope:** `/APPS/quickstep` fork plus design collateral under `design/`

---

## 1. Executive Snapshot
- **Phase 1 milestones:** Tasks 1.1–1.4 in `design/detailed-plan.md` are delivered; splits, merges, WAL replay, and eviction are instrumented and covered by integration tests (`tests/quickstep_*`).
- **Durability improvements:** WAL logging now captures logical `PageId`s plus per-leaf fence bounds; restart replay rebuilds cached and on-disk leaves before truncating the log (`CHANGELOG.md`, entries dated 2025-11-22 17:35–19:45 UTC).
- **Instrumentation/test depth:** New debug hooks (`QuickStep::debug_leaf_snapshot`, `debug_leaf_fences`, `debug_root_level`, `debug_wal_stats`) power regression suites for splits, merges, fence monotonicity, eviction, and WAL replay (see `design/phase-1-tests.md` and `tests/quickstep_*.rs`).
- **Primary gaps:** Cascading re-split after an initial leaf split is still `todo!()`, transactions lack logical commit/abort semantics beyond WAL markers, range scans and concurrent stress tests are absent, and eviction recycling (`MiniPageBuffer::dealloc`) is unfinished.
- **HelixDB alignment:** The vendored `design/helixdb` snapshot confirms LMDB is still the default storage backend; Quickstep must reach Phase 4 (API parity, KV trait adapter, benchmarks) before integration (see `design/roadmap.md` §4–§7).

---

## 2. Implementation Overview

| Area | What works | Key gaps / risks |
| --- | --- | --- |
| **Public API (`src/lib.rs`)** | `QuickStep::new` formats page 0, replays WAL, wires cache/tree/map table, and starts a WAL pressure monitor thread. Reads (`QuickStepTx::get`), puts with promotion, deletes with tombstones+auto-merge, and debug helpers are functional. | `QuickStepTx::put` still hits `todo!()` if a second split is required immediately after the first (no re-entrant splitting). Transactions only emit WAL markers; no undo replay is wired into `abort()`/`commit()` beyond WAL metadata, so crash-safe rollback is incomplete. |
| **B+ tree (`src/btree.rs`)** | Optimistic locking, write-lock bundles, root promotion/demotion, parent splits/merges, child pointer tracking, and split instrumentation all work. | Node free list reclamation is not implemented, so inner-node slab exhaustion is possible under churn. |
| **Mini-page buffer (`src/buffer.rs`)** | Circular allocator with per-size freelists, eviction pipeline that flushes dirty mini-pages, map-table rewiring, and eviction instrumentation. | `dealloc` is still a stub: reclaimed pages are not reinserted into freelists, so long-running workloads rely solely on eviction rather than reuse. |
| **Map table + lock manager** | Page-level reader/writer locks with write-pending fairness; transaction-scoped lock cache prevents double-locking. | Lock upgrade starvation is possible under heavy contention because writer waiting bits are not yet implemented. |
| **Node layer (`src/node.rs`, `src/types.rs`)** | Prefix compression, KV metadata packing, tombstones, replay helpers, fence installation, and record-count bookkeeping are complete. | `KVMeta::set_key_size`/`set_val_size` remain `todo!()` (unused today, but required for compaction). |
| **WAL / durability (`src/wal.rs`)** | Length-prefixed page groups, logical `PageId` records, redo+undo entry kinds, per-leaf stats, background checkpoint triggers, fence metadata capture, and replay-with-fences are in place. | Undo records are appended but never consumed; checkpoint manifests/LSNs (plan §2.3) are still future work, so the WAL grows until per-leaf checkpoints succeed. |
| **Tests** | `cargo test` suite covers new, put, split, merge, delete persistence, eviction, fence bounds, WAL config overrides. Each scenario is documented in `design/phase-1-tests.md`. | No concurrency, property-based, or range-query tests yet; warnings persist due to unfinished modules. |

---

## 3. Subsystem Detail

### 3.1 Public API & Transactions
- `QuickStep::new` now composes config overrides from env/CLI, resolves data paths, opens the IO engine + WAL, spawns a background WAL monitor, ensures page 0 exists, replays WAL entries, and seeds the map table/root.
- Transaction handles emit WAL begin markers and maintain undo logs, but `commit()`/`abort()` only append markers:

```639:644:src/lib.rs
    pub fn commit(self) {
        self.db
            .wal
            .append_txn_marker(WalTxnMarker::Commit, self.wal_entry_kind, self.txn_id)
            .expect("failed to record txn commit");
    }
```

  There is no enforcement that committed pages are flushed or that aborted operations replay the undo log against cached leaves, so crash safety relies entirely on WAL replay (per plan §2.3 this will change).

- Insert path: promotion + leaf splits are wired, but nested splits immediately after a split return `todo!()`:

```612:621:src/lib.rs
                match Self::try_put_with_promotion(self.db, target_guard, key, val)? {
                    TryPutResult::Success => { … }
                    TryPutResult::NeedsSplit => {
                        todo!("split cascading is not yet implemented");
                    }
```

  Tests never trigger this path yet; cascading split work should target this block.

### 3.2 Buffering and Eviction
- Circular buffer allocator honors `NodeSize` tiers and falls back to eviction when space is unavailable. Eviction flushes dirty records via `page_op::flush_dirty_entries`, checkpoints WAL entries, rewrites the map table to a disk leaf, and advances the head pointer.
- Free-list recycling is incomplete:

```262:272:src/buffer.rs
    pub unsafe fn dealloc(&self, node: MiniPageIndex) {
        let node_meta: &NodeMeta = self.get_meta_ref(node);
        // if its in the second chance region, there's no point adding it to a free list
        if todo!("Is in the second chance region") {
            // node_meta.
        } else {
        }
        let size = node_meta.size();
    }
```

  Without deallocation, long-lived workloads might churn through the buffer faster than eviction can recycle slots, especially once second-chance policy is enabled (see roadmap §2.1).

### 3.3 WAL & Crash Recovery
- WAL manager now operates at the logical `PageId` level, storing fence bounds and redo payloads. Replay hydrates `BTreeMap<Vec<u8>, Vec<u8>>` per page, reinstalls fences, and applies entries to both disk and cached leaves before truncating.
- Undo entries are emitted for every mutation but not yet replayed. The detailed plan §2.3 recommends adding an LSN/manifest so redo pruning does not discard undo coverage.
- WAL thresholds are configurable via env (`QUICKSTEP_WAL_*`) and CLI flags (`--quickstep-wal-*`), and `QuickStep::debug_wal_stats` allows tests/monitoring to inspect per-leaf counts/bytes.

### 3.4 Node / B-tree internals
- `NodeMeta` now exposes `reset_user_entries_with_fences`, `replay_entries`, tombstone helpers, and entry iterators, enabling splits/merges/WAL replay to rebuild leaves deterministically.
- Inner nodes keep a 4 KiB slab per entry, with `ChildPointer` tracking and root lock versioning. However, node reclamation/free lists remain unimplemented, so long sequences of splits could exhaust the `inner_node_upper_bound`.

### 3.5 Testing & Instrumentation
- Tests cover: constructor (`quickstep_new`), happy-path put (`quickstep_put_basic`), split permutations (`quickstep_split`), eviction (`quickstep_eviction`), merges (`quickstep_merge`), fence invariants (`quickstep_fence_keys`), config overrides (`quickstep_config_env`), and WAL persistence (`quickstep_delete_persist`).
- Instrumentation infrastructure:
  - `debug::split_events()` logs left/right page IDs, pivots, entry counts.
  - `debug::merge_events()` logs survivor/reclaimed pages.
  - `debug_leaf_snapshot`/`debug_leaf_fences` expose key ranges per leaf.
  - `debug_wal_stats` provides leaf-level WAL metrics.
- Remaining test gaps noted in `design/phase-1-tests.md` §1.5: no concurrency or abort coverage; range scans and stress tests are deferred to later phases.

---

## 4. Design Documentation Review
- `design/detailed-plan.md` (dated 2025-11-21/22) tracks Phase 1.1–1.4 completion with granular checklists. Phase 1.3/1.4 items are marked complete except for future work (WAL manifest, eviction-focused replay tests).
- `design/phase-1-tests.md` enumerates each test suite with command, outcome, and interpretation; it explicitly calls out missing transaction tests (pending until `abort`/`commit` semantics are implemented).
- `design/roadmap.md` uses `phase.task` numbering; Phase 1 tasks are mostly checked, whereas Phases 2–4 remain empty. Integration with HelixDB (Phase 7) depends on finishing Phase 4 (API readiness) and Phase 6 (benchmarks).
- `design/helixdb/README.md` reiterates HelixDB’s LMDB dependency and RAG-oriented goals, reinforcing the need for Quickstep to match LMDB’s KV semantics plus deliver graph-friendly range queries.
- `design/bf-tree-docs/` + `Bf-Tree—A Modern Read-Write-Optimized…pdf` remain the reference; `design/info.txt` captures citation links for Hao & Chandramouli.

---

## 5. Outstanding Issues & Recommendations
1. **Cascading split completion:** Implement the `todo!()` within `QuickStepTx::put` to handle re-splitting when the target leaf is still full after the initial split, and add regression coverage (Phase 1.3 checklist item).
2. **Transaction semantics:** Extend `QuickStepTx::commit/abort` to flush redo buffers and apply undo actions on abort before releasing locks. Hook the existing `undo_log` into crash recovery so WAL markers correspond to durable state (`design/phase-1-tests.md` §1.5).
3. **Buffer deallocation & second-chance policy:** Finish `MiniPageBuffer::dealloc` and the second-chance tracking bits so eviction can recycle slots deterministically (roadmap §2.1).
4. **Undo-aware WAL checkpoints:** Implement the manifest/LSN design from `detailed-plan.md` §2.3 so undo records survive checkpoints and WAL size stays bounded.
5. **Concurrency and stress testing:** Add multi-threaded transactional tests plus property-based checks to validate locking order, eviction correctness, and WAL replay under concurrent mutations (roadmap §3.1/§6.4).
6. **Range scan API:** Public API still lacks range iterators, which HelixDB will need for graph traversals (roadmap §4.1). Plan the key encoding + iterator semantics now that fence correctness is in place.
7. **HelixDB adapter alignment:** Begin Phase 4 planning (KV trait, adapter crate) so Quickstep’s API evolution matches HelixDB’s expectations documented in `design/helixdb/README.md`.

---

## 6. Next Steps (Suggested Order)
1. **Stabilize split/merge edge cases:** Finish cascading split support, add regression tests, and ensure map-table identities remain stable after multi-level splits (leveraging `debug_root_level` instrumentation).
2. **Transactional correctness pass:** Wire undo replay into `abort()`, add positive/negative transaction tests, and ensure WAL replay distinguishes committed vs aborted txn IDs (reusing `WalTxnMarker` metadata).
3. **Finalize eviction recycling:** Implement freelist re-entry in `MiniPageBuffer::dealloc`, add metrics, and expose debug counters so we can verify second-chance policy before Phase 2 benchmarks.
4. **Checkpoint manifest + redo/undo policy:** Design the WAL truncation protocol (per `detailed-plan` §2.3.2) and document operator guidance for the new CLI/env knobs.
5. **Begin integration planning:** Draft the HelixDB KV adapter outline (Phase 4.2) and identify any API deltas (range queries, iterators) that must exist before Phase 7.

---

Please reach out if you need deeper dives on any subsystem; all cited files/tests are under `/APPS/quickstep` at the timestamps recorded above.

