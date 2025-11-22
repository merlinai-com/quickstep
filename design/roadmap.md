# Quickstep Roadmap

## Guiding Principles

1. **Honour the reference implementation** – keep Raphael Darley’s architecture intact unless we have clear evidence for change.
2. **Build confidence iteratively** – finish core data-path correctness before layering more features.
3. **Keep HelixDB integration front-and-centre** – this fork exists to help give the HelixDB team a bespoke storage engine.

## Phase 0 – Repository Setup ✅

| No. | Workstream | Progress | Notes |
|-----|------------|----------|-------|
| 0.1 | Fork & rename repo | ✅ | Repo lives at `merlinai-com/quickstep` |
| 0.2 | Attribution & docs | ✅ | README, AUTHORS, design docs reference Raphael Darley and Hao & Chandramouli |
| 0.3 | Codebase analysis | ✅ | `design/codebase-analysis.md` |

## Phase 1 – Core Engine Completion (WIP)

| No. | Task | Progress | Notes |
|-----|------|----------|-------|
| 1.1 | Implement `QuickStep::new()` | ✅ | Wire up BPTree, buffer, map table, IO |
| 1.2 | Finish `put()` happy path | ✅ | Handle mini-page allocation/write, no splitting |
| 1.3 | Implement split/merge logic | WIP | Requires lock escalation + map table updates (coordinate with Raphael where possible) |
| 1.4 | Complete `get()` fence key handling |  | Lower/upper fence construction |
| 1.5 | Implement `abort`/`commit` on `QuickStepTx` |  | Track changes for rollback |

## Phase 2 – Persistence & Buffering

| No. | Task | Progress | Notes |
|-----|------|----------|-------|
| 2.1 | Finish eviction path (`buffer::evict`) |  | Flush dirty mini-pages, reclaim space |
| 2.2 | Improve IO engine (`IoEngine::get_new_addr`) |  | Page allocation & metadata page |
| 2.3 | Add WAL/checkpoint design |  | Decide on WAL vs epoch snapshots |
| 2.4 | Implement copy-on-access caching |  | Complete commented-out logic in `page_op.rs` |

## Phase 3 – Concurrency & Recovery

| No. | Task | Progress | Notes |
|-----|------|----------|-------|
| 3.1 | Stress-test optimistic locking |  | Add concurrency tests |
| 3.2 | Deadlock/livelock audit |  | Validate lock order guarantees (Raphael review appreciated) |
| 3.3 | Crash recovery story |  | WAL replay or manifests |

## Phase 4 – API & Integration Readiness

| No. | Task | Progress | Notes |
|-----|------|----------|-------|
| 4.1 | Implement delete/range APIs |  | Needed for HelixDB |
| 4.2 | Define KV trait + adapter |  | To plug into HelixDB |
| 4.3 | Benchmark harness vs RocksDB, LMDB |  | Validate perf claims |
| 4.4 | Document tuning knobs |  | Cache size, retry counts, etc. |

## Phase 5 – Developer Experience

| No. | Task | Progress | Notes |
|-----|------|----------|-------|
| 5.1 | Add CI (fmt, clippy, tests) |  | GitHub Actions |
| 5.2 | Add `cargo fmt` + `clippy` configs |  | Ensure consistent style |
| 5.3 | Create CONTRIBUTING.md |  | Outline process, coding standards |
| 5.4 | Publish roadmap updates quarterly |  | Keep this doc current |

## Phase 6 – Testing & Benchmarking

| No. | Task | Progress | Notes |
|-----|------|----------|-------|
| 6.1 | Establish unit tests per module |  | `btree`, `node`, `buffer`, `map_table`, etc. |
| 6.2 | Add integration tests using `QuickStep::new()` |  | Basic put/get/delete flows once core functions land |
| 6.3 | Introduce property-based testing (`proptest`) |  | Verify invariants (sorted keys, prefix compression) |
| 6.4 | Build stress/simulation harness |  | Randomised multi-threaded ops to smoke out races |
| 6.5 | Set up Criterion benchmarks |  | Micro (node/buffer) + macro (full KV workloads) |
| 6.6 | Record baseline perf vs RocksDB/LMDB |  | Track improvements/regressions |

## Phase 7 – HelixDB Integration

| No. | Task | Progress | Notes |
|-----|------|----------|-------|
| 7.1 | Document HelixDB storage trait expectations |  | Summarise key APIs & config knobs |
| 7.2 | Scaffold Quickstep adapter for HelixDB |  | Implement trait wrapper around `QuickStep` |
| 7.3 | Add configuration flag in HelixDB to select Quickstep |  | CLI / config integration |
| 7.4 | Run HelixDB’s integration tests with Quickstep backend |  | Validate functional parity |
| 7.5 | Build HelixDB workload benchmark harness |  | Compare LMDB vs Quickstep for graph ops |
| 7.6 | Prepare PR plan for upstream HelixDB |  | Once stable, publish adapter changes |

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

