//! Type-resolution context built by walking the AST once before any
//! per-function spec emission. Captures struct/class layout info,
//! polymorphic-class detection, enum definitions, and the `mutable`
//! field propagation needed by the const-method havoc model.

use super::cpp_types::parse_cpp_type;
use super::node::AstNode;
use super::visitor::{walk, ClassStack, Visitor, WalkAction};
use crate::constraints::{EnumVariant, TypeInfo};
use std::collections::{HashMap, HashSet};

/// Bundled type-resolution context shared by every extractor in this
/// module. Built by [`build`] and then passed by reference.
pub struct TypeContext {
    /// Struct/class name → list of `(field_name, field_type)` pairs.
    /// Excludes the implicit vptr.
    pub structs: HashMap<String, Vec<(String, TypeInfo)>>,

    /// Classes that contain at least one `mutable`-qualified data
    /// member, directly or in a base class. A `const` method on such a
    /// class can still legally write to `this` (mutable-field hole in
    /// the const havoc model).
    pub classes_with_mutable_field: HashSet<String>,

    /// Class name → list of immediate base-class names (in declaration
    /// order, with `class `/`struct ` prefixes preserved).
    pub class_bases: HashMap<String, Vec<String>>,

    /// Class name → list of own data members (excluding implicit vptr).
    /// Tuple is `(field_name, TypeInfo, default_literal)` where the
    /// literal is a Cryptol/integer literal string when the AST has an
    /// in-class initializer, or empty otherwise.
    pub class_own_fields: HashMap<String, OwnFieldList>,

    /// Set of polymorphic class names — classes whose hierarchy
    /// declares at least one `virtual` method. These layouts start
    /// with a vptr.
    pub polymorphic_classes: HashSet<String>,

    /// Enum name → `(variants, discriminant_bits)`. Each variant
    /// carries its explicit discriminant value (or the implied
    /// "previous + 1" value), so downstream constraint emission can
    /// distinguish contiguous-from-zero enums (range bound) from
    /// sparse / aliased enums (membership disjunction).
    pub enums: HashMap<String, (Vec<EnumVariant>, u32)>,
}

impl TypeContext {
    pub fn empty() -> Self {
        Self {
            structs: HashMap::new(),
            classes_with_mutable_field: HashSet::new(),
            class_bases: HashMap::new(),
            class_own_fields: HashMap::new(),
            polymorphic_classes: HashSet::new(),
            enums: HashMap::new(),
        }
    }
}

/// Build the full [`TypeContext`] for a translation unit.
///
/// Three passes are run in order:
///   1. Structs/classes: fields, bases, polymorphism, direct mutable.
///   2. Enums (after structs so enum-typed fields resolve cleanly).
///   3. Mutable-field propagation through inheritance to fixpoint.
pub fn build(ast: &AstNode) -> TypeContext {
    let mut ctx = TypeContext::empty();
    let mut sv = StructVisitor {
        ctx: &mut ctx,
        stack: ClassStack::new(),
    };
    walk(ast, &mut sv);
    let mut ev = EnumVisitor { ctx: &mut ctx };
    walk(ast, &mut ev);
    propagate_mutable_through_bases(&mut ctx);
    ctx
}

/// Bit width implied by an enum's `fixedUnderlyingType` `qualType`
/// string. Accepts both unqualified (`uint8_t`) and `std::`-qualified
/// (`std::uint8_t`) forms. Returns 32 for any unrecognized spelling.
pub fn enum_bits_from_underlying(qt: &str) -> u32 {
    let qt = qt.strip_prefix("std::").unwrap_or(qt);
    match qt {
        "uint8_t" | "int8_t" | "char" | "unsigned char" | "signed char" => 8,
        "uint16_t" | "int16_t" | "short" | "unsigned short" => 16,
        "uint64_t" | "int64_t" | "long long" | "unsigned long long" | "size_t" | "ssize_t"
        | "ptrdiff_t" | "intptr_t" | "uintptr_t" => 64,
        _ => 32,
    }
}

/// Extract the in-class initializer literal from a `FieldDecl`, when
/// present and simple (integer or boolean literal). Returns the empty
/// string otherwise.
pub fn extract_field_default_literal(field: &AstNode) -> String {
    if field.has_in_class_initializer != Some(true) {
        return String::new();
    }
    fn dfs(n: &AstNode) -> Option<String> {
        match n.kind.as_str() {
            "IntegerLiteral" => n
                .value
                .as_ref()
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            "CXXBoolLiteralExpr" => n.value.as_ref().and_then(|v| v.as_bool()).map(|b| {
                if b {
                    "1".into()
                } else {
                    "0".into()
                }
            }),
            _ => n.inner.iter().find_map(dfs),
        }
    }
    dfs(field).unwrap_or_default()
}

