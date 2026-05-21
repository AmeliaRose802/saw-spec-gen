// SAT — dynamic trait dispatch through `&dyn Stepper`.
//
// Unlike trait_static_dispatch/, the trait method is NOT monomorphized
// here: `&dyn Stepper` is a fat pointer (data_ptr, vtable_ptr), and the
// call `s.step()` is an indirect call through `vtable[step_slot]`.  In
// the LLVM IR you'll see the vtable as a private global table of
// function pointers and the call site loads through it.
//
// SAW's LLVM frontend resolves indirect vtable calls when the vtable
// is a concrete (non-symbolic) global and the dispatched-on `dyn`
// value's vtable can be tracked — which is the case here because we
// hand `add_with` a stack-allocated `UnitStepper` whose vtable rustc
// fills in statically at the coercion site.

trait Stepper {
    fn step(&self) -> u32;
}

struct UnitStepper;

impl Stepper for UnitStepper {
    fn step(&self) -> u32 { 1 }
}

// Note: takes `&dyn Stepper`, NOT a generic `T: Stepper`. The trait
// call is therefore an indirect call through the vtable.
#[inline(never)]
fn add_with(x: u32, s: &dyn Stepper) -> u32 {
    x + s.step()
}

pub fn add_one(x: u32) -> u32 {
    let s = UnitStepper;
    add_with(x, &s)
}
