//! Cryptol/LLVM type bridge helpers shared across SAW-spec emitters.
//!
//! Both the C++ generator ([`super::verify_script_steps`]) and the
//! Rust generator ([`crate::gen_verify_rust`]) must agree on how each
//! LLVM-side parameter is presented to a Cryptol spec. Centralizing
//! the conversion in one module means a `*_spec.cry` file authored for
//! one runner type-checks on the other without surprise.
//!
//! The single non-trivial case today is `bool` ↔ `Bit`: C++ `bool` and
//! Rust `bool` both lower to LLVM `i1`, which SAW exposes as the
//! Cryptol sequence type `[1]`. Cryptol's primitive boolean type is
//! `Bit` though, and idiomatic specs declare boolean parameters as
//! `Bit -> …` so `\/`, `/\`, `~`, etc. work directly. The `(name ! 0)`
//! wrap extracts bit 0 from the `[1]` and yields a `Bit`.
//!
//! Add new bridges here when a future type (e.g. `f32`/`f64`) needs
//! similar adjustment — never re-implement the convention in a single
//! runner only.

use crate::constraints::TypeInfo;

/// Whether `ty` is a `bool` (or pointer/reference to `bool`) as seen
/// by the front-end parsers. Both lower to LLVM `i1` at the call
/// boundary and therefore need Bit/`[1]` bridging on the Cryptol side.
pub fn is_bool_like(ty: &TypeInfo) -> bool {
    match ty {
        TypeInfo::Bool => true,
        TypeInfo::Pointer(inner) => matches!(inner.as_ref(), TypeInfo::Bool),
        _ => false,
    }
}

/// Bridge a Cryptol-side argument expression to match the LLVM-side
/// representation expected by the C++/Rust call. C++/Rust `bool`
/// parameters become `(name ! 0)` so the Cryptol callsite sees a
/// `Bit`. Other types pass through unchanged.
pub fn cryptol_arg_for(name: &str, ty: &TypeInfo) -> String {
    if is_bool_like(ty) {
        format!("({name} ! 0)")
    } else {
        name.to_string()
    }
}

/// Bridge a Cryptol-side call expression to match the LLVM-side return
/// representation produced by the C++/Rust function. A `Bit`-returning
/// Cryptol spec is wrapped as `[…] : [1]` so it lines up with the
/// LLVM `i1` returned by a C++/Rust `bool`. Other types pass through.
pub fn cryptol_return_for(call: &str, ty: &TypeInfo) -> String {
    if is_bool_like(ty) {
        format!("[{call}] : [1]")
    } else {
        call.to_string()
    }
}

/// Format a counterexample literal value as a typed Cryptol expression.
/// For `bits == 1` inputs, bridges via `((v : [1]) ! 0)` so the call
/// reaches a `Bit`-typed parameter. Returns plain `(v : [bits])` for
/// every other width.
///
/// Used by counterexample evaluators that need to call the same
/// Cryptol spec on a concrete witness produced by SAW.
pub fn cryptol_literal_for_bits(value: u64, bits: u32) -> String {
    if bits == 1 {
        format!("(({value} : [1]) ! 0)")
    } else {
        format!("({value} : [{bits}])")
    }
}
