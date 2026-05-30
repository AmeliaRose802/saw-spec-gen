// DEMO: BUGGY 8-bit popcount (Rust mirror) — stops at i < 7, misses bit 7.
// SAW disproves with x = 0x80.

fn popcount_u8(x: u8) -> u32 {
    let mut n: u32 = 0;
    let mut i: u32 = 0;
    while i < 7 {   // BUG: was i < 8.
        n += ((x >> i) & 1) as u32;
        i += 1;
    }
    n
}
