//! `std::basic_string` functional SAW spec emission. See sibling
//! [`super::bitcode_overrides_functional`] for the dispatcher and
//! the per-container architecture.
//!
//! ## Layout discovery
//!
//! Field indices come from the LLVM IR struct definition (parsed by
//! [`crate::parsers::llvm_ir::struct_defs`]). For libstdc++'s
//! `class.std::__cxx11::basic_string` the layout is:
//! ```text
//!   field 0 : %_Alloc_hider   (struct { ptr })   -> data pointer
//!   field 1 : i64                                -> _M_string_length
//!   field 2 : %union.anon     (16 bytes SSO buf)
//! ```
//! We look up the size field as "first scalar `iN` field". Integer
//! indices via `llvm_elem s N` are used in place of `llvm_field s
//! "_M_string_length"` because the latter requires DWARF debug info
//! that the e2e harness does not produce.

use std::collections::HashMap;

use crate::parsers::llvm_ir::IrStructDef;

use super::bitcode_overrides_functional::StlMethod;

/// Discovered IR layout for `std::basic_string`.
#[derive(Debug, Clone)]
pub struct StringLayout {
    /// The cleaned struct alias name as SAW expects it inside
    /// `llvm_alias "..."` (e.g. `class.std::__cxx11::basic_string`).
    pub alias: String,
    /// 0-based index of the size field within the *flat* IR layout
    /// (typically `1` for libstdc++).
    pub size_field_index: usize,
}

/// Find the LLVM IR struct that represents `std::basic_string` in the
/// current bitcode, and locate its size field. Returns `None` when
/// the IR does not declare any recognizable basic_string struct.
pub fn discover_string_layout(struct_defs: &HashMap<String, IrStructDef>) -> Option<StringLayout> {
    let alias = struct_defs.keys().find(|k| is_basic_string_alias(k))?;
    let def = struct_defs.get(alias)?;
    let size_field_index = def
        .fields
        .iter()
        .position(|f| f.trim() == "i64" || f.trim() == "i32")?;
    Some(StringLayout {
        alias: alias.clone(),
        size_field_index,
    })
}

fn is_basic_string_alias(name: &str) -> bool {
    let n = name;
    (n.starts_with("class.std::") || n.starts_with("class.std."))
        && n.contains("basic_string")
        && !n.contains("_Alloc_hider")
        && !n.contains("char_traits")
        && !n.contains("basic_stringbuf")
        && !n.contains("basic_stringstream")
}

