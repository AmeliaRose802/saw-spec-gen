// Rust implementation of get_status.
// Returns a 4-byte struct via sret (too large for register return).

#[repr(C)]
pub struct StatusResult {
    pub enabled: u8,
    pub active: u8,
    pub result: u8,
    pub reserved: u8,
}

#[inline(never)]
pub fn get_status(enabled: u8, active: u8) -> StatusResult {
    StatusResult {
        enabled,
        active,
        result: if enabled == 1 && active == 1 { 1 } else { 0 },
        reserved: 0,
    }
}
