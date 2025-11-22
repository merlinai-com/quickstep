#![allow(dead_code)]

use std::sync::{
    atomic::{AtomicU64, Ordering},
    Mutex,
};

#[derive(Clone, Debug)]
pub struct SplitEvent {
    pub left_page: u64,
    pub right_page: u64,
    pub pivot_key: Vec<u8>,
    pub left_count: usize,
    pub right_count: usize,
}

static SPLIT_REQUESTS: AtomicU64 = AtomicU64::new(0);
static SPLIT_EVENTS: Mutex<Vec<SplitEvent>> = Mutex::new(Vec::new());

pub fn record_split_event(
    left_page: u64,
    right_page: u64,
    pivot_key: Vec<u8>,
    left_count: usize,
    right_count: usize,
) {
    SPLIT_REQUESTS.fetch_add(1, Ordering::Relaxed);
    if let Ok(mut guard) = SPLIT_EVENTS.lock() {
        guard.push(SplitEvent {
            left_page,
            right_page,
            pivot_key,
            left_count,
            right_count,
        });
    }
}

pub fn reset_debug_counters() {
    SPLIT_REQUESTS.store(0, Ordering::Relaxed);
    let mut guard = match SPLIT_EVENTS.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    guard.clear();
}

pub fn split_requests() -> u64 {
    SPLIT_REQUESTS.load(Ordering::Relaxed)
}

pub fn split_events() -> Vec<SplitEvent> {
    match SPLIT_EVENTS.lock() {
        Ok(guard) => guard.clone(),
        Err(poison) => poison.into_inner().clone(),
    }
}
