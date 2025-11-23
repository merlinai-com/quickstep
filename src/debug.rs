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

#[derive(Clone, Debug)]
pub struct MergeEvent {
    pub survivor_page: u64,
    pub removed_page: u64,
    pub merged_count: usize,
}

static SPLIT_REQUESTS: AtomicU64 = AtomicU64::new(0);
static MERGE_REQUESTS: AtomicU64 = AtomicU64::new(0);
static EVICTION_REQUESTS: AtomicU64 = AtomicU64::new(0);
static SPLIT_EVENTS: Mutex<Vec<SplitEvent>> = Mutex::new(Vec::new());
static MERGE_EVENTS: Mutex<Vec<MergeEvent>> = Mutex::new(Vec::new());
static SECOND_CHANCE_PASSES: AtomicU64 = AtomicU64::new(0);

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

pub fn record_eviction() {
    EVICTION_REQUESTS.fetch_add(1, Ordering::Relaxed);
}

pub fn record_second_chance() {
    SECOND_CHANCE_PASSES.fetch_add(1, Ordering::Relaxed);
}

pub fn record_merge_event(survivor_page: u64, removed_page: u64, merged_count: usize) {
    MERGE_REQUESTS.fetch_add(1, Ordering::Relaxed);
    if let Ok(mut guard) = MERGE_EVENTS.lock() {
        guard.push(MergeEvent {
            survivor_page,
            removed_page,
            merged_count,
        });
    }
}

pub fn reset_debug_counters() {
    SPLIT_REQUESTS.store(0, Ordering::Relaxed);
    MERGE_REQUESTS.store(0, Ordering::Relaxed);
    EVICTION_REQUESTS.store(0, Ordering::Relaxed);
    SECOND_CHANCE_PASSES.store(0, Ordering::Relaxed);
    let mut guard = match SPLIT_EVENTS.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    guard.clear();
    let mut merges = match MERGE_EVENTS.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    merges.clear();
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

pub fn evictions() -> u64 {
    EVICTION_REQUESTS.load(Ordering::Relaxed)
}

pub fn second_chance_passes() -> u64 {
    SECOND_CHANCE_PASSES.load(Ordering::Relaxed)
}

pub fn merge_requests() -> u64 {
    MERGE_REQUESTS.load(Ordering::Relaxed)
}

pub fn merge_events() -> Vec<MergeEvent> {
    match MERGE_EVENTS.lock() {
        Ok(guard) => guard.clone(),
        Err(poison) => poison.into_inner().clone(),
    }
}
