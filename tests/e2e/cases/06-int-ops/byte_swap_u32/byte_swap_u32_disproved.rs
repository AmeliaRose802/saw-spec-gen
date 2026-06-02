// DEMO: BUGGY 32-bit byte-order reversal (Rust mirror).
// First lane uses `>> 16` instead of `>> 24` — SAW finds a counterexample
// whenever the top input byte is non-zero.

fn byte_swap_u32(x: u32) -> u32 {
    ((x >> 16) & 0x0000_00FF)   // BUG: was (x >> 24) & 0x0000_00FF
        | ((x >> 8) & 0x0000_FF00)
        | ((x << 8) & 0x00FF_0000)
        | ((x << 24) & 0xFF00_0000)
}
