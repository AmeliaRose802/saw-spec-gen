/// Simple async function for E2E testing of automatic async detection
/// in gen-verify-rust.
///
/// rustc lowers this to two symbols:
///   _RNvCs…<crate>7add_one        ← constructor shim (builds the future)
///   _RNCNvCs…<crate>7add_one0B3_  ← resume function  (state machine body)
///
/// gen-verify-rust must auto-detect the _RNC symbol and emit a mir_verify
/// script targeting the resume function without any --async flag.
#[inline(never)]
pub async fn add_one(x: u32) -> u32 {
    x + 1
}
