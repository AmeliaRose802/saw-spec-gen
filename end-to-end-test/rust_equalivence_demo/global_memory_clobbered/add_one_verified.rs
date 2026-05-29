/*
DEMO: Verified — SUPER_IMPORTANT is a `const`, so nothing can clobber it.

Mirrors end-to-end-test/vtable_havoc_spec_demos/global_memory_clobbered/add_one_verified.cpp.
The C++ version makes the global `const` so the havoc model can't
write to it. Rust's `const` is even stronger (compile-time constant,
no addressable storage), so the bail-out branch is trivially dead.
*/

const SUPER_IMPORTANT: i32 = 7;

fn log_message(_msg: &str) {
    // Visible to SAW: provably does not touch SUPER_IMPORTANT.
}

fn add_one(x: u32) -> u32 {
    log_message("Adding one to x");

    if SUPER_IMPORTANT == -1 {
        return 12;
    }
    x + 1
}
