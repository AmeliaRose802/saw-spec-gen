// DEMO: BUGGY three-argument signed minimum (Rust mirror).
// Third comparison flipped — SAW disproves with c > min(a, b).

fn min3_i32(a: i32, b: i32, c: i32) -> i32 {
    let mut m = a;
    if b < m { m = b; }
    if c > m { m = c; }   // BUG: comparison inverted.
    m
}
