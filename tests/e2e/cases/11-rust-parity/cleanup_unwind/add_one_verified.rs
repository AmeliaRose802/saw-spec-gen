/*
TEST: Cleanup funclets produced by `-C panic=unwind` on a function that
has a `Drop` guard AND calls a non-inlineable helper.

With `panic=unwind`, rustc emits the call to `helper` as an `invoke`
(since `helper` might panic — rustc can't prove otherwise at codegen
time) with a `cleanuppad` / `cleanupret` landing pad to run the guard's
destructor. This produces the exact Win64 SEH operand-bundle IR that
SAW's parser rejects (`FUNC_CODE_OPERAND_BUNDLE`).

The exception-lower pass must:
  1. Lower the cleanuppad/cleanupret to plain branches.
  2. Demote the invoke to a call (or keep the invoke with a normal
     unwind target).
  3. Leave the functional semantics unchanged: add_one(x) == x + 1.

Expected: VERIFIED (helper never actually panics, and even if it did,
the cleanup side-effect on COUNTER is invisible to the return value).
*/

static mut COUNTER: u32 = 0;

struct Guard;

impl Drop for Guard {
    #[inline(never)]
    fn drop(&mut self) {
        // Side effect in drop — invisible to add_one's return value,
        // but forces rustc to emit a real cleanup funclet.
        unsafe { COUNTER += 1; }
    }
}

#[inline(never)]
fn helper(x: u32) -> u32 {
    x + 1
}

fn add_one(x: u32) -> u32 {
    let _guard = Guard;
    // With panic=unwind, this is an `invoke` because _guard has a Drop.
    // The cleanup landing pad runs Guard::drop if helper panics.
    helper(x)
}
