// DEMO: BUGGY 64-bit add — subtracts instead. SAW disproves.

fn add_u64(a: u64, b: u64) -> u64 {
    a.wrapping_sub(b)   // BUG: was wrapping_add.
}
