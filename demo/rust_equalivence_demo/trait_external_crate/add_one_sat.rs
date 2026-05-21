// SAT — trait call where the trait is NOT defined in this crate.
//
// `core::ops::Add` lives in the `core` crate, which is a different
// compilation unit than the user crate we're verifying. We dispatch
// to it explicitly with UFCS so the cross-crate trait call is plainly
// visible: `<u32 as Add>::add(x, 1)`.
//
// `core::default::Default` (also from `core`) provides the "1" via a
// user-crate type `OneU32` whose `Default::default()` returns 1.  This
// gives us TWO different cross-crate trait calls in one tiny function,
// one of which dispatches on a type defined here.

use core::ops::Add;
use core::default::Default;

struct OneU32(u32);

impl Default for OneU32 {
    fn default() -> Self { OneU32(1) }
}

pub fn add_one(x: u32) -> u32 {
    let step: OneU32 = Default::default();   // cross-crate trait, user-crate impl
    <u32 as Add>::add(x, step.0)             // cross-crate trait on primitive u32
}
