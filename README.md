# Quickstep

A modern, concurrent embedded key-value store implementing the [Bf-tree](https://github.com/XiangpengHao/bf-tree-docs) data structure in Rust. The core implementation is led by [Raphael Darley](https://github.com/RaphaelDarley); this repository tracks MerlinAI’s public fork and contributions, with the explicit goal of powering the HelixDB storage layer.

## Overview

Quickstep is an implementation of a Bf-tree (B-tree with buffer pool optimisation), designed as a high-performance, concurrent storage engine. It provides:

- **Concurrent access** with optimistic locking and fine-grained page-level locks
- **Variable-size mini-pages** for efficient memory usage and write buffering
- **Prefix compression** for reduced storage overhead
- **Larger-than-memory** support for datasets that exceed available RAM
- **Range index** with sorted key-value storage

## Status

⚠️ **Early Development** - This project is in active development. Core architecture is implemented, but many features are still being completed. See [Current Implementation Status](#current-implementation-status) below.

## What is a Bf-tree?

The Bf-tree is a modern read-write-optimised concurrent range index, as described in the [VLDB 2024 paper](https://github.com/XiangpengHao/bf-tree-docs) by Xiangpeng Hao and Badrish Chandramouli. Key innovations include:

- **Mini-pages**: Variable-size pages (64B to 4KB) that serve as both a record-level cache and write buffer
- **Circular buffer pool**: Efficient memory management with second-chance eviction for LRU approximation
- **Optimised for modern SSDs**: Designed for parallel random 4KB writes with similar throughput to sequential writes

For detailed explanations and visualisations, see the [Bf-tree documentation repository](https://github.com/XiangpengHao/bf-tree-docs).

## Architecture

Quickstep consists of several key components:

- **B+ Tree** (`btree.rs`): Inner node structure with optimistic version-based locking
- **Mini-page Buffer** (`buffer.rs`): Circular buffer pool with variable-size page allocation
- **Mapping Table** (`map_table.rs`): Page ID to mini-page/leaf mapping with fine-grained locks
- **I/O Engine** (`io_engine.rs`): File-based persistence layer
- **Transaction Manager** (`lock_manager.rs`): Transaction-scoped lock tracking

See [`design/codebase-analysis.md`](design/codebase-analysis.md) for a detailed architecture overview.

## Getting Started

### Prerequisites

- Rust 1.70+ (2021 edition)
- A Unix-like system (uses Unix file extensions)

### Building

```bash
git clone https://github.com/merlinai-com/quickstep.git
cd quickstep
cargo build
```

### Running Tests

```bash
cargo test
```

### Documentation

Generate and view the API documentation:

```bash
cargo doc --open
```

## Current Implementation Status

### ✅ Implemented

- B+ tree inner node structure and optimistic locking
- Mini-page buffer allocation
- Mapping table with fine-grained page-level locks
- Transaction lock manager
- Leaf node prefix compression
- Key-value metadata encoding
- `QuickStep::new()` bootstraps the tree, cache, and map table, formatting the root leaf (page 0) on disk before the first transaction
- Delete/tombstone plumbing persists user-key removals via mini-page flush, WAL replay on restart, and instrumentation-backed tests
- Minimal WAL support (puts + deletes) replays cached updates during startup, per-leaf checkpoints prune the log, and a global WAL pressure monitor flushes the busiest leaves when the log grows too large
- Configurable WAL thresholds via `QuickStepConfig::with_wal_thresholds(...)`, the `QUICKSTEP_WAL_LEAF_THRESHOLD`, `QUICKSTEP_WAL_GLOBAL_RECORD_THRESHOLD`, and `QUICKSTEP_WAL_GLOBAL_BYTE_THRESHOLD` env vars, or CLI flags `--quickstep-wal-{leaf,global-record,global-byte}-threshold`, plus debug WAL stats (`QuickStep::debug_wal_stats`) and a lightweight background WAL monitor for observability/auto-checkpointing
- Fence guards derived from parent pivots via `QuickStep::debug_leaf_fences`, with integration tests (`tests/quickstep_fence_keys.rs`) that verify page 0 uses the sentinel `[0x00]`/`[0xFF]` bounds while split children, merge survivors, eviction-flushed leaves, and delete-triggered auto-merge survivors maintain monotonic lower/upper fences that cover their user keys; WAL entries now embed those fence bounds so crash replay reinstalls the same ranges before applying writes
- WAL records are grouped per logical `PageId`, checkpoints operate on `checkpoint_page(PageId)`, and startup replay hydrates both disk and cached leaves before flushing; the merge-crash regression runs entirely through the public API.
- WAL records are grouped per logical `PageId`, and crash replay reinstalls each leaf’s `[lower, upper]` bounds plus the sorted key/value set before writing back to disk; the merge-crash regression now passes via public operations only.

### ⚠️ Partially Implemented

- Put/get operations (mini-page promotion + cache writes are in place; cascading parent splits bubble to the root and publish new map-table entries immediately, while merge/eviction policies are still being fleshed out)
- Leaf split logic (Phase 1.3 now exercises root splits, cascading inner splits, and root promotions via instrumentation-backed integration tests; remaining work focuses on merge handling)
- Buffer eviction (baseline FIFO eviction flushes dirty mini-pages back to disk and updates the map table in place; second-chance policy & mixed-size freelists are still TODO)
- Delete/merge path (delete API now logs tombstones through the WAL, replays them during restart, triggers per-leaf checkpoints, and auto-merges underfilled siblings; range deletes + multi-level recovery semantics remain TODO)
- Merge logic (leaf-level merge plan, parent rewiring, root demotion, and merge instrumentation are implemented; delete-triggered thresholds and non-root cascading merges are still outstanding)
- Buffer eviction (structure present, merge-to-disk incomplete)
- I/O engine (read/write path works; WAL still lacks redo/undo for complex transactions and finer-grained checkpoint orchestration)
- WAL crash recovery now relies on logical `PageId`s with length-prefixed page groups. Remaining work focuses on broader eviction/replay regression coverage and documenting the new on-disk framing plus redo/undo requirements (see `design/detailed-plan.md` §1.4/§2.3).

### ❌ Not Yet Implemented

- Transaction commit/abort
- Full redo/undo logging for values beyond delete tombstones + crash-safe checkpoints
- Range queries

For a detailed breakdown, see [`design/codebase-analysis.md`](design/codebase-analysis.md).

## Project Structure

```
quickstep/
├── src/              # Source code
│   ├── lib.rs        # Public API
│   ├── btree.rs      # B+ tree for inner nodes
│   ├── buffer.rs     # Mini-page buffer pool
│   ├── map_table.rs  # Page mapping
│   ├── node.rs       # Leaf node operations
│   └── ...
├── design/           # Design documentation
│   ├── codebase-analysis.md
│   └── bf-tree-docs/ # Reference materials
├── scripts/          # Utility scripts
└── Cargo.toml        # Rust project configuration
```

## Design Documentation

This repository includes comprehensive design documentation:

- [`design/codebase-analysis.md`](design/codebase-analysis.md) - Detailed analysis of the codebase architecture
- [`design/bf-tree-docs/`](design/bf-tree-docs/) - Reference materials from the Bf-tree paper authors
- [`design/roadmap.md`](design/roadmap.md) - Phased roadmap and upcoming work; tasks are enumerated legal-style (`phase.task`) so dependencies and numbering remain clear

## Contributing

Contributions are welcome! This project is actively being developed. Please feel free to:

- Open issues for bugs or feature requests
- Submit pull requests for improvements
- Share feedback and suggestions

## License

This project is licensed under either of:

- Apache License, Version 2.0 (http://www.apache.org/licenses/LICENSE-2.0)
- MIT license (http://opensource.org/licenses/MIT)

at your option. See `Cargo.toml` for details.

## Acknowledgments

- **Original Implementation**: This project is based on the work by [Raphael Darley](https://github.com/RaphaelDarley/quickstep)
- **Bf-tree Research**: Inspired by the research of Xiangpeng Hao and Badrish Chandramouli ([paper](https://github.com/XiangpengHao/bf-tree-docs))
- **HelixDB / MerlinAI**: This fork is maintained by the small MerlinAI team (with Raphael’s access) specifically to integrate the storage engine into [HelixDB](https://github.com/HelixDB/helix-db)

## Related Projects

- [HelixDB](https://github.com/HelixDB/helix-db) - Graph-vector database that may use Quickstep as a storage engine
- [Bf-tree Documentation](https://github.com/XiangpengHao/bf-tree-docs) - Official documentation and resources

## References

- Hao, X., & Chandramouli, B. (2024). Bf-Tree: A Modern Read-Write-Optimized Concurrent Larger-Than-Memory Range Index. *Proceedings of the VLDB Endowment*, 17(11), 3442-3455.

## Support

For questions or issues:
- Open an issue on GitHub
- Check the [design documentation](design/) for implementation details

---

**Note**: This is an early-stage implementation. The API and internals may change significantly as development progresses.
