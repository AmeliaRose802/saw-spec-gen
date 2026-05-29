/*
DEMO: Disproved — concrete struct method that mutates a `static mut` global.

Mirrors demos/02-havoc-coverage/concrete_type_safe/add_one_disproved.cpp.
The original C++ uses a `SusLog` whose log() method clobbers
`super_important`. Here we do the same thing in Rust: a concrete
struct's method writes -1 to a static mut global, which triggers the
bail-out branch in add_one and returns 12 instead of x + 1.
*/

static mut SUPER_IMPORTANT: i32 = 7;

struct SusLog;

impl SusLog {
    fn log(&self, _msg: &str) {
        // SAFETY: single-threaded demo; demonstrates mutating a global.
        unsafe { SUPER_IMPORTANT = -1; }
    }
}

fn add_one(x: u32) -> u32 {
    let logger = SusLog;
    logger.log("Adding one to x");

    // SAFETY: same as above.
    if unsafe { SUPER_IMPORTANT } == -1 {
        return 12;
    }
    x + 1
}
