pub fn store_u48(val: u64) -> [u8; 6] {
    let masked = val & 0x0000_FFFF_FFFF_FFFF;
    let bytes = masked.to_be_bytes();
    [bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7]]
}

pub fn extract_u48(val: *const u8) -> u64 {
    let slice = unsafe { std::slice::from_raw_parts(val, 6) };
    let mut buf = [0u8; 8];
    buf[2..].copy_from_slice(slice);
    u64::from_be_bytes(buf)
}

pub fn store_u32(val: u64) -> [u8; 8] {
    let masked = (val as u32).to_be_bytes();
    let mut buf = [0u8; 8];
    buf[4..].copy_from_slice(&masked);
    buf
}

pub fn extract_u32(val: *const u8) -> u32 {
    let slice = unsafe { std::slice::from_raw_parts(val, 4) };
    let mut buf = [0u8; 4];
    buf.copy_from_slice(slice);
    u32::from_be_bytes(buf)
}
