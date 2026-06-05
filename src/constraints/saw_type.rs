//! Mapping from [`TypeInfo`] to SAW spec type strings.

use super::types::TypeInfo;

pub fn type_to_saw(ty: &TypeInfo) -> String {
    match ty {
        TypeInfo::Bool => "llvm_int 1".into(),
        TypeInfo::SignedInt(bits) | TypeInfo::UnsignedInt(bits) => format!("llvm_int {bits}"),
        TypeInfo::Float(32) => "llvm_float".into(),
        TypeInfo::Float(64) => "llvm_double".into(),
        TypeInfo::Float(bits) => format!("llvm_int {bits}"),
        TypeInfo::ByteArray(n) => format!("llvm_array {n} (llvm_int 8)"),
        TypeInfo::Pointer(inner) => type_to_saw(inner),
        TypeInfo::Struct {
            size_bytes: Some(n),
            ..
        } => format!("llvm_array {n} (llvm_int 8)"),
        TypeInfo::Struct { name, .. } => {
            // Clang emits struct types as "struct.Name" in LLVM IR
            if name.starts_with("struct.")
                || name.starts_with("class.")
                || name.starts_with("union.")
            {
                format!("llvm_alias \"{name}\"")
            } else {
                format!("llvm_alias \"struct.{name}\"")
            }
        }
        TypeInfo::Enum {
            discriminant_bits, ..
        } => format!("llvm_int {discriminant_bits}"),
        TypeInfo::Option(inner) => format!("// Option<{}>", type_to_saw(inner)),
        TypeInfo::Result(ok, err) => {
            format!("// Result<{}, {}>", type_to_saw(ok), type_to_saw(err))
        }
        TypeInfo::Opaque { name, size_bytes } => {
            if *size_bytes > 0 {
                format!("llvm_array {size_bytes} (llvm_int 8)")
            } else if name == "Self" || name == "Unknown" {
                // Unresolved this pointer for abstract class — use pointer-sized
                "llvm_int 64".into()
            } else if let Some(bits) = std_integer_typedef_bits(name) {
                // Recognize fixed-width integer typedefs that the C++ AST
                // parser left as `std::int64_t` / `std::size_t` / etc.
                // Emitting `llvm_alias "std::int64_t"` would never
                // resolve at SAW load time because these are typedefs,
                // not LLVM struct types.
                format!("llvm_int {bits}")
            } else {
                format!("llvm_alias \"{name}\"")
            }
        }
        TypeInfo::Void => "// void".into(),
    }
}

/// Lower a pointee `TypeInfo` to a SAW type string that is safe to
/// embed in an `llvm_alloc (…)` / `llvm_fresh_var "…" (…)` slot.
///
/// Unlike [`type_to_saw`], this never returns a string beginning with
/// `//`. The plain `type_to_saw` produces sentinels like `"// void"`
/// or `"// Option<…>"` for unrepresentable cases, but those parse as
/// SAWScript line comments — embedding them in an expression position
/// would silently truncate the surrounding `llvm_alloc (` and cause a
/// downstream syntax error. For pointee positions we substitute a
/// single opaque byte (`llvm_int 8`) and log a warning so the
/// generated spec still parses while flagging the gap.
pub fn pointee_saw_type(ty: &TypeInfo) -> String {
    // void* is a normal opaque-byte pointer — silently lower to i8.
    if matches!(ty, TypeInfo::Void) {
        return "llvm_int 8".into();
    }
    let lowered = type_to_saw(ty);
    if lowered.starts_with("//") {
        eprintln!(
            "warning: pointee type {ty:?} has no SAW lowering (got `{lowered}`); \
             substituting `llvm_int 8` so the spec parses. Use --precond or \
             extend constraints::saw_type to override."
        );
        return "llvm_int 8".into();
    }
    lowered
}

