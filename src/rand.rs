pub fn rand_for_cache() -> bool {
    let val = fastrand::u8(0..100);
    // cache 1% of the time
    val < 1
}
