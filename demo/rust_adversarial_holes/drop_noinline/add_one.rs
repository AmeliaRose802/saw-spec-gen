/*
TEST: A `Drop` impl marked `#[inline(never)]` so the optimizer CANNOT
fold it into add_one. The drop is observable: it writes self.x to a
static, then add_one's return path inspects that static via a second
function call (also #[inline(never)]) that returns x + 1 when the
static is 42 and the bail-out value otherwise. So:

  - Correct semantics: drop fires AFTER `read_super` is called, so
    `read_super` sees 0, and add_one returns x + 1. VERIFIED expected.

This forces SAW to actually deal with:
  (a) a non-ZST struct with a `Drop` impl
  (b) an indirect call through `drop_in_place::<Guard>`
  (c) the drop glue calling the user's Drop::drop body
*/

static mut SUPER: u32 = 0;

struct Guard { x: u32 }

impl Drop for Guard {
    #[inline(never)]
    fn drop(&mut self) {
        // SAFETY: single-threaded demo.
        unsafe { SUPER = self.x; }
    }
}

#[inline(never)]
fn read_super() -> u32 {
    // SAFETY: see above.
    unsafe { SUPER }
}

fn add_one(x: u32) -> u32 {
    let _g = Guard { x: 42 };
    // Reads SUPER BEFORE _g drops at end of scope, so this is 0.
    let s = read_super();
    if s == 42 {
        return 7;
    }
    x + 1
    // _g drops here. SUPER becomes 42, but we've already returned.
}
