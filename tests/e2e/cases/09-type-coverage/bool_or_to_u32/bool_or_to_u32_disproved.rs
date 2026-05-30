// DEMO: BUGGY bool-or-to-u32 — uses AND instead of OR.
// SAW disproves with a=true, b=false.

fn bool_or_to_u32(a: bool, b: bool) -> u32 {
    if a && b { 1 } else { 0 }   // BUG: was a || b.
}
