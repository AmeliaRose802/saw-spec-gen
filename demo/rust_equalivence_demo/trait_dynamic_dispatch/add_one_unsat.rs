// UNSAT — dynamic trait dispatch through `&dyn Stepper` where the
// impl returns the wrong value.  The trait call goes through a vtable
// (see add_one_sat.rs for the discussion), but the BadStepper vtable
// points at a `step()` that returns 7, so SAW should still resolve the
// indirect call (because the dyn coercion site is statically `&BadStepper`)
// and find that the post-call value is `x + 7`, contradicting the spec.

trait Stepper {
    fn step(&self) -> u32;
}

struct BadStepper;

impl Stepper for BadStepper {
    fn step(&self) -> u32 { 7 }   // wrong!
}

#[inline(never)]
fn add_with(x: u32, s: &dyn Stepper) -> u32 {
    x + s.step()
}

pub fn add_one(x: u32) -> u32 {
    let s = BadStepper;
    add_with(x, &s)
}