/// First pass: record per-class layout info as the walker enters each
/// `CXXRecordDecl`. Field types are resolved against the partial
/// `TypeContext` built so far — sufficient for the cases we care about
/// (POD / pointer-only structs and STL types resolved by name).
struct StructVisitor<'a> {
    ctx: &'a mut TypeContext,
    stack: ClassStack,
}

impl<'a> Visitor for StructVisitor<'a> {
    fn enter(&mut self, node: &AstNode) -> WalkAction {
        if node.is_record() {
            if let Some(name) = node.name.as_deref() {
                self.record_class(name, node);
            }
        }
        self.stack.push_if_class(node);
        WalkAction::Continue
    }
    fn leave(&mut self, node: &AstNode) {
        self.stack.pop_if(node.class_name().is_some());
    }
}

impl<'a> StructVisitor<'a> {
    fn record_class(&mut self, name: &str, node: &AstNode) {
        // Bases: keep the qualType verbatim (callers strip `class `/`struct `
        // prefixes as needed — historical behavior preserved).
        let bases: Vec<String> = node
            .bases
            .iter()
            .filter_map(|b| b.r#type.as_ref().and_then(|t| t.qual_type.clone()))
            .collect();
        if !bases.is_empty() {
            self.ctx.class_bases.insert(name.to_string(), bases);
        }
        let mut fields: Vec<(String, TypeInfo)> = Vec::new();
        let mut own_fields: Vec<(String, TypeInfo, String)> = Vec::new();
        let mut has_mutable_here = false;
        let mut has_virtual_here = false;
        for child in &node.inner {
            match child.kind.as_str() {
                "FieldDecl" => {
                    let fname = child.name.clone().unwrap_or_else(|| "unnamed".to_string());
                    let fty = child
                        .qual_type()
                        .map(|qt| parse_cpp_type(qt, self.ctx))
                        .unwrap_or(TypeInfo::Void);
                    if child.mutable == Some(true) {
                        has_mutable_here = true;
                    }
                    let default_lit = extract_field_default_literal(child);
                    fields.push((fname.clone(), fty.clone()));
                    own_fields.push((fname, fty, default_lit));
                }
                "CXXMethodDecl" | "CXXConstructorDecl" | "CXXDestructorDecl"
                    if child.r#virtual == Some(true) =>
                {
                    has_virtual_here = true;
                }
                _ => {}
            }
        }
        if !fields.is_empty() {
            self.ctx.structs.insert(name.to_string(), fields);
        }
        if !own_fields.is_empty() {
            self.ctx
                .class_own_fields
                .insert(name.to_string(), own_fields);
        }
        if has_mutable_here {
            self.ctx.classes_with_mutable_field.insert(name.to_string());
        }
        if has_virtual_here {
            self.ctx.polymorphic_classes.insert(name.to_string());
        }
    }
}

/// Second pass: every `EnumDecl` becomes an entry in `ctx.enums`.
struct EnumVisitor<'a> {
    ctx: &'a mut TypeContext,
}

impl<'a> Visitor for EnumVisitor<'a> {
    fn enter(&mut self, node: &AstNode) -> WalkAction {
        if node.kind != "EnumDecl" {
            return WalkAction::Continue;
        }
        let name = match node.name.as_deref() {
            Some(n) => n,
            None => return WalkAction::Continue,
        };
        let bits = node
            .fixed_underlying_type
            .as_ref()
            .and_then(|t| t.qual_type.as_deref())
            .map(enum_bits_from_underlying)
            .unwrap_or(32);
        let variants = collect_enum_variants(node);
        if !variants.is_empty() {
            self.ctx.enums.insert(name.to_string(), (variants, bits));
        } else if node.fixed_underlying_type.is_some() {
            // Forward-declared enum with explicit underlying type. Seed
            // an empty variants entry so `parse_cpp_type` produces
            // TypeInfo::Enum (which feeds the post-processing pass)
            // rather than falling through to Opaque.
            self.ctx
                .enums
                .entry(name.to_string())
                .or_insert_with(|| (Vec::new(), bits));
        }
        WalkAction::Continue
    }
}

/// Extract `(name, value)` pairs from an `EnumDecl`'s
/// `EnumConstantDecl` children.
///
/// Each `EnumConstantDecl` carries an explicit initializer expression
/// when the source spells one out (`Ok = 0`, `Denied = 100`); we look
/// for an integer literal anywhere in that subtree. When the source
/// omits an initializer, the discriminant follows the C rule of
/// "previous variant + 1, starting at 0". Tracking explicit values is
/// what lets the constraint emitter produce a membership disjunction
/// for sparse enums instead of the wrong `<= len-1` bound.
fn collect_enum_variants(enum_node: &AstNode) -> Vec<EnumVariant> {
    let mut next_implicit: i128 = 0;
    let mut out: Vec<EnumVariant> = Vec::new();
    for child in &enum_node.inner {
        if child.kind != "EnumConstantDecl" {
            continue;
        }
        let Some(vname) = child.name.clone() else {
            continue;
        };
        let value = extract_enum_constant_value(child).unwrap_or(next_implicit);
        next_implicit = value.saturating_add(1);
        out.push(EnumVariant::new(vname, value));
    }
    out
}

