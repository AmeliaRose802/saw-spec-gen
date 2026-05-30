// DEMO: Power-of-two predicate (Rust mirror of is_power_of_two_verified.cpp).
//
// Returns 1 iff x is exactly 2^k for 0 <= k < 32. The `wrapping_sub`
// keeps the `x = 0` case well-defined (0u32.wrapping_sub(1) = 0xFFFFFFFF),
// then the `x != 0` guard rejects it.

fn is_power_of_two(x: u32) -> u32 {
    if x != 0 && (x & x.wrapping_sub(1)) == 0 { 1 } else { 0 }
}
