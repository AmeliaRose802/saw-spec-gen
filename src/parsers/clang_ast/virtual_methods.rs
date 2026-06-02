//! Virtual-method extraction and virtual-destructor detection.

use super::functions::parse_function_decl;
use super::node::AstNode;
use super::system_headers::is_system_include_path;
use super::type_ctx::{build as build_type_ctx, build_node_id_map, TypeContext};
use super::visitor::{walk, ClassStack, Visitor, WalkAction};
use crate::constraints::FunctionInfo;
use anyhow::Result;
use std::collections::{HashMap, HashSet};

/// A virtual method extracted from a C++ class, used to generate
/// interface stubs and vtable havoc specs.
#[derive(Debug, Clone)]
pub struct InterfaceMethod {
    /// The declaring class (e.g. `"IValidator"`).
    pub class_name: String,
    /// The method's [`FunctionInfo`], including the implicit `this` param.
    pub method: FunctionInfo,
    /// `= 0` in the source.
    pub is_pure: bool,
    /// Carries an `OverrideAttr` (true for `override`-marked derived
    /// methods, false for base/own first-declaration virtual methods).
    pub is_override: bool,
    /// Source offset of the declaration, used to order methods by their
    /// position in the class body — this is the order MSVC assigns to
    /// vtable slots. Falls back to 0 for synthesized/implicit decls.
    pub source_offset: u64,
}

/// Find every class with a virtual destructor in `ast`, either:
/// - explicit `virtual ~T()`, or
/// - `~T() override` (clang doesn't set `"virtual": true` on these), or
/// - inherits a virtual destructor from a base class (transitive).
pub fn classes_with_virtual_dtor(ast: &AstNode) -> HashSet<String> {
    let mut set: HashSet<String> = HashSet::new();
    let mut bases: HashMap<String, Vec<String>> = HashMap::new();
    let mut v = VDtorVisitor {
        out: &mut set,
        bases: &mut bases,
        stack: ClassStack::new(),
    };
    walk(ast, &mut v);
    propagate_through_bases(&mut set, &bases);
    set
}

struct VDtorVisitor<'a> {
    out: &'a mut HashSet<String>,
    bases: &'a mut HashMap<String, Vec<String>>,
    stack: ClassStack,
}

