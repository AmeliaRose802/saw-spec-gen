//! C++ qualType-string parsing and known-type size tables.
//!
//! Clang exposes types in two complementary places: as nested AST
//! children (for fully resolved layouts) and as the textual
//! `qualType` field for everything that flows through a function
//! signature. The original module mostly relies on the latter — we
//! follow suit but route every access through the helpers here so
//! the string-munging stays in one place.

use super::type_ctx::TypeContext;
use crate::constraints::TypeInfo;

/// Strip C/C++ `restrict` qualifiers (`__restrict`, `__restrict__`,
/// `restrict`) from a qualType string. Clang preserves these but they
/// don't affect SAW's type model and they break trailing-`*` pointer
/// detection if left in.
pub fn strip_restrict(s: &str) -> String {
    let mut out = s.replace("__restrict__", "");
    out = out.replace("__restrict", "");
    let padded = format!(" {out} ");
    let stripped = padded.replace(" restrict ", " ");
    stripped.trim().to_string()
}

/// Parse a C++ qualType string into a [`TypeInfo`].
pub fn parse_cpp_type(qual_type: &str, ctx: &TypeContext) -> TypeInfo {
    let stripped = strip_restrict(qual_type);
    let t = stripped.trim();
    let t = t.trim_start_matches("const ");

    // Detect C array types like `uint8_t[32]` or `int[10]`.
    if let Some(bracket_pos) = t.find('[') {
        if t.ends_with(']') {
            let elem_type_str = t[..bracket_pos].trim();
            let count_str = &t[bracket_pos + 1..t.len() - 1];
            if let Ok(count) = count_str.parse::<usize>() {
                let elem_type = parse_cpp_type(elem_type_str, ctx);
                if matches!(elem_type, TypeInfo::UnsignedInt(8) | TypeInfo::SignedInt(8)) {
                    return TypeInfo::ByteArray(count);
                }
                if let Some((elem_size, _)) = cpp_type_size_align(&elem_type) {
                    return TypeInfo::ByteArray(count * elem_size);
                }
            }
        }
    }

    let is_ptr = t.ends_with('*');
    let is_ref = t.ends_with('&');
    let t = t.trim_end_matches('&').trim_end_matches('*').trim();

    // Normalize std-qualified fixed-width and pointer-sized integer
    // typedefs (`std::int64_t`, `std::size_t`, etc.) to their
    // unqualified C forms so the explicit match arms below catch them
    // instead of falling through to `resolve_named` and ending up as an
    // unresolvable `llvm_alias`. See bug #12 in the tester report.
    // Only rewrite when the suffix is itself a recognized integer
    // typedef, so we don't accidentally strip `std::` from container
    // names like `std::vector<int>`.
    let t = match t.strip_prefix("std::") {
        Some(rest) if is_std_integer_typedef(rest) => rest,
        _ => t,
    };

    let base = match t {
        "bool" | "_Bool" => TypeInfo::Bool,
        "int" | "int32_t" | "long" => TypeInfo::SignedInt(32),
        "long long" | "int64_t" | "__int64" | "intptr_t" | "ptrdiff_t" => TypeInfo::SignedInt(64),
        "short" | "int16_t" => TypeInfo::SignedInt(16),
        "char" | "int8_t" | "signed char" => TypeInfo::SignedInt(8),
        "unsigned int" | "uint32_t" | "unsigned long" | "DWORD" | "ULONG" => {
            TypeInfo::UnsignedInt(32)
        }
        "unsigned long long" | "uint64_t" | "size_t" | "__size_t" | "UINT64" | "ULONG64"
        | "uintptr_t" | "ssize_t" => TypeInfo::UnsignedInt(64),
        "unsigned short" | "uint16_t" | "WORD" | "USHORT" => TypeInfo::UnsignedInt(16),
        "unsigned char" | "uint8_t" | "BYTE" | "UCHAR" => TypeInfo::UnsignedInt(8),
        "float" => TypeInfo::Float(32),
        "double" => TypeInfo::Float(64),
        "long double" => TypeInfo::Float(128),
        "void" => TypeInfo::Void,
        other => resolve_named(other, ctx),
    };

    if is_ptr || is_ref {
        TypeInfo::Pointer(Box::new(base))
    } else {
        base
    }
}

/// Return `true` for the fixed-width and pointer-sized integer typedef
/// names that should be lowered to primitive `TypeInfo::SignedInt` /
/// `TypeInfo::UnsignedInt` regardless of whether they appear bare or
/// `std::`-qualified.
fn is_std_integer_typedef(name: &str) -> bool {
    matches!(
        name,
        "int8_t"
            | "int16_t"
            | "int32_t"
            | "int64_t"
            | "uint8_t"
            | "uint16_t"
            | "uint32_t"
            | "uint64_t"
            | "size_t"
            | "ssize_t"
            | "ptrdiff_t"
            | "intptr_t"
            | "uintptr_t"
    )
}

