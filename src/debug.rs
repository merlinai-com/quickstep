#![allow(dead_code)]

use std::sync::{
    atomic::{AtomicU64, Ordering},
    Mutex,
};

#[derive(Clone, Debug)]
pub struct SplitEvent {
    pub left_page: u64,
    pub right_page: u64,
}

static SPLIT_REQUESTS: AtomicU64 = AtomicU64::new(0);
static SPLIT_EVENTS: Mutex<Vec<SplitEvent>> = Mutex::new(Vec::new());

pub fn record_split_event(left_page: u64, right_page: u64) {
    SPLIT_REQUESTS.fetch_add(1, Ordering::Relaxed);
    if let Ok(mut guard) = SPLIT_EVENTS.lock() {
        guard.push(SplitEvent {
            left_page,
            right_page,
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
