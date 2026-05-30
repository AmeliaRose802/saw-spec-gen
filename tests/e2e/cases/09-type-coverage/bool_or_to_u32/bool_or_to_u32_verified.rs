// DEMO: bool-or-to-u32 (Rust mirror of bool_or_to_u32_verified.cpp).
// Exercises `bool` parameter (i1) lowering — absent from earlier suites.

fn bool_or_to_u32(a: bool, b: bool) -> u32 {
    if a || b { 1 } else { 0 }
}
