//! Mapping from [`TypeInfo`] to SAW spec type strings.

use super::types::TypeInfo;

pub fn type_to_saw(ty: &TypeInfo) -> String {
    match ty {
        TypeInfo::Bool => "llvm_int 1".into(),
        TypeInfo::SignedInt(bits) | TypeInfo::UnsignedInt(bits) => format!("llvm_int {bits}"),
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
            } else {
                format!("llvm_alias \"{name}\"")
            }
        }
        TypeInfo::Void => "// void".into(),
    }
}

#[cfg(test)]
mod tests {
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
            variants: vec!["Ok".into(), "Err".into(), "Pending".into()],
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
}