/// Look up a non-primitive type name. Tries `ctx.enums`, then
/// `ctx.structs`, then [`lookup_known_type_size`] (STL / Win32 names),
/// and falls back to [`TypeInfo::Opaque`] with size 0.
fn resolve_named(other: &str, ctx: &TypeContext) -> TypeInfo {
    if let Some((variants, bits)) = ctx.enums.get(other) {
        return TypeInfo::Enum {
            name: other.to_string(),
            variants: variants.clone(),
            discriminant_bits: *bits,
        };
    }
    if let Some(fields) = ctx.structs.get(other) {
        let size = compute_struct_size_from_fields(fields);
        return TypeInfo::Struct {
            name: other.to_string(),
            size_bytes: size,
            fields: fields.clone(),
        };
    }
    if let Some(size) = lookup_known_type_size(other) {
        return TypeInfo::Opaque {
            name: other.to_string(),
            size_bytes: size,
        };
    }
    TypeInfo::Opaque {
        name: other.to_string(),
        size_bytes: 0,
    }
}

/// Extract the function's return type from a `"Ret (Params)"` qualType.
pub fn parse_return_type(qual_type: &str, ctx: &TypeContext) -> TypeInfo {
    let ret = qual_type.split('(').next().unwrap_or("void").trim();
    parse_cpp_type(ret, ctx)
}

/// Well-known C++ STL and platform type sizes for the MSVC x64 ABI.
pub fn lookup_known_type_size(name: &str) -> Option<usize> {
    let normalized = name
        .trim_start_matches("class ")
        .trim_start_matches("struct ");
    match normalized {
        // STL containers and strings (MSVC x64 sizes)
        "std::string"
        | "std::basic_string<char>"
        | "std::basic_string<char, std::char_traits<char>, std::allocator<char>>" => Some(32),
        "std::wstring"
        | "std::basic_string<wchar_t>"
        | "std::basic_string<wchar_t, std::char_traits<wchar_t>, std::allocator<wchar_t>>" => {
            Some(32)
        }
        s if s.starts_with("std::basic_string<") => Some(32),
        s if s.starts_with("std::vector<") => Some(24),
        s if s.starts_with("std::array<") => None,
        s if s.starts_with("std::map<") || s.starts_with("std::set<") => Some(16),
        s if s.starts_with("std::unordered_map<") || s.starts_with("std::unordered_set<") => {
            Some(64)
        }
        s if s.starts_with("std::shared_ptr<") => Some(16),
        s if s.starts_with("std::unique_ptr<") => Some(8),
        s if s.starts_with("std::optional<") => None,
        s if s.starts_with("std::variant<") => None,
        s if s.starts_with("std::tuple<") => None,
        "std::mutex" | "std::recursive_mutex" => Some(80),
        s if s.starts_with("std::function<") => Some(64),
        s if s.starts_with("std::span<") => Some(16),
        "std::string_view" | "std::basic_string_view<char>" => Some(16),
        "std::wstring_view" | "std::basic_string_view<wchar_t>" => Some(16),
        // Win32 / MSVC types
        "GUID" | "CLSID" | "IID" => Some(16),
        "FILETIME" | "LARGE_INTEGER" | "ULARGE_INTEGER" => Some(8),
        "CRITICAL_SECTION" => Some(40),
        "SRWLOCK" => Some(8),
        _ => None,
    }
}

/// Compute struct size using C/C++ x64 layout rules (natural alignment,
/// no packing pragmas considered).
pub fn compute_struct_size_from_fields(fields: &[(String, TypeInfo)]) -> Option<usize> {
    let mut offset = 0usize;
    let mut max_align = 1usize;
    for (_, ty) in fields {
        let (size, align) = cpp_type_size_align(ty)?;
        max_align = max_align.max(align);
        let remainder = offset % align;
        if remainder != 0 {
            offset += align - remainder;
        }
        offset += size;
    }
    if max_align > 0 {
        let remainder = offset % max_align;
        if remainder != 0 {
            offset += max_align - remainder;
        }
    }
    Some(offset)
}

