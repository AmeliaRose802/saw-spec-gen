/*
DEMO: Sat — validator takes `&u32`, so it cannot write through the
pointer.

Mirrors demo/vtable_havoc_spec_demos/input_param_modified/add_one_sat.cpp,
which marks the validator parameter as `const uint32_t*`. In Rust the
equivalent is `&u32` — the borrow checker statically forbids the
callee from mutating the value. SAW sees that `input` is unchanged
across the call and concludes `input + 1 == x + 1`.
*/

fn validate(_val: &u32) {
    // Read-only borrow — cannot assign through `_val`.
}

fn add_one(x: u32) -> u32 {
    let input = x;
    validate(&input);
    input + 1
}
