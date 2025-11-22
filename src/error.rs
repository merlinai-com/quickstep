#[derive(Debug)]
pub enum QSError {
    /// There was a failure to acquire a page lock
    PageLockFail,
    /// The number of retries on inner nodes was exceeded
    OLCRetriesExceeded,
    /// The mini-page buffer could not allocate space for a promotion
    CacheExhausted,
    /// Internal error while applying a leaf split
    SplitFailed,
    /// Internal node is full and requires its own split
    NodeFull,
    /// Parent node did not contain expected child pointer
    ParentChildMissing,
    /// Inner node slab exhausted
    TreeFull,
    /// Pivot key exceeded internal node storage limits
    KeyTooLarge,
}
