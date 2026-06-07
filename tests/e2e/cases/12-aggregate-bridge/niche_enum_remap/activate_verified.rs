// Rust implementation of activate.
// Returns u8 discriminant: 0=Success, 1=AlreadyActive.
// (The Rust enum has only 2 variants vs the spec's 3, so
// --variant-map restricts the reachable subset.)

#[inline(never)]
pub fn activate(key: u32) -> u8 {
    if key == 1 { 0 } else { 1 }
}
