// DEMO: 32-bit byte-order reversal (Rust mirror of byte_swap_u32_verified.cpp).
// Same shift+mask formula as the C++ version.

fn byte_swap_u32(x: u32) -> u32 {
    ((x >> 24) & 0x0000_00FF)
        | ((x >> 8) & 0x0000_FF00)
        | ((x << 8) & 0x00FF_0000)
        | ((x << 24) & 0xFF00_0000)
}
