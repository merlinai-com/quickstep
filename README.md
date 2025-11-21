# Quickstep

A modern, concurrent embedded key-value store implementing the [Bf-tree](https://github.com/XiangpengHao/bf-tree-docs) data structure in Rust.

## Overview

Quickstep is an implementation of a Bf-tree (B-tree with buffer pool optimization), designed as a high-performance, concurrent storage engine. It provides:

- **Concurrent access** with optimistic locking and fine-grained page-level locks
- **Variable-size mini-pages** for efficient memory usage and write buffering
- **Prefix compression** for reduced storage overhead
- **Larger-than-memory** support for datasets that exceed available RAM
- **Range index** with sorted key-value storage

## Status

⚠️ **Early Development** - This project is in active development. Core architecture is implemented, but many features are still being completed. See [Current Implementation Status](#current-implementation-status) below.

## What is a Bf-tree?

The Bf-tree is a modern read-write-optimized concurrent range index, as described in the [VLDB 2024 paper](https://github.com/XiangpengHao/bf-tree-docs) by Xiangpeng Hao and Badrish Chandramouli. Key innovations include:

- **Mini-pages**: Variable-size pages (64B to 4KB) that serve as both a record-level cache and write buffer
- **Circular buffer pool**: Efficient memory management with second-chance eviction for LRU approximation
- **Optimized for modern SSDs**: Designed for parallel random 4KB writes with similar throughput to sequential writes

For detailed explanations and visualizations, see the [Bf-tree documentation repository](https://github.com/XiangpengHao/bf-tree-docs).

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

### ⚠️ Partially Implemented

- Put/get operations (core logic present, split handling incomplete)
- Buffer eviction (structure present, merge-to-disk incomplete)
- I/O engine (read/write work, address allocation missing)
- Initialization (`QuickStep::new()` is `todo!()`)

### ❌ Not Yet Implemented

- Transaction commit/abort
- Split/merge operations
- Recovery/WAL (write-ahead logging)
- Range queries
- Delete operations

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
- **MerlinAI**: This fork is maintained by [MerlinAI](https://github.com/merlinai-com) for integration with our platform

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
