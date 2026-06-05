/// Returns 1 if status is 0 (Success), else 0.
/// The enum in the spec has 3 variants but the Cryptol model
/// only considers two (Success:0, Failure:1).
#[inline(never)]
pub fn is_success(status: u8) -> u8 {
    if status == 0 { 1 } else { 0 }
}
