// DEMO: 16-bit byte swap (Rust mirror of swap_bytes_u16_verified.cpp).
// Exercises u16 parameter + return, absent from earlier suites.

fn swap_bytes_u16(x: u16) -> u16 {
    (x >> 8) | (x << 8)
}
