// DEMO: Three-argument signed minimum (Rust mirror of min3_i32_verified.cpp).
//
// Covers multi-arg + signed (i32) lowering, neither of which the
// existing single-arg `add_one` Rust suite exercises.

fn min3_i32(a: i32, b: i32, c: i32) -> i32 {
    let mut m = a;
    if b < m { m = b; }
    if c < m { m = c; }
    m
}
