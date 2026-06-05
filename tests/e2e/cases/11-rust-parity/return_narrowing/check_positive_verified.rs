/// Returns 0 (Ok) if x > 0, else 1 (Err).
/// Cryptol spec returns Bit; the variant-map bridges Bit ↔ u8.
#[inline(never)]
pub fn check_positive(x: i32) -> u8 {
    if x > 0 { 0 } else { 1 }
}
