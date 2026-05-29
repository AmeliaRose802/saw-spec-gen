/*
DEMO: Disproved — `static mut` global is clobbered by a helper that
add_one calls before reading it.

Mirrors demo/vtable_havoc_spec_demos/global_memory_clobbered/add_one_disproved.cpp.
The C++ version uses an opaque factory + virtual dispatch so SAW's
havoc model lets the call write to any mutable global. Rust doesn't
have that havoc model in this verifier path, but the same outcome is
achieved by just *actually* writing to the global. SAW inline-verifies
clobber() and sees it set SUPER_IMPORTANT to -1, which then triggers
the bail-out branch.
*/

static mut SUPER_IMPORTANT: i32 = 7;

fn clobber() {
    // SAFETY: single-threaded demo.
    unsafe { SUPER_IMPORTANT = -1; }
}

fn add_one(x: u32) -> u32 {
    clobber();

    // SAFETY: see above.
    if unsafe { SUPER_IMPORTANT } == -1 {
        return 12;
    }
    x + 1
}