impl<'a> Visitor for VDtorVisitor<'a> {
    fn enter(&mut self, node: &AstNode) -> WalkAction {
        if let Some(name) = node.class_name() {
            // Strip `class `/`struct ` from base qualType names so the
            // resulting map keys match the keys we insert from elsewhere.
            let base_names: Vec<String> = node
                .bases
                .iter()
                .filter_map(|b| b.r#type.as_ref().and_then(|t| t.qual_type.as_deref()))
                .map(|bn| {
                    bn.trim_start_matches("class ")
                        .trim_start_matches("struct ")
                        .to_string()
                })
                .collect();
            if !base_names.is_empty() {
                self.bases.insert(name.to_string(), base_names);
            }
        }
        if node.kind == "CXXDestructorDecl" {
            let is_virtual = node.r#virtual == Some(true);
            let is_implicit = node.is_implicit == Some(true);
            let has_override = node.has_override_attr();
            if (is_virtual || has_override) && !is_implicit {
                if let Some(class) = self.stack.current() {
                    self.out.insert(class.to_string());
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

/// Walk the base-class map to a fixed point.
fn propagate_through_bases(set: &mut HashSet<String>, bases: &HashMap<String, Vec<String>>) {
    loop {
        let mut changed = false;
        let derived: Vec<String> = bases.keys().cloned().collect();
        for cls in derived {
            if set.contains(&cls) {
                continue;
            }
            if let Some(bs) = bases.get(&cls) {
                if bs.iter().any(|b| set.contains(b)) {
                    set.insert(cls);
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
}

/// Extract every virtual method from `ast`, optionally filtered by name
/// substring. Carries enough metadata for vtable-slot ordering.
///
/// Classes whose declaration originates in a system/SDK header (per
/// [`is_system_include_path`]) are skipped. STL classes like
/// `std::exception` and `std::bad_variant_access` survive the AST
/// path filter when they appear under a kept `namespace std { … }`
/// wrapper, but the verifier has no business generating havoc specs
/// and vtable stubs for them — the auto-emitted return-type lowering
/// frequently disagrees with the stub's pointer signature (e.g.
/// `what()` returns `const char *` but the spec generator falls
/// back to `llvm_int 8`), aborting SAW load. User code that wants to
/// model an STL virtual method should provide its own override.
pub fn extract_virtual_methods(
    ast: &AstNode,
    filter: Option<&str>,
) -> Result<Vec<InterfaceMethod>> {
    let ctx = build_type_ctx(ast);
    let id_to_name = build_node_id_map(ast);
    let mut out = Vec::new();
    let mut v = VMethodVisitor {
        out: &mut out,
        filter,
        ctx: &ctx,
        id_to_name: &id_to_name,
        stack: ClassStack::new(),
        current_file: None,
        file_stack: Vec::new(),
        system_depth: 0,
    };
    walk(ast, &mut v);
    Ok(out)
}

struct VMethodVisitor<'a> {
    out: &'a mut Vec<InterfaceMethod>,
    filter: Option<&'a str>,
    ctx: &'a TypeContext,
    id_to_name: &'a HashMap<String, String>,
    stack: ClassStack,
    /// Most recently seen `loc.file`. Clang elides this field on
    /// consecutive siblings sharing the same source file, so we
    /// remember it across the walk to recover the effective file for
    /// elided nodes.
    current_file: Option<String>,
    /// Saved `current_file` at each class push, restored on pop, so a
    /// class declared in user code immediately after a system-header
    /// namespace doesn't inherit the system file.
    file_stack: Vec<Option<String>>,
    /// Depth into nested classes whose declaration is in a system
    /// header. Methods are emitted only when this is zero.
    system_depth: usize,
}

impl<'a> VMethodVisitor<'a> {
    /// Update `current_file` from `node.loc.file` (if present) and
    /// return whether the resulting effective file is a system header.
    fn note_loc(&mut self, node: &AstNode) -> bool {
        if let Some(file) = node.loc.as_ref().and_then(|l| l.file.as_deref()) {
            self.current_file = Some(file.to_string());
        }
        self.current_file
            .as_deref()
            .is_some_and(is_system_include_path)
    }
}

impl<'a> Visitor for VMethodVisitor<'a> {
    fn enter(&mut self, node: &AstNode) -> WalkAction {
        let in_system = self.note_loc(node);
        let entering_class = node.class_name().is_some();
        if entering_class {
            // Snapshot current_file so the matching pop in `leave`
            // restores the surrounding scope's effective file.
            self.file_stack.push(self.current_file.clone());
            if in_system {
                self.system_depth += 1;
            }
        }
        if node.kind == "CXXMethodDecl" && self.system_depth == 0 {
            let is_virtual = node.r#virtual == Some(true);
            let has_override = node.has_override_attr();
            if is_virtual || has_override {
                let is_pure = node.pure == Some(true);
                if let Some(func) =
                    parse_function_decl(node, true, self.ctx, self.id_to_name, self.stack.current())
                {
                    if self.filter.map(|f| func.name.contains(f)).unwrap_or(true) {
                        let class_name = node
                            .parent_decl_context_id
                            .as_deref()
                            .and_then(|id| self.id_to_name.get(id))
                            .map(|s| s.as_str())
                            .or(self.stack.current())
                            .unwrap_or("Unknown")
                            .to_string();
                        self.out.push(InterfaceMethod {
                            class_name,
                            method: func,
                            is_pure,
                            is_override: has_override,
                            source_offset: node.source_offset(),
                        });
                    }
                }
            }
        }
        self.stack.push_if_class(node);
        WalkAction::Continue
    }
    fn leave(&mut self, node: &AstNode) {
        let was_class = node.class_name().is_some();
        if was_class {
            // Undo the system-header bookkeeping in the same order we
            // applied it on enter (snapshot first, then maybe bump).
            let was_system = self
                .current_file
                .as_deref()
                .is_some_and(is_system_include_path);
            if was_system && self.system_depth > 0 {
                self.system_depth -= 1;
            }
            self.current_file = self.file_stack.pop().unwrap_or(None);
        }
        self.stack.pop_if(was_class);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constraints::Mutability;
    use serde_json::json;

    fn parse(v: serde_json::Value) -> AstNode {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn extracts_pure_virtual_methods() {
        let ast = parse(json!({
            "kind": "TranslationUnitDecl",
            "inner": [{
                "id": "0xS",
                "kind": "CXXRecordDecl",
                "name": "IService",
                "inner": [
                    {"kind": "CXXMethodDecl", "name": "process",
                     "type": {"qualType": "bool (int) const noexcept"},
                     "virtual": true, "pure": true,
                     "inner": [{"kind": "ParmVarDecl", "name": "x", "type": {"qualType": "int"}}]},
                    {"kind": "CXXMethodDecl", "name": "status",
                     "type": {"qualType": "int () const noexcept"},
                     "virtual": true, "pure": true}
                ]
            }]
        }));
        let m = extract_virtual_methods(&ast, None).unwrap();
        assert_eq!(m.len(), 2);
        assert!(m.iter().all(|x| x.is_pure && x.class_name == "IService"));
    }

    #[test]
    fn does_not_extract_plain_methods() {
        let ast = parse(json!({
            "kind": "TranslationUnitDecl",
            "inner": [{
                "id": "0xP",
                "kind": "CXXRecordDecl",
                "name": "Plain",
                "inner": [{"kind": "CXXMethodDecl", "name": "f",
                           "type": {"qualType": "void ()"}}]
            }]
        }));
        assert!(extract_virtual_methods(&ast, None).unwrap().is_empty());
    }

    #[test]
    fn override_attr_makes_a_method_count_as_virtual() {
        let ast = parse(json!({
            "kind": "TranslationUnitDecl",
            "inner": [{
                "id": "0xD",
                "kind": "CXXRecordDecl",
                "name": "Derived",
                "inner": [{
                    "kind": "CXXMethodDecl", "name": "f",
                    "type": {"qualType": "void ()"},
                    "inner": [{"kind": "OverrideAttr"}]
                }]
            }]
        }));
        let m = extract_virtual_methods(&ast, None).unwrap();
        assert_eq!(m.len(), 1);
        assert!(m[0].is_override);
    }

    #[test]
    fn const_vs_mutable_this_propagates_to_method_info() {
        let ast = parse(json!({
            "kind": "TranslationUnitDecl",
            "inner": [{
                "id": "0xW",
                "kind": "CXXRecordDecl",
                "name": "IWidget",
                "inner": [
                    {"kind": "CXXMethodDecl", "name": "read_val",
                     "type": {"qualType": "int () const"},
                     "virtual": true, "pure": true},
                    {"kind": "CXXMethodDecl", "name": "mutate",
                     "type": {"qualType": "void ()"},
                     "virtual": true, "pure": true}
                ]
            }]
        }));
        let m = extract_virtual_methods(&ast, None).unwrap();
        let read = m.iter().find(|x| x.method.name == "read_val").unwrap();
        let mut_ = m.iter().find(|x| x.method.name == "mutate").unwrap();
        assert_eq!(read.method.params[0].mutability, Mutability::Readonly);
        assert_eq!(mut_.method.params[0].mutability, Mutability::Mutable);
    }

    #[test]
    fn vdtor_propagates_via_inheritance() {
        let ast = parse(json!({
            "kind": "TranslationUnitDecl",
            "inner": [
                {"kind": "CXXRecordDecl", "name": "Base", "inner": [
                    {"kind": "CXXDestructorDecl", "name": "~Base", "virtual": true}
                ]},
                {"kind": "CXXRecordDecl", "name": "Derived",
                 "bases": [{"type": {"qualType": "class Base"}}],
                 "inner": []}
            ]
        }));
        let s = classes_with_virtual_dtor(&ast);
        assert!(s.contains("Base"));
        assert!(s.contains("Derived"));
    }

    #[test]
    fn skips_virtual_methods_from_system_headers() {
        // Mirrors what happens with `#include <algorithm>` on MSVC: a
        // `std::exception` decl with `what()` survives the path filter
        // under a kept `namespace std { … }` wrapper. The visitor must
        // not emit a havoc spec / vtable stub for it, because the
        // resulting return-type lowering disagrees with the stub's
        // pointer signature and aborts SAW load.
        let ast = parse(json!({
            "kind": "TranslationUnitDecl",
            "inner": [
                // User class — kept.
                {
                    "id": "0xUS",
                    "kind": "CXXRecordDecl",
                    "name": "IUserService",
                    "loc": {"file": r"C:\repos\myproj\src\foo.cpp"},
                    "inner": [{
                        "kind": "CXXMethodDecl", "name": "process",
                        "type": {"qualType": "int () const"},
                        "virtual": true, "pure": true
                    }]
                },
                // System class — dropped.
                {
                    "id": "0xSE",
                    "kind": "CXXRecordDecl",
                    "name": "exception",
                    "loc": {"file": r"C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Tools\MSVC\14.40.33807\include\vcruntime_exception.h"},
                    "inner": [{
                        "kind": "CXXMethodDecl", "name": "what",
                        "type": {"qualType": "const char *() const"},
                        "virtual": true
                    }]
                }
            ]
        }));
        let m = extract_virtual_methods(&ast, None).unwrap();
        assert_eq!(m.len(), 1, "system-header virtual methods must be skipped");
        assert_eq!(m[0].method.name, "process");
    }
}
