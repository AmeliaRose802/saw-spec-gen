// DEMO: BUGGY power-of-two predicate — missing the `x != 0` guard.
// SAW disproves with x = 0.

fn is_power_of_two(x: u32) -> u32 {
    if (x & x.wrapping_sub(1)) == 0 { 1 } else { 0 }   // BUG: no x != 0 guard.
}
