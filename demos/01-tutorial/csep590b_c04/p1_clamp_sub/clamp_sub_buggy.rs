// DEMO: Problem 1 (clamp_sub) — BUGGY reference (Rust mirror of
// clamp_sub_buggy.cpp).
//
// Literal port of the buggy Python pseudocode. `wrapping_sub` is used
// so the subtraction has the same modular-i32 semantics as the C++
// version (and to suppress Rust's overflow panic in debug builds —
// we also pass -C overflow-checks=off via verify-rust.ps1).
//
// Expected verdict: DISPROVED against `clamp_sub_spec`.
// Counterexample: a = 0, b = i32::MIN.

fn clamp_sub(a: i32, b: i32) -> i32 {
    let r = a.wrapping_sub(b);
    if a > 0 && b < 0 && r < 0 {
        i32::MAX
    } else if a < 0 && b > 0 && r > 0 {
        i32::MIN
    } else {
        r
    }
}
