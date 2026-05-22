//! Typed Rust view of clang's `-ast-dump=json` output.
//!
//! Clang emits a recursive JSON tree where each node has a `kind` string
//! tag and a flat bag of kind-specific fields. The historical approach in
//! this crate was to pass `serde_json::Value` around and reach into named
//! fields with `.get("...")` calls — easy to write but hostile to refactor
//! and impossible for the compiler to typecheck.
//!
//! [`AstNode`] models the union of fields we actually inspect across all
//! kinds, with `Option<T>` for fields that only appear on some kinds and
//! `extra` (a `serde_json::Map`) for everything else.  No information is
//! discarded — round-tripping is preserved via [`serde(flatten)`].

use serde::Deserialize;
use serde_json::{Map, Value};

/// A single node in the clang JSON AST.
///
/// The set of typed fields is intentionally focused: it covers the
/// declarations, expressions, types, attributes and source locations
/// touched by this crate. Anything not modeled lands in [`AstNode::extra`]
/// and remains accessible via raw JSON access if a future extractor
/// needs it.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AstNode {
    /// AST node kind tag (e.g. `"FunctionDecl"`, `"CXXRecordDecl"`,
    /// `"AnnotateAttr"`). Always present on real clang output; defaults
    /// to the empty string for nodes synthesized by the parser shim.
    #[serde(default)]
    pub kind: String,

    /// Stable per-node hex id (e.g. `"0x1ffd7063098"`). Used by
    /// `referencedDecl`, `parentDeclContextId`, etc. to cross-link nodes.
    #[serde(default)]
    pub id: Option<String>,

    /// Declared name (functions, parameters, fields, classes, enums...).
    #[serde(default)]
    pub name: Option<String>,

    /// ABI-mangled symbol name on `FunctionDecl` / `VarDecl` / methods.
    #[serde(default, rename = "mangledName")]
    pub mangled_name: Option<String>,

    /// `type.qualType` payload on value-bearing nodes.
    #[serde(default)]
    pub r#type: Option<TypeRef>,

    /// Base-class list on `CXXRecordDecl` / `ClassTemplateSpecializationDecl`.
    #[serde(default)]
    pub bases: Vec<BaseRef>,

    /// Child nodes. Recursion happens here.
    #[serde(default)]
    pub inner: Vec<AstNode>,

    /// `virtual: true` on virtual methods/destructors. **Note**: clang
    /// does NOT set this on `override`-marked overrides — check for an
    /// `OverrideAttr` child as a fallback.
    #[serde(default)]
    pub r#virtual: Option<bool>,

    /// `pure: true` on pure-virtual methods (`= 0`).
    #[serde(default)]
    pub pure: Option<bool>,

    /// `mutable: true` on `FieldDecl` — relaxes the const-method havoc
    /// model.
    #[serde(default)]
    pub mutable: Option<bool>,

    /// `isImplicit: true` on synthesized decls (e.g. implicit dtors).
    #[serde(default, rename = "isImplicit")]
    pub is_implicit: Option<bool>,

    /// e.g. `"extern"`, `"static"` on `VarDecl`.
    #[serde(default, rename = "storageClass")]
    pub storage_class: Option<String>,

    /// `FieldDecl.hasInClassInitializer`.
    #[serde(default, rename = "hasInClassInitializer")]
    pub has_in_class_initializer: Option<bool>,

    /// `EnumDecl.fixedUnderlyingType` for `enum class E : uint32_t` etc.
    #[serde(default, rename = "fixedUnderlyingType")]
    pub fixed_underlying_type: Option<TypeRef>,

    /// `FunctionDecl.exceptionSpec` — alternative location for noexcept
    /// info when the spec isn't baked into `qualType`.
    #[serde(default, rename = "exceptionSpec", deserialize_with = "deserialize_exception_spec")]
    pub exception_spec: Option<ExceptionSpec>,

    /// Source location of the primary token. Used to filter system
    /// headers and to order virtual methods by declaration site.
    #[serde(default)]
    pub loc: Option<SourceLoc>,

    /// Half-open source byte range. Carries the SAL macro `expansionLoc`
    /// for `AnnotateAttr` nodes.
    #[serde(default)]
    pub range: Option<SourceRange>,

    /// `CXXMethodDecl.parentDeclContextId` (sometimes omitted by clang
    /// — visitors should fall back to the enclosing-class stack).
    #[serde(default, rename = "parentDeclContextId")]
    pub parent_decl_context_id: Option<String>,

    /// Payload for literal / annotation nodes: `IntegerLiteral.value` is
    /// a numeric string, `CXXBoolLiteralExpr.value` is a bool,
    /// `AnnotateAttr.value` is the macro source text (when present).
    #[serde(default)]
    pub value: Option<Value>,

    /// `referencedDecl` summary on `CallExpr` / `DeclRefExpr` / `MemberExpr`.
    #[serde(default, rename = "referencedDecl")]
    pub referenced_decl: Option<Box<AstNode>>,

    /// ID-only callee reference on `MemberExpr`.
    #[serde(default, rename = "referencedMemberDecl")]
    pub referenced_member_decl: Option<String>,

    /// ID-only ctor reference on `CXXConstructExpr`.
    #[serde(default, rename = "constructorDecl")]
    pub constructor_decl: Option<String>,

    /// ID-only dtor reference on `CXXDeleteExpr` etc.
    #[serde(default, rename = "destructorDecl")]
    pub destructor_decl: Option<String>,

    /// `CXXNewExpr.operatorNewDecl` resolved allocator summary.
    #[serde(default, rename = "operatorNewDecl")]
    pub operator_new_decl: Option<Box<AstNode>>,

    /// `CXXDeleteExpr.operatorDeleteDecl` resolved deallocator summary.
    #[serde(default, rename = "operatorDeleteDecl")]
    pub operator_delete_decl: Option<Box<AstNode>>,

    /// All other fields, preserved verbatim. Lets us round-trip without
    /// losing information for unmodeled clang extensions.
    #[serde(flatten)]
    #[allow(dead_code)]
    pub extra: Map<String, Value>,
}

