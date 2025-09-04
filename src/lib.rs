pub mod btree;
pub mod buffer;
pub mod map_table;
pub mod node;
pub mod types;

pub const SPIN_RETRIES: usize = 256;

const _: () = assert!(std::mem::size_of::<usize>() == std::mem::size_of::<u64>());
