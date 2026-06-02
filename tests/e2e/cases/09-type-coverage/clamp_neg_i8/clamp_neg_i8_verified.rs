// DEMO: clamp-negative-to-zero, i8 (Rust mirror of clamp_neg_i8_verified.cpp).
// Exercises i8 parameter + return (signed 8-bit), absent from earlier suites.

fn clamp_neg_i8(x: i8) -> i8 {
    if x < 0 { 0 } else { x }
}
