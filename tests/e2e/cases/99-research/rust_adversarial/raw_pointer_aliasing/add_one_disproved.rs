/*
ADVERSARIAL: raw `*mut u32` aliasing.

Rust's `&mut T` carries a `noalias` LLVM attribute, but `*mut T` does
not. This function writes through `q` and then reads through `p`,
where the caller has aliased them. At the LLVM level this is a
*defined* sequence of memory operations (no UB), so SAW should see
that `*p` is now 42 and the function returns x + 43.

The catch: if a verifier silently lifts `noalias`-style assumptions
from the source-level type when looking at raw pointers, it would
miss this clobber.
*/

fn transform(p: *mut u32, q: *mut u32) {
    // SAFETY: caller guarantees both pointers are valid; aliasing is
    // intentional for this demo.
    unsafe { *q = 42; }
    let _read = unsafe { *p };
}

fn add_one(x: u32) -> u32 {
    let mut bias: u32 = 0;
    let p: *mut u32 = &mut bias as *mut u32;
    // Pass the same pointer twice — raw pointer aliasing is allowed.
    transform(p, p);
    // If `transform` wrote to *q (== *p), bias is now 42.
    x + 1 + bias
}
