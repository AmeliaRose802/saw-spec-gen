/// Dummy source — `gen-verify-rust` won't find a symbol named
/// `nonexistent_fn`, and `--spec-only-on-missing` should make it
/// soft-exit with a `result.json` containing `not_attempted`.
#[no_mangle]
pub fn some_other_fn(x: u32) -> u32 {
    x + 1
}