/// DFS the subtree of an `EnumConstantDecl` for the first
/// `IntegerLiteral` (the value of the initializer when one is
/// present). Mirrors [`extract_field_default_literal`]'s pattern.
fn extract_enum_constant_value(decl: &AstNode) -> Option<i128> {
    fn dfs(n: &AstNode) -> Option<i128> {
        if n.kind == "IntegerLiteral" {
            if let Some(s) = n.value.as_ref().and_then(|v| v.as_str()) {
                return s.parse::<i128>().ok();
            }
        }
        n.inner.iter().find_map(dfs)
    }
    decl.inner.iter().find_map(dfs)
}

/// Walk inheritance graphs to a fixed point, marking every class whose
/// base (transitively) has a mutable-field flag. Required because a
/// `const` method on a derived class can still legally modify mutable
/// fields inherited from a base.
fn propagate_mutable_through_bases(ctx: &mut TypeContext) {
    loop {
        let mut changed = false;
        let derived: Vec<String> = ctx.class_bases.keys().cloned().collect();
        for cls in derived {
            if ctx.classes_with_mutable_field.contains(&cls) {
                continue;
            }
            if let Some(bases) = ctx.class_bases.get(&cls).cloned() {
                let bases_clean: Vec<String> = bases
                    .iter()
                    .map(|b| {
                        b.trim_start_matches("class ")
                            .trim_start_matches("struct ")
                            .to_string()
                    })
                    .collect();
                if bases_clean
                    .iter()
                    .any(|b| ctx.classes_with_mutable_field.contains(b))
                {
                    ctx.classes_with_mutable_field.insert(cls);
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
}

/// `(field_name, type, default_literal)` triple describing a single
/// own data member of a C++ class.
pub type OwnField = (String, TypeInfo, String);

/// Ordered list of own data members for a class.
pub type OwnFieldList = Vec<OwnField>;

/// Compute the LLVM struct type name and ordered field list for a
/// polymorphic class. When the class adds no own fields, clang reuses
/// the topmost ancestor that owns fields as the layout type — we
/// reproduce that resolution here.
pub fn compute_class_layout(class: &str, ctx: &TypeContext) -> Option<(String, OwnFieldList)> {
    // Walk root → derived to assemble the layout chain.
    let mut chain: Vec<String> = Vec::new();
    let mut cur = class.to_string();
    let mut guard = 0u32;
    loop {
        guard += 1;
        if guard > 64 {
            break;
        }
        chain.push(cur.clone());
        let bases = match ctx.class_bases.get(&cur) {
            Some(b) if !b.is_empty() => b
                .iter()
                .map(|s| {
                    s.trim_start_matches("class ")
                        .trim_start_matches("struct ")
                        .to_string()
                })
                .collect::<Vec<_>>(),
            _ => break,
        };
        cur = bases.into_iter().next().unwrap();
    }
    chain.reverse();
    let mut all_fields: Vec<(String, TypeInfo, String)> = Vec::new();
    let mut last_with_fields: Option<String> = None;
    for c in &chain {
        if let Some(fs) = ctx.class_own_fields.get(c) {
            all_fields.extend(fs.iter().cloned());
            last_with_fields = Some(c.clone());
        }
    }
    let derived_has_own_fields = ctx
        .class_own_fields
        .get(class)
        .map(|f| !f.is_empty())
        .unwrap_or(false);
    let layout_name = if derived_has_own_fields {
        class.to_string()
    } else {
        last_with_fields.unwrap_or_else(|| class.to_string())
    };
    Some((format!("class.{layout_name}"), all_fields))
}

/// Build a map from clang node IDs (`"0x..."`) to class/struct names,
/// used to resolve `parentDeclContextId` on methods.
pub fn build_node_id_map(ast: &AstNode) -> HashMap<String, String> {
    struct V<'a>(&'a mut HashMap<String, String>);
    impl<'a> Visitor for V<'a> {
        fn enter(&mut self, n: &AstNode) -> WalkAction {
            if n.is_record() {
                if let (Some(id), Some(name)) = (n.id.as_deref(), n.name.as_deref()) {
                    self.0.insert(id.to_string(), name.to_string());
                }
            }
            WalkAction::Continue
        }
    }
    let mut out = HashMap::new();
    let mut v = V(&mut out);
    walk(ast, &mut v);
    out
}

#[cfg(test)]
#[path = "type_ctx_tests.rs"]
mod tests;
