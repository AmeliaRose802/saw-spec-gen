/*
DEMO: Disproved — method takes `&mut self` and rewrites the struct's field.

Mirrors demo/vtable_havoc_spec_demos/class_member_clobbered/add_one_disproved.cpp.
The C++ version uses a non-const virtual method that clobbers `bias_`.
Here we do the equivalent in Rust: `prepare` takes `&mut self` and
overwrites `self.bias`. The function then returns `x + 1 + bias`,
which is `x + 43` — not `x + 1`. SAW finds a counterexample on any x.
*/

struct Counter {
    bias: u32,
}

impl Counter {
    fn prepare(&mut self, _x: u32) {
        self.bias = 42;
    }
}

fn add_one(x: u32) -> u32 {
    let mut counter = Counter { bias: 0 };
    counter.prepare(x);
    // counter.bias is now 42, so this returns x + 43.
    x + 1 + counter.bias
}
