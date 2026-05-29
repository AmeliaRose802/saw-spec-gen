// Saturating add — Rust port of sat_add.cpp.
//
// `wrapping_add` does the same modular arithmetic the C++ `+` does on
// `uint32_t`, and we use the same `sum < a` overflow check.

fn sat_add(a: u32, b: u32) -> u32 {
    let sum = a.wrapping_add(b);
    if sum < a {
        u32::MAX
    } else {
        sum
    }
}
