//! Constructor extraction + IR-symbol filtering.
//!
//! Pure-virtual interface classes (e.g. `IKeyStore`) have implicit
//! default constructors that the compiler may never emit. Binding a
//! SAW spec against a non-existent symbol fails the proof; [`filter_ctors_by_ir_symbols`]
//! drops those before they reach `llvm_unsafe_assume_spec`.

use super::node::AstNode;
use super::type_ctx::{build as build_type_ctx, compute_class_layout, TypeContext};
use super::visitor::{walk, ClassStack, Visitor, WalkAction};
use crate::constraints::{FunctionInfo, TypeInfo};
use anyhow::Result;

/// A constructor extracted from a C++ class, used for emitting
/// `llvm_unsafe_assume_spec` ctor overrides.
#[derive(Debug, Clone)]
pub struct ClassConstructor {
    /// e.g. `"OkLog"`.
    pub class_name: String,
    /// Mangled constructor symbol (e.g. `"??0OkLog@@QEAA@XZ"`).
    pub mangled_name: String,
    /// LLVM struct type name used when allocating `this` (e.g.
    /// `"class.ILogger"`). Resolved through the layout chain.
    pub layout_type_name: String,
    /// Non-vptr data members in layout order. Each entry is
    /// `(field_name, TypeInfo, default_literal)`.
    pub layout_fields: Vec<(String, TypeInfo, String)>,
}

/// Extract default constructors for every class in `class_names`.
pub fn extract_constructors(
    ast: &AstNode,
    class_names: &[String],
) -> Result<Vec<ClassConstructor>> {
    let ctx = build_type_ctx(ast);
    let mut out = Vec::new();
    let mut v = CtorVisitor {
        out: &mut out,
        class_names,
        ctx: &ctx,
        stack: ClassStack::new(),
    };
    walk(ast, &mut v);
    Ok(out)
}

struct CtorVisitor<'a> {
    out: &'a mut Vec<ClassConstructor>,
    class_names: &'a [String],
    ctx: &'a TypeContext,
    stack: ClassStack,
}

impl<'a> Visitor for CtorVisitor<'a> {
    fn enter(&mut self, node: &AstNode) -> WalkAction {
        if node.kind == "CXXConstructorDecl" {
            if let Some(mangled) = node.mangled_name.as_deref() {
                if let Some(class_name) = self.stack.current() {
                    if self.class_names.iter().any(|c| c == class_name) {
                        let param_count = node
                            .inner
                            .iter()
                            .filter(|c| c.kind == "ParmVarDecl")
                            .count();
                        if param_count == 0 {
                            let (layout_type_name, layout_fields) =
                                compute_class_layout(class_name, self.ctx)
                                    .unwrap_or_else(|| {
                                        (format!("class.{class_name}"), Vec::new())
                                    });
                            self.out.push(ClassConstructor {
                                class_name: class_name.to_string(),
                                mangled_name: mangled.to_string(),
                                layout_type_name,
                                layout_fields,
                            });
                        }
                    }
                }
            }
        }
        self.stack.push_if_class(node);
        WalkAction::Continue
    }
    fn leave(&mut self, node: &AstNode) {
        self.stack.pop_if(node.class_name().is_some());
    }
}

/// Drop entries from `ctors` whose `mangled_name` isn't present in `ir_funcs`.
///
/// The LLVM IR-text parser keeps surrounding quotes on MSVC-mangled
/// names; we strip them before comparison so quoted and unquoted forms
/// match.
///
/// Itanium ABI emits multiple constructor variants:
///   - C1 = complete object constructor (standalone allocation)
///   - C2 = base object constructor (as subobject)
///   - C3 = allocating constructor
/// The clang AST exposes the C1 mangling, but the compiler often emits
/// only C2 if no C1 call site exists. When the AST's C1 symbol is
/// missing from the bitcode but C2 (or C3) is present, transparently
/// substitute the available variant so the override still applies.
/// Same logic applies for destructors (D0/D1/D2).
pub fn filter_ctors_by_ir_symbols(
    ctors: &mut Vec<ClassConstructor>,
    ir_funcs: &[FunctionInfo],
) {
    let ir_symbols: std::collections::HashSet<&str> = ir_funcs
        .iter()
        .map(|f| f.name.as_str().trim_matches('"'))
        .collect();
    let before = ctors.len();
    let mut dropped: Vec<String> = Vec::new();
    ctors.retain_mut(|c| {
        if ir_symbols.contains(c.mangled_name.as_str()) {
            return true;
        }
        // Try Itanium ABI variant substitution: C1↔C2↔C3, D0↔D1↔D2.
        if let Some(alt) = itanium_ctor_dtor_variants(&c.mangled_name)
            .into_iter()
            .find(|alt| ir_symbols.contains(alt.as_str()))
        {
            c.mangled_name = alt;
            return true;
        }
        dropped.push(format!(
            "{} (symbol {} not in bitcode)",
            c.class_name, c.mangled_name,
        ));
        false
    });
    if !dropped.is_empty() {
        eprintln!(
            "note: dropping {}/{} constructor override(s) whose mangled symbol \
             is absent from the bitcode (likely pure-virtual interface ctors that \
             the compiler never emitted):",
            dropped.len(),
            before,
        );
        for d in &dropped {
            eprintln!("  - {d}");
        }
    }
}

