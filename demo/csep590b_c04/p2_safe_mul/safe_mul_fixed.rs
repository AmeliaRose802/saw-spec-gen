// DEMO: Problem 2 (safe_mul) — FIXED reference (Rust mirror of
// safe_mul_fixed.cpp).
//
// Multiply in i64, check the i32 range, return 0 on overflow.
//
// Expected verdict: VERIFIED against `safe_mul_spec`.

fn safe_mul(a: i32, b: i32) -> i32 {
    let c: i64 = (a as i64) * (b as i64);
    if c > i32::MAX as i64 || c < i32::MIN as i64 {
        0
    } else {
        c as i32
    }
}
