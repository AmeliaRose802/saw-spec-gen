// SAT case: `add_step` takes `&dyn Stepper` from an UNKNOWN caller. The
// vtable pointer is a symbolic function input (a true opaque trait
// object). To verify this with SAW we have to model `Stepper::step` as
// an uninterpreted operation — exactly the case the C++ side handles
// via `vtable_stubs.bc` + `interface_overrides.saw`.
//
// The technique used by the runner script `run_unknown_impl.ps1`:
//   1. Compile this .rs to .bc.
//   2. Hand-write `trait_stubs.ll` with:
//        * `@__stub_Stepper_step(ptr) -> i32`  (body irrelevant)
//        * `@__stubvtable_Stepper` constant pointing at the stub.
//   3. llvm-link the two into one module.
//   4. SAW script: `llvm_unsafe_assume_spec` on the stub (havoc spec
//      that returns a fresh symbolic value `step_ret`), then verify
//      `add_step` against a Cryptol spec parameterised by `step_ret`.
//
// This proves: `add_step(x, &dyn) == x + step(self)` for ANY impl of
// `Stepper::step` — i.e. functional correctness modulo the trait
// method, exactly like a C++ vtable havoc spec.

pub trait Stepper {
    fn step(&self) -> u32;
}

#[inline(never)]
pub fn add_step(x: u32, s: &dyn Stepper) -> u32 {
    x + s.step()
}
