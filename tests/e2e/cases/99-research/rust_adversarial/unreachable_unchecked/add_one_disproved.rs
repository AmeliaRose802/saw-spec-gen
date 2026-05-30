/*
ADVERSARIAL: `unreachable_unchecked` makes the verifier "prove" garbage.

At the source level this function is INCORRECT — for any nonzero x at
runtime it triggers undefined behavior. But UB in LLVM is "assume this
path is impossible", which propagates as an assumption to SAW. The
solver therefore restricts the symbolic input x to 0, and at x = 0 the
return value is 0 + 1 = 1 = add_one_spec 0.

So SAW reports VERIFIED — a false sense of security, because the binary
is actually unsound on nearly every input.

This is the Rust analog of the C++ `__builtin_unreachable()` /
`__assume(false)` hole. A tool that wants to be sound on Rust source
must either (a) refuse to look through unreachable_unchecked, or (b)
treat reaching it as a verification failure.
*/

fn add_one(x: u32) -> u32 {
    if x != 0 {
        // SAFETY: this is a deliberate lie — the function will be called
        // with nonzero x at runtime, which is UB. We do this to expose
        // a verifier hole.
        unsafe { std::hint::unreachable_unchecked() }
    }
    x + 1
}
