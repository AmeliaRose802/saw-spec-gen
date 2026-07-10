// Rust implementation of get_status.
// Returns a 24-byte struct via sret (three u64 fields; larger than two
// registers on x86_64, so the Itanium ABI uses a hidden first pointer
// argument — i.e. sret).

#[repr(C)]
pub struct StatusResult {
    pub enabled: u64,
    pub active: u64,
    pub result: u64,
}

#[inline(never)]
pub fn get_status(enabled: u8, active: u8) -> StatusResult {
    StatusResult {
        enabled: enabled as u64,
        active: active as u64,
        result: if enabled == 1 && active == 1 { 1 } else { 0 },
    }
}
