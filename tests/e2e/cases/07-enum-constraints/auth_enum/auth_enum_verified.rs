// DEMO: Rust mirror of auth_enum_verified.cpp.
//
// `#[repr(u8)] enum AuthResult { Ok=0, Pending=1, Failed=2 }` lowers
// to an `i8` on the LLVM IR boundary. A safe `match` over all declared
// variants would normally be exhaustive, but to expose the same
// problem the C++ side has — symbolic enum tags out of range — we
// drop into `unreachable_unchecked` on the impossible-at-source-level
// `_` arm.
//
// Expected verdict: VERIFIED, once saw-spec-gen learns to emit an
// enum-range precondition from the mir-json ADT info
// (`adts[*].variants` + `discriminant_bits`). Until then verify-rust.ps1
// generates a bare `x0 <- llvm_fresh_var "x0" (llvm_int 8)` with no
// constraint and SAW returns DISPROVED with `x0 = 3`.

#[repr(u8)]
pub enum AuthResult {
    Ok      = 0,
    Pending = 1,
    Failed  = 2,
}

fn classify(r: AuthResult) -> u32 {
    match r {
        AuthResult::Ok      => 0,
        AuthResult::Pending => 1,
        AuthResult::Failed  => 2,
    }
}