/// For an Itanium-ABI constructor or destructor mangling, return the
/// alternative variant manglings to try (in priority order). The input
/// is expected to embed `C<digit>` or `D<digit>` in the C++-name part
/// of an Itanium mangled symbol (e.g. `_ZN1AC1Ev`); we substitute the
/// digit with each of the other valid variants. Returns an empty Vec
/// for non-Itanium (MSVC `??0...`) names.
fn itanium_ctor_dtor_variants(mangled: &str) -> Vec<String> {
    if !mangled.starts_with("_Z") {
        return Vec::new();
    }
    // Scan for the FIRST occurrence of `C[123]` or `D[012]` followed
    // by `E` (end of nested-name). The `E` disambiguates from class
    // names that happen to contain `C1`/`D0`/etc.
    let bytes = mangled.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 2 < bytes.len() {
        let c = bytes[i];
        let d = bytes[i + 1];
        let e = bytes[i + 2];
        if e == b'E' && ((c == b'C' && (d == b'1' || d == b'2' || d == b'3'))
            || (c == b'D' && (d == b'0' || d == b'1' || d == b'2')))
        {
            // Generate alternatives by replacing the digit.
            let alts: &[u8] = if c == b'C' {
                match d {
                    b'1' => &[b'2', b'3'],
                    b'2' => &[b'1', b'3'],
                    b'3' => &[b'1', b'2'],
                    _ => &[],
                }
            } else {
                match d {
                    b'0' => &[b'1', b'2'],
                    b'1' => &[b'2', b'0'],
                    b'2' => &[b'1', b'0'],
                    _ => &[],
                }
            };
            for &alt in alts {
                let mut s = mangled.as_bytes().to_vec();
                s[i + 1] = alt;
                out.push(String::from_utf8(s).expect("ASCII substitution"));
            }
            return out;
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constraints::TypeInfo;
    use serde_json::json;

    fn parse(v: serde_json::Value) -> AstNode {
        serde_json::from_value(v).unwrap()
    }

    fn stub_ir_fn(name: &str) -> FunctionInfo {
        FunctionInfo {
            name: name.into(),
            mangled_name: None,
            params: vec![],
            return_type: TypeInfo::Void,
            can_throw: false,
            is_virtual: false,
            has_body: true,
            is_system: false,
            annotations: vec![],
            referenced_globals: vec![],
            called_functions: vec![],
        }
    }

    fn stub_ctor(class: &str, mangled: &str) -> ClassConstructor {
        ClassConstructor {
            class_name: class.into(),
            mangled_name: mangled.into(),
            layout_type_name: format!("class.{class}"),
            layout_fields: vec![],
        }
    }

    #[test]
    fn extracts_default_ctor_only() {
        let ast = parse(json!({
            "kind": "TranslationUnitDecl",
            "inner": [{
                "kind": "CXXRecordDecl",
                "name": "OkLog",
                "inner": [
                    {"kind": "CXXConstructorDecl", "name": "OkLog",
                     "mangledName": "??0OkLog@@QEAA@XZ",
                     "type": {"qualType": "void ()"}},
                    {"kind": "CXXConstructorDecl", "name": "OkLog",
                     "mangledName": "??0OkLog@@QEAA@AEBV0@@Z",
                     "type": {"qualType": "void (const OkLog&)"},
                     "inner": [
                         {"kind": "ParmVarDecl", "name": "o",
                          "type": {"qualType": "const OkLog&"}}
                     ]}
                ]
            }]
        }));
        let ctors =
            extract_constructors(&ast, &["OkLog".to_string()]).unwrap();
        assert_eq!(ctors.len(), 1);
        assert_eq!(ctors[0].mangled_name, "??0OkLog@@QEAA@XZ");
    }

    #[test]
    fn filter_drops_when_symbol_missing() {
        let mut c = vec![
            stub_ctor("Real", "??0Real@@QEAA@XZ"),
            stub_ctor("Missing", "??0Missing@@QEAA@XZ"),
        ];
        let ir = vec![stub_ir_fn("\"??0Real@@QEAA@XZ\"")];
        filter_ctors_by_ir_symbols(&mut c, &ir);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].class_name, "Real");
    }

    #[test]
    fn filter_strips_msvc_ir_quotes() {
        let mut c = vec![stub_ctor("OkLog", "??0OkLog@@QEAA@XZ")];
        let ir = vec![stub_ir_fn("\"??0OkLog@@QEAA@XZ\"")];
        filter_ctors_by_ir_symbols(&mut c, &ir);
        assert_eq!(c.len(), 1);
    }
}