/// Emit a SAW spec block + `llvm_unsafe_assume_spec` binding for one
/// recognized basic_string method. `safe` is the sanitized identifier
/// suffix used by [`super::bitcode_overrides::emit_one`] for the
/// `<safe>_spec` and `ov_<safe>` names.
pub fn emit_string_override(
    out: &mut String,
    method: StlMethod,
    layout: &StringLayout,
    symbol: &str,
    safe: &str,
    ov_name: &str,
) {
    out.push_str(&format!(
        "\n// override: {sym}  [stl-functional: {kind:?}]\n",
        sym = symbol,
        kind = method,
    ));
    out.push_str(&format!("let {safe}_spec = do {{\n"));
    out.push_str(&format!(
        "    s <- llvm_fresh_pointer (llvm_alias \"{}\");\n",
        layout.alias,
    ));
    let idx = layout.size_field_index;
    match method {
        StlMethod::BasicStringCtorDefault => {
            out.push_str("    llvm_execute_func [s];\n");
            out.push_str(&format!(
                "    llvm_points_to (llvm_elem s {idx}) (llvm_term {{{{ 0 : [64] }}}});\n",
            ));
        }
        StlMethod::BasicStringDtor => {
            // No observable side-effect — leave the post-state empty.
            out.push_str("    llvm_execute_func [s];\n");
        }
        StlMethod::BasicStringSize => {
            // Pre-state binds field `idx` to a fresh symbolic; post-
            // state returns the same symbolic. The next call (if any)
            // that touches that field will see the value written by
            // the most recent `resize`-style override.
            out.push_str("    sz <- llvm_fresh_var \"sz\" (llvm_int 64);\n");
            out.push_str(&format!(
                "    llvm_points_to (llvm_elem s {idx}) (llvm_term sz);\n",
            ));
            out.push_str("    llvm_execute_func [s];\n");
            out.push_str("    llvm_return (llvm_term sz);\n");
        }
        StlMethod::BasicStringResize => {
            out.push_str("    n <- llvm_fresh_var \"n\" (llvm_int 64);\n");
            out.push_str("    llvm_execute_func [s, llvm_term n];\n");
            out.push_str(&format!(
                "    llvm_points_to (llvm_elem s {idx}) (llvm_term n);\n",
            ));
        }
        StlMethod::BasicStringData => {
            out.push_str("    d <- llvm_fresh_pointer (llvm_int 8);\n");
            out.push_str("    llvm_points_to (llvm_elem s 0) d;\n");
            out.push_str("    llvm_execute_func [s];\n");
            out.push_str("    llvm_return d;\n");
        }
        // Vector-family variants are handled by the vector emitter.
        _ => unreachable!("emit_string_override called with non-string method"),
    }
    out.push_str("};\n");
    out.push_str(&format!(
        "{ov_name} <- llvm_unsafe_assume_spec m \"{symbol}\" {safe}_spec;\n",
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def_basic_string() -> (String, IrStructDef) {
        (
            "class.std::__cxx11::basic_string".to_string(),
            IrStructDef {
                fields: vec![
                    "%\"struct.std::__cxx11::basic_string<char>::_Alloc_hider\"".to_string(),
                    "i64".to_string(),
                    "%union.anon".to_string(),
                ],
                is_packed: false,
            },
        )
    }

    #[test]
    fn discover_string_layout_finds_libstdcpp_struct() {
        let mut defs = HashMap::new();
        let (k, v) = def_basic_string();
        defs.insert(k.clone(), v);
        let layout = discover_string_layout(&defs).expect("layout discovered");
        assert_eq!(layout.alias, k);
        assert_eq!(layout.size_field_index, 1);
    }

    #[test]
    fn discover_string_layout_returns_none_without_basic_string() {
        let mut defs = HashMap::new();
        defs.insert(
            "class.std::vector".to_string(),
            IrStructDef {
                fields: vec!["ptr".to_string(), "ptr".to_string(), "ptr".to_string()],
                is_packed: false,
            },
        );
        assert!(discover_string_layout(&defs).is_none());
    }

    #[test]
    fn discover_string_layout_skips_alloc_hider_alias() {
        let mut defs = HashMap::new();
        defs.insert(
            "struct.std::__cxx11::basic_string<char>::_Alloc_hider".to_string(),
            IrStructDef {
                fields: vec!["ptr".to_string()],
                is_packed: false,
            },
        );
        assert!(discover_string_layout(&defs).is_none());
    }

    #[test]
    fn emit_size_uses_integer_index_not_field_name() {
        let (alias, _) = def_basic_string();
        let layout = StringLayout {
            alias,
            size_field_index: 1,
        };
        let mut out = String::new();
        emit_string_override(
            &mut out,
            StlMethod::BasicStringSize,
            &layout,
            "_ZNKSt7__cxx1112basic_stringIcSt11char_traitsIcESaIcEE4sizeEv",
            "ov_size_safe",
            "ov_size",
        );
        assert!(out.contains("llvm_elem s 1"), "spec body:\n{out}");
        assert!(!out.contains("_M_string_length"));
        assert!(out.contains("llvm_return (llvm_term sz)"));
    }

    #[test]
    fn emit_resize_writes_field_after_execute() {
        let (alias, _) = def_basic_string();
        let layout = StringLayout {
            alias,
            size_field_index: 1,
        };
        let mut out = String::new();
        emit_string_override(
            &mut out,
            StlMethod::BasicStringResize,
            &layout,
            "_ZNSt7__cxx1112basic_stringIcSt11char_traitsIcESaIcEE6resizeEm",
            "ov_resize_safe",
            "ov_resize",
        );
        let exec_pos = out.find("llvm_execute_func").expect("has execute");
        let write_pos = out
            .find("llvm_points_to (llvm_elem s 1) (llvm_term n)")
            .expect("has write");
        assert!(write_pos > exec_pos, "write must come after execute");
    }
}