/// Wrapper for the `type` object that holds `qualType`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TypeRef {
    #[serde(default, rename = "qualType")]
    pub qual_type: Option<String>,
    #[serde(flatten)]
    #[allow(dead_code)]
    pub extra: Map<String, Value>,
}

/// One entry in a `CXXRecordDecl.bases` array.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct BaseRef {
    #[serde(default)]
    pub r#type: Option<TypeRef>,
    #[serde(flatten)]
    #[allow(dead_code)]
    pub extra: Map<String, Value>,
}

/// `FunctionDecl.exceptionSpec` shape.
///
/// Clang 20+ may emit this as a bare string `"noexcept"` instead of an
/// object `{"type":"noexcept",...}`.  The custom deserializer on
/// `AstNode::exception_spec` handles both forms.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ExceptionSpec {
    #[serde(default)]
    pub r#type: Option<String>,
    #[serde(flatten)]
    #[allow(dead_code)]
    pub extra: Map<String, Value>,
}

/// Accept `"noexcept"` (string) or `{"type":"noexcept",...}` (object).
fn deserialize_exception_spec<'de, D>(deserializer: D) -> Result<Option<ExceptionSpec>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let val = Option::<Value>::deserialize(deserializer)?;
    match val {
        None => Ok(None),
        Some(Value::String(s)) => Ok(Some(ExceptionSpec {
            r#type: Some(s),
            extra: Map::new(),
        })),
        Some(Value::Object(map)) => {
            let spec = ExceptionSpec {
                r#type: map.get("type").and_then(|v| v.as_str()).map(String::from),
                extra: map,
            };
            Ok(Some(spec))
        }
        Some(_) => Ok(None),
    }
}

/// A clang source location.  Some fields are only set on specific node
/// kinds (e.g. `expansionLoc` on macro-expanded attributes,
/// `includedFrom` at the top of an `#include` chain).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SourceLoc {
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub offset: Option<u64>,
    #[serde(default, rename = "includedFrom")]
    pub included_from: Option<Box<SourceLoc>>,
    #[serde(default, rename = "expansionLoc")]
    pub expansion_loc: Option<Box<SourceLoc>>,
    #[serde(default, rename = "tokLen")]
    pub tok_len: Option<u64>,
    #[serde(flatten)]
    #[allow(dead_code)]
    pub extra: Map<String, Value>,
}

/// A source byte range with begin/end locations.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SourceRange {
    #[serde(default)]
    pub begin: Option<SourceLoc>,
    #[serde(default)]
    #[allow(dead_code)]
    pub end: Option<SourceLoc>,
    #[serde(flatten)]
    #[allow(dead_code)]
    pub extra: Map<String, Value>,
}

impl AstNode {
    /// Returns `node.type.qualType` if present.
    pub fn qual_type(&self) -> Option<&str> {
        self.r#type.as_ref()?.qual_type.as_deref()
    }

    /// True for `CXXRecordDecl`, `RecordDecl`, and class template
    /// specializations — the node kinds that define a struct/class layout.
    pub fn is_record(&self) -> bool {
        matches!(
            self.kind.as_str(),
            "CXXRecordDecl" | "ClassTemplateSpecializationDecl" | "RecordDecl"
        )
    }

    /// Returns the class/struct name when this node defines a class layout
    /// (`CXXRecordDecl` or template specialization). Excludes plain C
    /// `RecordDecl` to match the historical behavior of context tracking.
    pub fn class_name(&self) -> Option<&str> {
        if matches!(
            self.kind.as_str(),
            "CXXRecordDecl" | "ClassTemplateSpecializationDecl"
        ) {
            self.name.as_deref()
        } else {
            None
        }
    }

    /// True for the function-like declaration kinds we want to walk into.
    pub fn is_function_like(&self) -> bool {
        matches!(
            self.kind.as_str(),
            "FunctionDecl"
                | "CXXMethodDecl"
                | "CXXConstructorDecl"
                | "CXXDestructorDecl"
                | "CXXConversionDecl"
        )
    }

