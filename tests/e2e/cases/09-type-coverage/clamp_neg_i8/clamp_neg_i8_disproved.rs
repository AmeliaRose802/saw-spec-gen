// DEMO: BUGGY clamp-negative-to-zero, i8 — comparison inverted.
// SAW disproves with any positive x.

fn clamp_neg_i8(x: i8) -> i8 {
    if x > 0 { 0 } else { x }   // BUG: was x < 0.
}
