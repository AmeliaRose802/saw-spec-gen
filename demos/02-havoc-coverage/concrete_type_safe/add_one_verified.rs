/*
DEMO: Verified — concrete struct method, no global state touched.

Mirrors demos/02-havoc-coverage/concrete_type_safe/add_one_verified.cpp.
In the C++ version this shows that calling on a concrete type lets SAW
verify the actual implementation (rather than the havoc model of an
opaque virtual call). In Rust there's no havoc model in this verifier
path — every function body SAW can see gets inlined. The demo here
keeps the same shape (concrete struct with a method) and the same
spec (add_one = x + 1).
*/

const SUPER_IMPORTANT: i32 = 7;

struct OkLog;

impl OkLog {
    fn log(&self, _msg: &str) {
        // No-op safe implementation — does not touch SUPER_IMPORTANT.
    }
}

fn add_one(x: u32) -> u32 {
    let logger = OkLog;
    logger.log("Adding one to x");

    // SUPER_IMPORTANT is const, and OkLog::log doesn't write anywhere
    // SAW can see, so this branch is provably unreachable.
    if SUPER_IMPORTANT == -1 {
        return 12;
    }
    x + 1
}
