#[derive(Debug)]
pub enum QSError {
    /// There was a failure to acquire a page lock
    PageLockFail,
    /// The number of retries on inner nodes was exceeded
    OLCRetriesExceeded,
}
