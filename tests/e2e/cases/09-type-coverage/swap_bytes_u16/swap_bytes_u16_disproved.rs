// DEMO: BUGGY 16-bit byte swap — returns x unchanged.
// SAW disproves with any x whose two bytes differ.

fn swap_bytes_u16(x: u16) -> u16 {
    x   // BUG: missing the shift+or composition.
}
