// DEMO: Problem 2 (safe_mul) — BUGGY reference (Rust).
//
// We deliberately use a *different* (simpler) bug than the literal
// Python-style C++ version in safe_mul_buggy.cpp. The Python check
// `if c // a != b` when ported via Rust's `wrapping_div` hits
// crucible-llvm's poison-value path on `sdiv i32::MIN, -1` and
// crashes the simulator instead of producing a clean DISPROVED
// verdict. To keep the demo deterministic the Rust buggy version
// just omits the overflow check entirely:
//
//     a.wrapping_mul(b)   // BUG: no clamp-to-0 on overflow
//
// The Cryptol spec demands 0 on overflow, so SAW disproves
// equivalence for any (a, b) whose true product exceeds the i32
// range.
//
// (The literal Python-style buggy round-trip remains in the C++
// sibling file safe_mul_buggy.cpp for the parity demo.)
//
// Expected verdict: DISPROVED against `safe_mul_spec`.

fn safe_mul(a: i32, b: i32) -> i32 {
    a.wrapping_mul(b)
}
