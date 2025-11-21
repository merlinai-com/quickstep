# Quickstep Roadmap

## Guiding Principles

1. **Honour the reference implementation** – keep Raphael Darley’s architecture intact unless we have clear evidence for change.
2. **Build confidence iteratively** – finish core data-path correctness before layering more features.
3. **Keep HelixDB integration front-and-centre** – this fork exists to help give the HelixDB team a bespoke storage engine.

## Phase 0 – Repository Setup ✅

| Workstream | Status | Notes |
|------------|--------|-------|
| Fork & rename repo | ✅ Done | Repo lives at `merlinai-com/quickstep` |
| Attribution & docs | ✅ Done | README, AUTHORS, design docs reference Raphael Darley and Hao & Chandramouli |
| Codebase analysis | ✅ Done | `design/codebase-analysis.md` |

## Phase 1 – Core Engine Completion (WIP)

| # | Task | Notes |
|---|------|-------|
| 1 | Implement `QuickStep::new()` | Wire up BPTree, buffer, map table, IO |
| 2 | Finish `put()` happy path | Handle mini-page allocation/write, no splitting |
| 3 | Implement split/merge logic | Requires lock escalation + map table updates (coordinate with Raphael where possible) |
| 4 | Complete `get()` fence key handling | Lower/upper fence construction |
| 5 | Implement `abort`/`commit` on `QuickStepTx` | Track changes for rollback |

## Phase 2 – Persistence & Buffering

| # | Task | Notes |
|---|------|-------|
| 6 | Finish eviction path (`buffer::evict`) | Flush dirty mini-pages, reclaim space |
| 7 | Improve IO engine (`IoEngine::get_new_addr`) | Page allocation & metadata page |
| 8 | Add WAL/checkpoint design | Decide on WAL vs epoch snapshots |
| 9 | Implement copy-on-access caching | Complete commented-out logic in `page_op.rs` |

## Phase 3 – Concurrency & Recovery

| # | Task | Notes |
|---|------|-------|
| 10 | Stress-test optimistic locking | Add concurrency tests |
| 11 | Deadlock/livelock audit | Validate lock order guarantees (Raphael review appreciated) |
| 12 | Crash recovery story | WAL replay or manifests |

## Phase 4 – API & Integration Readiness

| # | Task | Notes |
|---|------|-------|
| 13 | Implement delete/range APIs | Needed for HelixDB |
| 14 | Define KV trait + adapter | To plug into HelixDB |
| 15 | Benchmark harness vs RocksDB, LMDB | Validate perf claims |
| 16 | Document tuning knobs | Cache size, retry counts, etc. |

## Phase 5 – Developer Experience

| # | Task | Notes |
|---|------|-------|
| 17 | Add CI (fmt, clippy, tests) | GitHub Actions |
| 18 | Add `cargo fmt` + `clippy` configs | Ensure consistent style |
| 19 | Create CONTRIBUTING.md | Outline process, coding standards |
| 20 | Publish roadmap updates quarterly | Keep this doc current |

## Phase 6 – Testing & Benchmarking

| # | Task | Notes |
|---|------|-------|
| 21 | Establish unit tests per module | `btree`, `node`, `buffer`, `map_table`, etc. |
| 22 | Add integration tests using `QuickStep::new()` | Basic put/get/delete flows once core functions land |
| 23 | Introduce property-based testing (`proptest`) | Verify invariants (sorted keys, prefix compression) |
| 24 | Build stress/simulation harness | Randomized multi-threaded ops to smoke out races |
| 25 | Set up Criterion benchmarks | Micro (node/buffer) + macro (full KV workloads) |
| 26 | Record baseline perf vs RocksDB/LMDB | Track improvements/regressions |

## Phase 7 – HelixDB Integration

| # | Task | Notes |
|---|------|-------|
| 27 | Document HelixDB storage trait expectations | Summarise key APIs & config knobs |
| 28 | Scaffold Quickstep adapter for HelixDB | Implement trait wrapper around `QuickStep` |
| 29 | Add configuration flag in HelixDB to select Quickstep | CLI / config integration |
| 30 | Run HelixDB’s integration tests with Quickstep backend | Validate functional parity |
| 31 | Build HelixDB workload benchmark harness | Compare LMDB vs Quickstep for graph ops |
| 32 | Prepare PR plan for upstream HelixDB | Once stable, publish adapter changes |

## Open Questions

- What durability guarantees do we aim for initially? (Best-effort vs crash-safe)
- How tightly do we need to couple to HelixDB’s key format?
- Do we want to maintain binary compatibility with the canonical Quickstep?

## Next Steps (0–4 weeks)

1. Finish `QuickStep::new()` wiring.
2. Complete `put()` without splits and add smoke tests.
3. Implement split logic (start with leaf splits).
4. Stand up a minimal WAL or at least durable flush for dirty mini-pages.

Progress should be tracked in GitHub issues; tag roadmap items with `phase-1`, `phase-2`, etc.

