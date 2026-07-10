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
    // On GCC/Clang the LLVM type is `class.std::__cxx11::basic_string` (no
    // template args in the name). On MSVC the full template is spelled out:
    // `class.std::basic_string<char,struct std::char_traits<char>,...>`.
    // Both contain `basic_string`; neither contains `_Alloc_hider` or the
    // streaming variants. We no longer exclude `char_traits` here because the
    // MSVC full-template name naturally includes it as a template argument.
    (n.starts_with("class.std::") || n.starts_with("class.std."))
        && n.contains("basic_string")
        && !n.contains("_Alloc_hider")
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
        StlMethod::BasicStringByteViewNoArg => {
            // SSO handling (saw_spec_gen-xzg). libstdc++ basic_string
            // is short-string-optimized: for strings <= 15 chars
            // `data()` returns a pointer into the embedded
            // `_M_local_buf` (field 2 of the struct); for longer
            // strings it returns the heap pointer stashed in
            // `_M_dataplus._M_p` (field 0). The two branches return
            // pointers backed by storage in completely different
            // fields, and the discriminator is a length comparison
            // against 15.
            //
            // We use the *model-agnostic* option (a) from xzg:
            // ignore the pre-state of `s` entirely and return a
            // fresh symbolic byte pointer. This over-approximates
            // both SSO branches uniformly \u2014 any read through the
            // returned pointer sees fully symbolic bytes \u2014 and
            // avoids any cross-coupling with the `resize` / size
            // overrides (which only write field 1). Crucially, the
            // override has no `llvm_points_to` precondition on the
            // string's fields, so it never fails structural matching
            // when called against a string the caller initialized
            // only via `resize`.
            out.push_str("    llvm_execute_func [s];\n");
            out.push_str("    d <- llvm_fresh_pointer (llvm_int 8);\n");
            out.push_str("    llvm_return d;\n");
        }
        // operator[](pos) and at(pos) return char& (i8* in LLVM IR).
        // Content is symbolic; we return a fresh byte pointer.
        StlMethod::BasicStringByteViewIndexArg => {
            out.push_str("    idx <- llvm_fresh_var \"idx\" (llvm_int 64);\n");
            out.push_str("    llvm_execute_func [s, llvm_term idx];\n");
            out.push_str("    d <- llvm_fresh_pointer (llvm_int 8);\n");
            out.push_str("    llvm_return d;\n");
        }
        // basic_string(const char*): init size to symbolic `n`;
        // ctor returns void so no llvm_return.
        StlMethod::BasicStringCtorFromCStr => {
            out.push_str("    src <- llvm_fresh_pointer (llvm_int 8);\n");
            out.push_str("    n <- llvm_fresh_var \"n\" (llvm_int 64);\n");
            out.push_str("    llvm_execute_func [s, src];\n");
            out.push_str(&format!(
                "    llvm_points_to (llvm_elem s {idx}) (llvm_term n);\n",
            ));
        }
        // basic_string(const basic_string&): copy ctor — threads the
        // source size into the destination. Ctor returns void.
        StlMethod::BasicStringCtorCopy => {
            out.push_str(&format!(
                "    src <- llvm_fresh_pointer (llvm_alias \"{}\");\n",
                layout.alias,
            ));
            out.push_str("    sz_src <- llvm_fresh_var \"sz_src\" (llvm_int 64);\n");
            out.push_str(&format!(
                "    llvm_points_to (llvm_elem src {idx}) (llvm_term sz_src);\n",
            ));
            out.push_str("    llvm_execute_func [s, src];\n");
            out.push_str(&format!(
                "    llvm_points_to (llvm_elem s {idx}) (llvm_term sz_src);\n",
            ));
        }
        // assign(const char*) / operator=(const char*): set size to
        // a fresh symbolic value; returns *this (basic_string&).
        StlMethod::BasicStringAssignCStr | StlMethod::BasicStringOpEqCStr => {
            out.push_str("    src <- llvm_fresh_pointer (llvm_int 8);\n");
            out.push_str("    n <- llvm_fresh_var \"n\" (llvm_int 64);\n");
            out.push_str("    llvm_execute_func [s, src];\n");
            out.push_str(&format!(
                "    llvm_points_to (llvm_elem s {idx}) (llvm_term n);\n",
            ));
            out.push_str("    llvm_return s;\n");
        }
        // assign(const basic_string&) / operator=(const basic_string&):
        // threads the source size into the destination; returns *this.
        StlMethod::BasicStringAssignStr | StlMethod::BasicStringOpEqStr => {
            out.push_str(&format!(
                "    src <- llvm_fresh_pointer (llvm_alias \"{}\");\n",
                layout.alias,
            ));
            out.push_str("    sz_src <- llvm_fresh_var \"sz_src\" (llvm_int 64);\n");
            out.push_str(&format!(
                "    llvm_points_to (llvm_elem src {idx}) (llvm_term sz_src);\n",
            ));
            out.push_str("    llvm_execute_func [s, src];\n");
            out.push_str(&format!(
                "    llvm_points_to (llvm_elem s {idx}) (llvm_term sz_src);\n",
            ));
            out.push_str("    llvm_return s;\n");
        }
        // empty(): reads the size field and returns sz == 0 as i1.
        StlMethod::BasicStringEmpty => {
            out.push_str("    sz <- llvm_fresh_var \"sz\" (llvm_int 64);\n");
            out.push_str(&format!(
                "    llvm_points_to (llvm_elem s {idx}) (llvm_term sz);\n",
            ));
            out.push_str("    llvm_execute_func [s];\n");
            out.push_str("    llvm_return (llvm_term {{ sz == 0 }});\n");
        }
        // Size-neutral scalar query family (e.g. capacity()).
        // We don't track capacity, so return a fresh i64.
        StlMethod::BasicStringSizeNeutralScalarQuery => {
            out.push_str("    cap <- llvm_fresh_var \"cap\" (llvm_int 64);\n");
            out.push_str("    llvm_execute_func [s];\n");
            out.push_str("    llvm_return (llvm_term cap);\n");
        }
        // Size-neutral mutator family (e.g. reserve(n)): no
        // observable post-state in our model.
        StlMethod::BasicStringSizeNeutralMutator => {
            out.push_str("    n <- llvm_fresh_var \"n\" (llvm_int 64);\n");
            out.push_str("    llvm_execute_func [s, llvm_term n];\n");
        }
        // Vector-family variants and any future unknowns.
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

    #[test]
    fn emit_data_is_model_agnostic_no_prestate_points_to() {
        // saw_spec_gen-xzg: the data() override must NOT impose a
        // pre-state `llvm_points_to` on the string's data-pointer
        // field, otherwise SAW's structural matcher fails for any
        // string that was only `resize`d (which only touches the
        // size field). The override returns a fresh symbolic byte
        // pointer to over-approximate both SSO branches uniformly.
        let (alias, _) = def_basic_string();
        let layout = StringLayout {
            alias,
            size_field_index: 1,
        };
        let mut out = String::new();
        emit_string_override(
            &mut out,
            StlMethod::BasicStringByteViewNoArg,
            &layout,
            "_ZNSt7__cxx1112basic_stringIcSt11char_traitsIcESaIcEE4dataEv",
            "ov_data_safe",
            "ov_data",
        );
        // No pre-state assertion on field 0 (or any field):
        let exec_pos = out.find("llvm_execute_func").expect("has execute");
        let pre_slice = &out[..exec_pos];
        assert!(
            !pre_slice.contains("llvm_points_to"),
            "data() pre-state must be empty for option (a); got:\n{out}",
        );
        // Returns a fresh symbolic byte pointer:
        assert!(out.contains("llvm_fresh_pointer (llvm_int 8)"));
        assert!(out.contains("llvm_return d"));
    }

    #[test]
    fn emit_cstr_matches_data_pattern() {
        let (alias, _) = def_basic_string();
        let layout = StringLayout {
            alias,
            size_field_index: 1,
        };
        let mut out = String::new();
        emit_string_override(
            &mut out,
            StlMethod::BasicStringByteViewNoArg,
            &layout,
            "_ZNKSt7__cxx1112basic_stringIcSt11char_traitsIcESaIcEE5c_strEv",
            "ov_cstr_safe",
            "ov_cstr",
        );
        assert!(out.contains("llvm_fresh_pointer (llvm_int 8)"));
        assert!(out.contains("llvm_return d"));
        let exec_pos = out.find("llvm_execute_func").expect("has execute");
        assert!(!out[..exec_pos].contains("llvm_points_to"));
    }

    #[test]
    fn emit_index_passes_idx_and_returns_fresh_pointer() {
        let (alias, _) = def_basic_string();
        let layout = StringLayout {
            alias,
            size_field_index: 1,
        };
        let mut out = String::new();
        emit_string_override(
            &mut out,
            StlMethod::BasicStringByteViewIndexArg,
            &layout,
            "_ZNSt7__cxx1112basic_stringIcSt11char_traitsIcESaIcEEixEm",
            "ov_idx_safe",
            "ov_idx",
        );
        assert!(out.contains("llvm_fresh_var \"idx\""));
        assert!(out.contains("llvm_execute_func [s, llvm_term idx]"));
        assert!(out.contains("llvm_fresh_pointer (llvm_int 8)"));
        assert!(out.contains("llvm_return d"));
    }

    #[test]
    fn emit_empty_returns_size_eq_zero() {
        let (alias, _) = def_basic_string();
        let layout = StringLayout {
            alias,
            size_field_index: 1,
        };
        let mut out = String::new();
        emit_string_override(
            &mut out,
            StlMethod::BasicStringEmpty,
            &layout,
            "_ZNKSt7__cxx1112basic_stringIcSt11char_traitsIcESaIcEE5emptyEv",
            "ov_empty_safe",
            "ov_empty",
        );
        assert!(out.contains("llvm_fresh_var \"sz\""));
        assert!(out.contains("llvm_points_to (llvm_elem s 1) (llvm_term sz)"));
        assert!(out.contains("llvm_return (llvm_term {{ sz == 0 }})"));
    }

    #[test]
    fn emit_copy_assign_threads_size_from_src_to_dst() {
        let (alias, _) = def_basic_string();
        let layout = StringLayout {
            alias: alias.clone(),
            size_field_index: 1,
        };
        let mut out = String::new();
        emit_string_override(
            &mut out,
            StlMethod::BasicStringAssignStr,
            &layout,
            "_ZNSt7__cxx1112basic_stringIcSt11char_traitsIcESaIcEE6assignERKS4_",
            "ov_assign_safe",
            "ov_assign",
        );
        assert!(out.contains(&format!("llvm_fresh_pointer (llvm_alias \"{alias}\")",)));
        assert!(out.contains("sz_src"));
        assert!(out.contains("llvm_points_to (llvm_elem src 1) (llvm_term sz_src)"));
        assert!(out.contains("llvm_points_to (llvm_elem s 1) (llvm_term sz_src)"));
        assert!(out.contains("llvm_return s"));
    }

    #[test]
    fn emit_ctor_from_cstr_sets_symbolic_size_no_return() {
        let (alias, _) = def_basic_string();
        let layout = StringLayout {
            alias,
            size_field_index: 1,
        };
        let mut out = String::new();
        emit_string_override(
            &mut out,
            StlMethod::BasicStringCtorFromCStr,
            &layout,
            "_ZNSt7__cxx1112basic_stringIcSt11char_traitsIcESaIcEEC1EPKc",
            "ov_ctor_cstr_safe",
            "ov_ctor_cstr",
        );
        assert!(out.contains("llvm_fresh_pointer (llvm_int 8)"));
        assert!(out.contains("llvm_fresh_var \"n\""));
        assert!(out.contains("llvm_points_to (llvm_elem s 1) (llvm_term n)"));
        assert!(
            !out.contains("llvm_return"),
            "ctor must not emit llvm_return"
        );
    }

    #[test]
    fn emit_reserve_has_no_post_state() {
        let (alias, _) = def_basic_string();
        let layout = StringLayout {
            alias,
            size_field_index: 1,
        };
        let mut out = String::new();
        emit_string_override(
            &mut out,
            StlMethod::BasicStringSizeNeutralMutator,
            &layout,
            "_ZNSt7__cxx1112basic_stringIcSt11char_traitsIcESaIcEE7reserveEm",
            "ov_reserve_safe",
            "ov_reserve",
        );
        let exec_pos = out.find("llvm_execute_func").expect("has execute");
        let post_slice = &out[exec_pos..];
        assert!(
            !post_slice.contains("llvm_points_to"),
            "reserve post-state must be empty; got:\n{out}",
        );
        assert!(!out.contains("llvm_return"));
    }
}
