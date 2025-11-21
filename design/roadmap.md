# Quickstep Roadmap

## Guiding Principles

1. **Honor the reference implementation** – keep Raphael Darley’s architecture intact unless we have clear evidence for change.
2. **Build confidence iteratively** – finish core data-path correctness before layering more features.
3. **Keep HelixDB integration front-and-centre** – this fork exists to help give the HelixDB team a bespoke storage engine.

## Phase 0 – Repository Setup ✅

| Workstream | Status | Notes |
|------------|--------|-------|
| Fork & rename repo | ✅ Done | Repo lives at `merlinai-com/quickstep` |
| Attribution & docs | ✅ Done | README, AUTHORS, design docs reference Raphael Darley |
| Codebase analysis | ✅ Done | `design/codebase-analysis.md` |

## Phase 1 – Core Engine Completion (WIP)

| # | Task | Owner | Notes |
|---|------|-------|-------|
| 1 | Implement `QuickStep::new()` | MerlinAI | Wire up BPTree, buffer, map table, IO |
| 2 | Finish `put()` happy path | MerlinAI | Handle mini-page allocation/write, no splitting |
| 3 | Implement split/merge logic | MerlinAI + Raphael if possible | Requires lock escalation + map table updates |
| 4 | Complete `get()` fence key handling | MerlinAI | Lower/upper fence construction |
| 5 | Implement `abort`/`commit` on `QuickStepTx` | MerlinAI | Track changes for rollback |

## Phase 2 – Persistence & Buffering

| # | Task | Owner | Notes |
|---|------|-------|-------|
| 6 | Finish eviction path (`buffer::evict`) | MerlinAI | Flush dirty mini-pages, reclaim space |
| 7 | Improve IO engine (`IoEngine::get_new_addr`) | MerlinAI | Page allocation & metadata page |
| 8 | Add WAL/checkpoint design | MerlinAI | Decide on WAL vs epoch snapshots |
| 9 | Implement copy-on-access caching | MerlinAI | Complete commented-out logic in `page_op.rs` |

## Phase 3 – Concurrency & Recovery

| # | Task | Owner | Notes |
|---|------|-------|-------|
| 10 | Stress-test optimistic locking | MerlinAI | Add concurrency tests |
| 11 | Deadlock/livelock audit | MerlinAI + Raphael | Validate lock order guarantees |
| 12 | Crash recovery story | MerlinAI | WAL replay or manifests |

## Phase 4 – API & Integration Readiness

| # | Task | Owner | Notes |
|---|------|-------|-------|
| 13 | Implement delete/range APIs | MerlinAI | Needed for HelixDB |
| 14 | Define KV trait + adapter | MerlinAI | To plug into HelixDB |
| 15 | Benchmark harness vs RocksDB, LMDB | MerlinAI | Validate perf claims |
| 16 | Document tuning knobs | MerlinAI | Cache size, retry counts, etc. |

## Phase 5 – Developer Experience

| # | Task | Owner | Notes |
|---|------|-------|-------|
| 17 | Add CI (fmt, clippy, tests) | MerlinAI | GitHub Actions |
| 18 | Add `cargo fmt` + `clippy` configs | MerlinAI | Ensure consistent style |
| 19 | Create CONTRIBUTING.md | MerlinAI | Outline process, coding standards |
| 20 | Publish roadmap updates quarterly | MerlinAI | Keep this doc current |

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

