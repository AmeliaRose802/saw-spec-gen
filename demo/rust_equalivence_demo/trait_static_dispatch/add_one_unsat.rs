// UNSAT — static trait dispatch where the impl returns the wrong value.
//
// Same shape as add_one_sat.rs: a `Stepper` trait defined in this crate,
// generics for static dispatch.  But the `BadStepper` impl returns 7
// instead of 1, so the post-call value is `x + 7` and SAW finds a
// counterexample against the Cryptol spec `add_one_spec x = x + 1`.

trait Stepper {
    fn step(&self) -> u32;
}

struct BadStepper;

impl Stepper for BadStepper {
    fn step(&self) -> u32 { 7 }   // wrong!
}

#[inline(never)]
fn add_with<T: Stepper>(x: u32, s: &T) -> u32 {
    x + s.step()
}

pub fn add_one(x: u32) -> u32 {
    add_with(x, &BadStepper)
}
