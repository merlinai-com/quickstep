pub fn rand_for_cache() -> bool {
    let val = fastrand::u8(0..100);
    // cache 20% of the time
    // The paper suggests that this is a sensible default
    // for maximising throughput
    val < 20
}
