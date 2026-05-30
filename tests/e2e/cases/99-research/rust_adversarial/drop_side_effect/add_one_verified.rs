/*
ADVERSARIAL: side effect hidden in a `Drop` impl.

`Guard::drop` runs at end of scope after the body's `return` value has
been computed — but Rust's evaluation order means the function's return
value is bound to a temporary BEFORE drops run, and reading
`SUPER_IMPORTANT` to compute the return happens BEFORE the drop fires.
So this version is actually `x + 1` (the drop's side effect is invisible
to the caller of *this* call). A naive port of the C++
`global_memory_clobbered` pattern into Rust via `Drop` doesn't reproduce
the bug — the verifier should still report VERIFIED.

This is informational: it shows where the C++ → Rust analogy breaks
down and confirms the verifier follows Rust's drop semantics correctly.
*/

static mut SUPER_IMPORTANT: i32 = 7;

struct Guard;

impl Drop for Guard {
    fn drop(&mut self) {
        // SAFETY: single-threaded demo.
        unsafe { SUPER_IMPORTANT = -1; }
    }
}

fn add_one(x: u32) -> u32 {
    let _g = Guard;

    // SAFETY: see above.
    let s = unsafe { SUPER_IMPORTANT };
    if s == -1 {
        return 12;
    }
    let r = x + 1;
    // _g drops here, AFTER r is computed. Side effect is invisible to caller.
    r
}
