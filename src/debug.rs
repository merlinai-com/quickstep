#![allow(dead_code)]

use std::sync::atomic::{AtomicU64, Ordering};

static SPLIT_REQUESTS: AtomicU64 = AtomicU64::new(0);

pub fn record_split_request() {
    SPLIT_REQUESTS.fetch_add(1, Ordering::Relaxed);
}

pub fn reset_debug_counters() {
    SPLIT_REQUESTS.store(0, Ordering::Relaxed);
}

pub fn split_requests() -> u64 {
    SPLIT_REQUESTS.load(Ordering::Relaxed)
}