/// `(size, alignment)` for a [`TypeInfo`] under the x64 ABI.
pub fn cpp_type_size_align(ty: &TypeInfo) -> Option<(usize, usize)> {
    match ty {
        TypeInfo::Bool => Some((1, 1)),
        TypeInfo::SignedInt(8) | TypeInfo::UnsignedInt(8) => Some((1, 1)),
        TypeInfo::SignedInt(16) | TypeInfo::UnsignedInt(16) => Some((2, 2)),
        TypeInfo::SignedInt(32) | TypeInfo::UnsignedInt(32) => Some((4, 4)),
        TypeInfo::SignedInt(64) | TypeInfo::UnsignedInt(64) => Some((8, 8)),
        TypeInfo::Float(32) => Some((4, 4)),
        TypeInfo::Float(64) => Some((8, 8)),
        TypeInfo::Float(128) => Some((16, 16)),
        TypeInfo::Pointer(_) => Some((8, 8)),
        TypeInfo::ByteArray(n) => Some((*n, 1)),
        TypeInfo::Enum {
            discriminant_bits, ..
        } => {
            let bytes = (*discriminant_bits).div_ceil(8) as usize;
            Some((bytes, bytes.max(1)))
        }
        TypeInfo::Struct {
            size_bytes: Some(n),
            ..
        } => Some((*n, 8.min(*n))),
        TypeInfo::Opaque { size_bytes, .. } if *size_bytes > 0 => {
            Some((*size_bytes, 8.min(*size_bytes)))
        }
        TypeInfo::Void => Some((0, 1)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_restrict_removes_all_three_variants() {
        assert_eq!(strip_restrict("uint32_t *__restrict"), "uint32_t *");
        assert_eq!(strip_restrict("int * __restrict__"), "int *");
        assert_eq!(strip_restrict("char * restrict "), "char *");
        // Whole word — identifier suffixes left alone:
        assert_eq!(strip_restrict("Restrictor"), "Restrictor");
    }

    #[test]
    fn parses_primitives() {
        let ctx = TypeContext::empty();
        assert_eq!(parse_cpp_type("int", &ctx), TypeInfo::SignedInt(32));
        assert_eq!(parse_cpp_type("uint64_t", &ctx), TypeInfo::UnsignedInt(64));
        assert_eq!(parse_cpp_type("BYTE", &ctx), TypeInfo::UnsignedInt(8));
        assert_eq!(parse_cpp_type("bool", &ctx), TypeInfo::Bool);
    }

    #[test]
    fn pointer_wraps_base_type() {
        let ctx = TypeContext::empty();
        let t = parse_cpp_type("int *", &ctx);
        assert!(matches!(t, TypeInfo::Pointer(b) if *b == TypeInfo::SignedInt(32)));
    }

    #[test]
    fn const_qualifier_is_stripped_before_lookup() {
        let ctx = TypeContext::empty();
        assert_eq!(parse_cpp_type("const int", &ctx), TypeInfo::SignedInt(32));
    }

    #[test]
    fn c_array_returns_byte_array() {
        let ctx = TypeContext::empty();
        assert_eq!(parse_cpp_type("uint8_t[16]", &ctx), TypeInfo::ByteArray(16));
        // i32 array → byte array sized in bytes
        assert_eq!(parse_cpp_type("int[4]", &ctx), TypeInfo::ByteArray(16));
    }

    #[test]
    fn unknown_resolves_to_opaque() {
        let ctx = TypeContext::empty();
        let t = parse_cpp_type("MyCustomClass", &ctx);
        assert!(matches!(t, TypeInfo::Opaque { ref name, .. } if name == "MyCustomClass"));
    }

    #[test]
    fn std_qualified_integer_typedefs_lower_to_primitives() {
        // Bug #12: `std::int64_t` is a primitive integer typedef, not a
        // struct. It must not fall through to `resolve_named` (where it
        // would become an unresolvable `llvm_alias "std::int64_t"`).
        let ctx = TypeContext::empty();
        assert_eq!(
            parse_cpp_type("std::int64_t", &ctx),
            TypeInfo::SignedInt(64)
        );
        assert_eq!(
            parse_cpp_type("std::uint32_t", &ctx),
            TypeInfo::UnsignedInt(32)
        );
        assert_eq!(
            parse_cpp_type("std::size_t", &ctx),
            TypeInfo::UnsignedInt(64)
        );
        assert_eq!(
            parse_cpp_type("std::ptrdiff_t", &ctx),
            TypeInfo::SignedInt(64)
        );
        // Pointer forms still wrap the primitive.
        let t = parse_cpp_type("std::int64_t *", &ctx);
        assert!(matches!(t, TypeInfo::Pointer(b) if *b == TypeInfo::SignedInt(64)));
        // `const` qualifier interacts correctly.
        assert_eq!(
            parse_cpp_type("const std::uint64_t", &ctx),
            TypeInfo::UnsignedInt(64)
        );
    }

    #[test]
    fn std_prefix_not_stripped_from_container_names() {
        // Regression guard: stripping `std::` indiscriminately would
        // break the STL container size lookup in
        // `lookup_known_type_size` (e.g. `std::vector<int>` \u2192 24).
        let ctx = TypeContext::empty();
        let t = parse_cpp_type("std::vector<int>", &ctx);
        match t {
            TypeInfo::Opaque { name, size_bytes } => {
                assert_eq!(name, "std::vector<int>");
                assert_eq!(size_bytes, 24);
            }
            other => panic!("expected Opaque(std::vector<int>, 24), got {other:?}"),
        }
    }

    #[test]
    fn lookup_known_handles_template_prefixes() {
        assert_eq!(lookup_known_type_size("std::vector<int>"), Some(24));
        assert_eq!(lookup_known_type_size("std::shared_ptr<int>"), Some(16));
        assert_eq!(lookup_known_type_size("std::unique_ptr<int>"), Some(8));
        assert_eq!(lookup_known_type_size("std::string"), Some(32));
        assert_eq!(lookup_known_type_size("std::string_view"), Some(16));
        assert_eq!(lookup_known_type_size("GUID"), Some(16));
        assert_eq!(lookup_known_type_size("CRITICAL_SECTION"), Some(40));
        assert_eq!(lookup_known_type_size("Definitely::Not::Known"), None);
    }

    #[test]
    fn struct_size_includes_padding() {
        let fields = vec![
            ("a".into(), TypeInfo::SignedInt(8)),
            ("b".into(), TypeInfo::SignedInt(32)),
        ];
        assert_eq!(compute_struct_size_from_fields(&fields), Some(8));
    }

    #[test]
    fn parse_return_type_splits_on_first_paren() {
        let ctx = TypeContext::empty();
        assert_eq!(
            parse_return_type("int (int, int)", &ctx),
            TypeInfo::SignedInt(32),
        );
        assert_eq!(parse_return_type("void (int)", &ctx), TypeInfo::Void);
    }

    #[test]
    fn parses_float_and_double() {
        let ctx = TypeContext::empty();
        assert_eq!(parse_cpp_type("float", &ctx), TypeInfo::Float(32));
        assert_eq!(parse_cpp_type("double", &ctx), TypeInfo::Float(64));
        assert_eq!(parse_cpp_type("long double", &ctx), TypeInfo::Float(128));
    }

    #[test]
    fn const_float_is_stripped() {
        let ctx = TypeContext::empty();
        assert_eq!(parse_cpp_type("const float", &ctx), TypeInfo::Float(32));
        assert_eq!(parse_cpp_type("const double", &ctx), TypeInfo::Float(64));
    }

    #[test]
    fn float_pointer_wraps_base_type() {
        let ctx = TypeContext::empty();
        let t = parse_cpp_type("float *", &ctx);
        assert!(matches!(t, TypeInfo::Pointer(b) if *b == TypeInfo::Float(32)));
        let t = parse_cpp_type("double *", &ctx);
        assert!(matches!(t, TypeInfo::Pointer(b) if *b == TypeInfo::Float(64)));
    }

    #[test]
    fn float_return_type_parsed() {
        let ctx = TypeContext::empty();
        assert_eq!(
            parse_return_type("double (double, double)", &ctx),
            TypeInfo::Float(64),
        );
        assert_eq!(parse_return_type("float (int)", &ctx), TypeInfo::Float(32),);
    }

    #[test]
    fn float_size_align() {
        assert_eq!(cpp_type_size_align(&TypeInfo::Float(32)), Some((4, 4)));
        assert_eq!(cpp_type_size_align(&TypeInfo::Float(64)), Some((8, 8)));
        assert_eq!(cpp_type_size_align(&TypeInfo::Float(128)), Some((16, 16)));
    }

    #[test]
    fn struct_with_float_field_sizes_correctly() {
        let fields = vec![
            ("x".into(), TypeInfo::Float(32)),
            ("y".into(), TypeInfo::Float(32)),
        ];
        assert_eq!(compute_struct_size_from_fields(&fields), Some(8));
    }

    #[test]
    fn float_array_returns_byte_array() {
        let ctx = TypeContext::empty();
        // float[4] = 4 * 4 bytes = 16 bytes
        assert_eq!(parse_cpp_type("float[4]", &ctx), TypeInfo::ByteArray(16));
        // double[2] = 2 * 8 bytes = 16 bytes
        assert_eq!(parse_cpp_type("double[2]", &ctx), TypeInfo::ByteArray(16));
    }
}
