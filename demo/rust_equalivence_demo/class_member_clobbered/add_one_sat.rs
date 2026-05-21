/*
DEMO: Sat — method takes `&self`, so it cannot mutate the struct's field.

Mirrors demo/vtable_havoc_spec_demos/class_member_clobbered/add_one_sat.cpp.
The C++ version makes the virtual method `const`, which is how it
promises not to write through `this`. In Rust this is enforced at the
type level: `&self` cannot mutate `self`. The borrow checker is doing
the same job the `const` qualifier does in C++.
*/

struct Counter {
    bias: u32,
}

impl Counter {
    fn prepare(&self, _x: u32) {
        // `&self` — cannot assign to self.bias here even if we wanted to.
    }
}

fn add_one(x: u32) -> u32 {
    let counter = Counter { bias: 0 };
    counter.prepare(x);
    // counter.bias is provably still 0, so this is x + 1.
    x + 1 + counter.bias
}