/// Recognize C / C++ fixed-width and pointer-sized integer typedefs
/// that should lower directly to `llvm_int N` instead of being left as
/// an opaque `llvm_alias`. Covers the unqualified `<stdint.h>` names
/// (`int64_t`), their `std::`-qualified C++ counterparts
/// (`std::int64_t`), `size_t` / `ptrdiff_t` / `intptr_t` / `uintptr_t`,
/// and the corresponding `std::` aliases.
///
/// The unqualified primitive arms are handled directly in
/// `parsers::clang_ast::cpp_types::parse_cpp_type`; this helper exists
/// for the case where the AST already produced a `TypeInfo::Opaque`
/// (typically because the typedef appeared inside a more complex
/// qualType that wasn't string-matched) and we still want to recover
/// the integer width at SAW emission time.
fn std_integer_typedef_bits(name: &str) -> Option<u32> {
    let stripped = name.strip_prefix("std::").unwrap_or(name);
    match stripped {
        "int8_t" | "uint8_t" => Some(8),
        "int16_t" | "uint16_t" => Some(16),
        "int32_t" | "uint32_t" => Some(32),
        "int64_t" | "uint64_t" => Some(64),
        // Pointer-sized integer typedefs (x86_64 ABI).
        "size_t" | "ptrdiff_t" | "intptr_t" | "uintptr_t" | "ssize_t" => Some(64),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::super::types::EnumVariant;
    use super::*;

    #[test]
    fn test_type_to_saw_scalars() {
        assert_eq!(type_to_saw(&TypeInfo::Bool), "llvm_int 1");
        assert_eq!(type_to_saw(&TypeInfo::SignedInt(32)), "llvm_int 32");
        assert_eq!(type_to_saw(&TypeInfo::UnsignedInt(64)), "llvm_int 64");
    }

    #[test]
    fn test_type_to_saw_byte_array() {
        assert_eq!(
            type_to_saw(&TypeInfo::ByteArray(16)),
            "llvm_array 16 (llvm_int 8)"
        );
    }

    #[test]
    fn test_type_to_saw_struct_with_size() {
        let ty = TypeInfo::Struct {
            name: "MyStruct".into(),
            size_bytes: Some(24),
            fields: vec![],
        };
        assert_eq!(type_to_saw(&ty), "llvm_array 24 (llvm_int 8)");
    }

    #[test]
    fn test_type_to_saw_struct_named() {
        let ty = TypeInfo::Struct {
            name: "MyStruct".into(),
            size_bytes: None,
            fields: vec![("x".into(), TypeInfo::SignedInt(32))],
        };
        assert_eq!(type_to_saw(&ty), "llvm_alias \"struct.MyStruct\"");
    }

    #[test]
    fn test_type_to_saw_enum() {
        let ty = TypeInfo::Enum {
            name: "Status".into(),
            variants: vec![
                EnumVariant::new("Ok", 0),
                EnumVariant::new("Err", 1),
                EnumVariant::new("Pending", 2),
            ],
            discriminant_bits: 64,
        };
        assert_eq!(type_to_saw(&ty), "llvm_int 64");
    }

    #[test]
    fn test_type_to_saw_option() {
        let ty = TypeInfo::Option(Box::new(TypeInfo::SignedInt(32)));
        assert_eq!(type_to_saw(&ty), "// Option<llvm_int 32>");
    }

    #[test]
    fn test_type_to_saw_result() {
        let ty = TypeInfo::Result(
            Box::new(TypeInfo::UnsignedInt(8)),
            Box::new(TypeInfo::SignedInt(32)),
        );
        assert_eq!(type_to_saw(&ty), "// Result<llvm_int 8, llvm_int 32>");
    }

    #[test]
    fn test_type_to_saw_opaque_with_size() {
        let ty = TypeInfo::Opaque {
            name: "Unknown".into(),
            size_bytes: 32,
        };
        assert_eq!(type_to_saw(&ty), "llvm_array 32 (llvm_int 8)");
    }

    #[test]
    fn test_type_to_saw_opaque_no_size() {
        let ty = TypeInfo::Opaque {
            name: "std::string".into(),
            size_bytes: 0,
        };
        assert_eq!(type_to_saw(&ty), "llvm_alias \"std::string\"");
    }

    #[test]
    fn test_type_to_saw_pointer() {
        let ty = TypeInfo::Pointer(Box::new(TypeInfo::SignedInt(32)));
        assert_eq!(type_to_saw(&ty), "llvm_int 32");
    }

    #[test]
    fn pointee_saw_type_rewrites_void_to_byte() {
        // Bug #11: `void*` parameters used to drop `"// void"` into
        // `llvm_alloc (\u2026)` slots, where SAWScript parsed it as a line
        // comment and broke the surrounding expression. Pointee
        // positions must always produce a real type.
        assert_eq!(pointee_saw_type(&TypeInfo::Void), "llvm_int 8");
        assert!(!pointee_saw_type(&TypeInfo::Void).contains("//"));
    }

    #[test]
    fn pointee_saw_type_passes_through_real_types() {
        assert_eq!(pointee_saw_type(&TypeInfo::Bool), "llvm_int 1");
        assert_eq!(pointee_saw_type(&TypeInfo::UnsignedInt(64)), "llvm_int 64");
        assert_eq!(
            pointee_saw_type(&TypeInfo::ByteArray(16)),
            "llvm_array 16 (llvm_int 8)"
        );
    }

    #[test]
    fn pointee_saw_type_rewrites_option_and_result_sentinels() {
        // Any `type_to_saw` result starting with `"//"` is unsafe in
        // pointee position; the helper must scrub it.
        let opt = TypeInfo::Option(Box::new(TypeInfo::SignedInt(32)));
        let res = TypeInfo::Result(
            Box::new(TypeInfo::UnsignedInt(8)),
            Box::new(TypeInfo::SignedInt(32)),
        );
        assert_eq!(pointee_saw_type(&opt), "llvm_int 8");
        assert_eq!(pointee_saw_type(&res), "llvm_int 8");
    }

    #[test]
    fn opaque_std_integer_typedef_lowers_to_int() {
        // Bug #12: `std::int64_t` / `std::uint32_t` / `std::size_t`
        // etc. arriving as `TypeInfo::Opaque` (size 0) must lower to
        // `llvm_int N` rather than `llvm_alias "std::int64_t"`, which
        // would never resolve at SAW load time.
        for (name, expect) in [
            ("std::int8_t", "llvm_int 8"),
            ("std::int16_t", "llvm_int 16"),
            ("std::int32_t", "llvm_int 32"),
            ("std::int64_t", "llvm_int 64"),
            ("std::uint8_t", "llvm_int 8"),
            ("std::uint32_t", "llvm_int 32"),
            ("std::uint64_t", "llvm_int 64"),
            ("std::size_t", "llvm_int 64"),
            ("std::ptrdiff_t", "llvm_int 64"),
            ("std::intptr_t", "llvm_int 64"),
            ("std::uintptr_t", "llvm_int 64"),
            ("int64_t", "llvm_int 64"),
            ("uintptr_t", "llvm_int 64"),
        ] {
            let ty = TypeInfo::Opaque {
                name: name.into(),
                size_bytes: 0,
            };
            assert_eq!(type_to_saw(&ty), expect, "lowering for {name}");
        }
    }

    #[test]
    fn test_type_to_saw_float() {
        assert_eq!(type_to_saw(&TypeInfo::Float(32)), "llvm_float");
    }

    #[test]
    fn test_type_to_saw_double() {
        assert_eq!(type_to_saw(&TypeInfo::Float(64)), "llvm_double");
    }

    #[test]
    fn test_type_to_saw_float_pointer() {
        let ty = TypeInfo::Pointer(Box::new(TypeInfo::Float(64)));
        assert_eq!(type_to_saw(&ty), "llvm_double");
    }
}
