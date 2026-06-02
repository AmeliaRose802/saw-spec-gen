/*
ADVERSARIAL: Two private functions named `add_one` in different modules.

Both end up in the bitcode (we compile with -C link-dead-code=yes). The
verifier's symbol-resolution regex matches mangled names ending with
`${funcLen}${Function}` — both `safe_impl::add_one` and `buggy_impl::add_one`
qualify. The current code picks "the first" and prints a warning, but a
user scanning the output for VERIFIED/DISPROVED may miss it.

If the verifier picks `safe_impl::add_one` (which returns x + 1), it
reports VERIFIED even though the *real* entry point delegates to the
buggy version.

NOTE: there is no `add_one` callable from outside this file at the source
level — the public entry point is named `entry`, and it calls the buggy
implementation. But because we link dead code, both private fns survive
into the bitcode for the regex to find.
*/

mod safe_impl {
    pub fn add_one(x: u32) -> u32 {
        x + 1
    }
}

mod buggy_impl {
    pub fn add_one(x: u32) -> u32 {
        x + 7
    }
}

pub fn entry(x: u32) -> u32 {
    buggy_impl::add_one(x)
}

// And a function actually named `add_one` at the top level, which is
// what the user *thinks* they're verifying. It also wraps the bug.
fn add_one(x: u32) -> u32 {
    buggy_impl::add_one(x)
}
