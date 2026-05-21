/*
ADVERSARIAL: `Box::new` exercises `__rust_alloc`.

The allocator's symbols aren't defined in a freestanding -C panic=abort
bitcode of a single source file. SAW will likely fail with an
unresolved external symbol — i.e., the demo doesn't break soundness but
it does demonstrate a class of input the current pipeline can't handle
without linking in core / alloc.

If we ever silently stubbed __rust_alloc, the result of `*b` would be
unconstrained and SAW could either report a spurious counterexample or
(worse) prove anything depending on the stub.
*/

fn add_one(x: u32) -> u32 {
    let b: Box<u32> = Box::new(x + 1);
    *b
}
