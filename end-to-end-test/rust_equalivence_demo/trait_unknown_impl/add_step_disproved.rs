// DISPROVED case: same shape as add_step_verified.rs, but the function adds a
// hard-coded extra `+ 7` ON TOP OF the trait call.  The havoc spec for
// `Stepper::step` allows the trait call to return anything, so SAW
// must isolate the `+ 7` discrepancy: the actual return becomes
//   x + step(self) + 7
// while the Cryptol spec says
//   x + step_ret                    ( = x + step(self) )
// and SAW will produce a counterexample for any `step_ret`.

pub trait Stepper {
    fn step(&self) -> u32;
}

#[inline(never)]
pub fn add_step(x: u32, s: &dyn Stepper) -> u32 {
    x + s.step() + 7   // wrong!
}
