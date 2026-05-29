/*
DEMO: Disproved — validator takes `&mut u32` and rewrites the value.

Mirrors demos/02-havoc-coverage/input_param_modified/add_one_disproved.cpp,
where the validator's pointer parameter is non-const. In Rust the
equivalent is `&mut u32`. After the call, `input` has been clobbered to
999, so the function returns 1000 — not x + 1. SAW finds a
counterexample on any x.
*/

fn validate(val: &mut u32) {
    *val = 999;
}

fn add_one(x: u32) -> u32 {
    let mut input = x;
    validate(&mut input);
    input + 1
}
