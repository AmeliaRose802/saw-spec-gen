// DEMO: clamp-negative-to-zero, i64 (Rust mirror of clamp_neg_i64_verified.cpp).
// Exercises i64 parameter + return — largest signed integer width.

fn clamp_neg_i64(x: i64) -> i64 {
    if x < 0 { 0 } else { x }
}
