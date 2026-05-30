// DISPROVED — same cross-crate trait shape as add_one_verified.rs, but the
// user-crate impl of the foreign trait `Default` returns 7 instead of
// 1, so the call `<u32 as Add>::add(x, step.0)` produces `x + 7` and
// the Cryptol spec `x + 1` is violated.

use core::ops::Add;
use core::default::Default;

struct OneU32(u32);

impl Default for OneU32 {
    fn default() -> Self { OneU32(7) }   // wrong constant
}

pub fn add_one(x: u32) -> u32 {
    let step: OneU32 = Default::default();
    <u32 as Add>::add(x, step.0)
}
