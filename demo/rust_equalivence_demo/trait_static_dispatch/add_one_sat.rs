// SAT — static trait dispatch (monomorphization).
//
// `Stepper` is a trait defined in THIS crate. `add_one` is generic over
// `T: Stepper` so each instantiation gets its own monomorphized copy
// in the bitcode. The trait call `stepper.step()` resolves to a direct
// call to `<UnitStepper as Stepper>::step` at compile time, so SAW sees
// a regular static function call (no vtable involved).
//
// We instantiate at u32 and call through the trait method.  The
// `step()` impl returns 1, so the result is x + 1 — matching the spec.

trait Stepper {
    fn step(&self) -> u32;
}

struct UnitStepper;

impl Stepper for UnitStepper {
    fn step(&self) -> u32 { 1 }
}

#[inline(never)]
fn add_with<T: Stepper>(x: u32, s: &T) -> u32 {
    x + s.step()
}

pub fn add_one(x: u32) -> u32 {
    add_with(x, &UnitStepper)
}
