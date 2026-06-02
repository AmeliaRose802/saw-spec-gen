// DEMO: 64-bit unsigned wrapping add (Rust mirror of add_u64_verified.cpp).
// Exercises u64 parameter + return. `wrapping_add` because verify-rust.ps1
// disables overflow checks, but plain `+` would also work in release.

fn add_u64(a: u64, b: u64) -> u64 {
    a.wrapping_add(b)
}
