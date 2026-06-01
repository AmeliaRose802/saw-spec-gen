// DEMO: BUGGY clamp-negative-to-zero, i64 — comparison inverted.

fn clamp_neg_i64(x: i64) -> i64 {
    if x > 0 { 0 } else { x }   // BUG: was x < 0.
}
