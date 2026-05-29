// DEMO: Problem 5 (isqrt_newton) — reference (Rust mirror of
// isqrt_verified.cpp).
//
// Bounded-loop port of the homework's Newton's-method isqrt. Input
// is clamped to [1, 255] (sentinel 0 outside) so SAW can fully
// unroll the fixed 10-iteration loop. x stays >= 1 by induction so
// the LLVM `udiv n, x` is well-defined on every reachable path.
//
// Expected verdict: VERIFIED against `isqrt_spec`.

fn isqrt(n: u32) -> u32 {
    if n < 1 || n > 255 {
        return 0;
    }
    let mut x: u32 = n;
    let mut y: u32 = (x.wrapping_add(1)) / 2;
    let mut i: u32 = 0;
    while i < 10 {
        if y < x {
            x = y;
            y = (x.wrapping_add(n / x)) / 2;
        }
        i = i.wrapping_add(1);
    }
    x
}
