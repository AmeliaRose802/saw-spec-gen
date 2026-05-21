/*
DEMO: Sat — `&u32` (in) and `&mut u32` (out) cannot alias by the
borrow checker.

Mirrors demo/vtable_havoc_spec_demos/pointer_aliasing/add_one_sat.cpp,
which uses `_In_`, `_Out_`, and `__restrict` to tell the C++ tool the
two pointers don't alias. In Rust this is the default: you cannot
simultaneously hold `&u32` and `&mut u32` pointing at the same value
in safe code, so the `*output = *input + 1` pattern is naturally
sound.

The corresponding unsat case from the C++ demos (where the lack of
`_In_`/`_Out_`/`__restrict` makes the havoc model loose) does not
have a natural counterpart in safe Rust — the borrow checker would
reject any code that introduced the issue — so it is intentionally
omitted.
*/

fn transform(input: &u32, output: &mut u32) {
    *output = *input + 1;
}

fn add_one(x: u32) -> u32 {
    let mut result: u32 = 0;
    transform(&x, &mut result);
    result
}
