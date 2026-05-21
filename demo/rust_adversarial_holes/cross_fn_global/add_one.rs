/*
TEST: read a `static mut` through a separate function (not inlined).
No Drop, no Guard — just isolate whether crossing a function boundary
to read a global breaks SAW's memory model.
*/

static mut SUPER: u32 = 0;

#[inline(never)]
fn read_super() -> u32 {
    // SAFETY: single-threaded demo.
    unsafe { SUPER }
}

fn add_one(x: u32) -> u32 {
    let s = read_super();
    if s == 42 {
        return 7;
    }
    x + 1
}