    /// True when an `OverrideAttr` child is present.  Used to detect
    /// virtual-via-override methods on which clang omits `"virtual": true`.
    pub fn has_override_attr(&self) -> bool {
        self.inner.iter().any(|c| c.kind == "OverrideAttr")
    }

    /// Source offset for ordering nodes by declaration site. Prefers
    /// `loc.offset` (always present on user-written decls) and falls back
    /// through `range.begin.offset` for implicit/synthesized nodes.
    pub fn source_offset(&self) -> u64 {
        if let Some(off) = self.loc.as_ref().and_then(|l| l.offset) {
            return off;
        }
        self.range
            .as_ref()
            .and_then(|r| r.begin.as_ref())
            .and_then(|b| b.offset)
            .unwrap_or(0)
    }

    /// Walk the `inner` array as the seed for visitor traversal.
    #[allow(dead_code)]
    pub fn children(&self) -> &[AstNode] {
        &self.inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_function_decl_minimal_shape() {
        let v = json!({
            "kind": "FunctionDecl",
            "id": "0xdead",
            "name": "foo",
            "mangledName": "_Z3foov",
            "type": { "qualType": "void ()" },
            "inner": []
        });
        let n: AstNode = serde_json::from_value(v).unwrap();
        assert_eq!(n.kind, "FunctionDecl");
        assert_eq!(n.id.as_deref(), Some("0xdead"));
        assert_eq!(n.name.as_deref(), Some("foo"));
        assert_eq!(n.mangled_name.as_deref(), Some("_Z3foov"));
        assert_eq!(n.qual_type(), Some("void ()"));
        assert!(n.inner.is_empty());
        assert!(n.is_function_like());
    }

    #[test]
    fn unknown_fields_are_preserved_in_extra() {
        let v = json!({
            "kind": "FunctionDecl",
            "name": "foo",
            "newFutureClangField": { "a": 1 }
        });
        let n: AstNode = serde_json::from_value(v).unwrap();
        assert!(n.extra.contains_key("newFutureClangField"));
    }

    #[test]
    fn class_name_excludes_plain_record() {
        let cxx: AstNode = serde_json::from_value(json!({
            "kind": "CXXRecordDecl",
            "name": "Foo"
        }))
        .unwrap();
        assert_eq!(cxx.class_name(), Some("Foo"));

        let plain: AstNode = serde_json::from_value(json!({
            "kind": "RecordDecl",
            "name": "FooCStruct"
        }))
        .unwrap();
        assert_eq!(plain.class_name(), None);
        assert!(plain.is_record());
    }

    #[test]
    fn override_attr_detected_via_child() {
        let n: AstNode = serde_json::from_value(json!({
            "kind": "CXXMethodDecl",
            "name": "foo",
            "inner": [{ "kind": "OverrideAttr" }]
        }))
        .unwrap();
        assert!(n.has_override_attr());
    }

    #[test]
    fn source_offset_falls_back_to_range_begin() {
        let n: AstNode = serde_json::from_value(json!({
            "kind": "CXXMethodDecl",
            "range": { "begin": { "offset": 1234 } }
        }))
        .unwrap();
        assert_eq!(n.source_offset(), 1234);
    }

    #[test]
    fn expansion_loc_round_trips_through_nested_struct() {
        let n: AstNode = serde_json::from_value(json!({
            "kind": "AnnotateAttr",
            "range": {
                "begin": {
                    "expansionLoc": {
                        "file": "C:\\foo.h",
                        "offset": 100,
                        "tokLen": 5
                    }
                }
            }
        }))
        .unwrap();
        let exp = n
            .range
            .as_ref()
            .and_then(|r| r.begin.as_ref())
            .and_then(|b| b.expansion_loc.as_ref())
            .expect("expansionLoc parsed");
        assert_eq!(exp.file.as_deref(), Some("C:\\foo.h"));
        assert_eq!(exp.offset, Some(100));
        assert_eq!(exp.tok_len, Some(5));
    }

    #[test]
    fn referenced_decl_recursion_is_boxed() {
        let n: AstNode = serde_json::from_value(json!({
            "kind": "DeclRefExpr",
            "referencedDecl": {
                "kind": "FunctionDecl",
                "id": "0xabc",
                "name": "callee",
                "mangledName": "_Z6calleev"
            }
        }))
        .unwrap();
        let r = n.referenced_decl.as_ref().expect("referencedDecl parsed");
        assert_eq!(r.kind, "FunctionDecl");
        assert_eq!(r.name.as_deref(), Some("callee"));
    }

    #[test]
    fn base_refs_parse() {
        let n: AstNode = serde_json::from_value(json!({
            "kind": "CXXRecordDecl",
            "name": "Derived",
            "bases": [
                { "type": { "qualType": "class Base" } },
                { "type": { "qualType": "IFace" } }
            ]
        }))
        .unwrap();
        assert_eq!(n.bases.len(), 2);
        assert_eq!(
            n.bases[0].r#type.as_ref().and_then(|t| t.qual_type.as_deref()),
            Some("class Base")
        );
    }
}
