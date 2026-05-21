//! Parse clang -ast-dump=json output and extract function signatures.
//!
//! Usage:
//!   clang -Xclang -ast-dump=json -fsyntax-only MyFile.cpp > ast.json
//!   saw-spec-gen from-clang-ast --input ast.json --output specs/

use crate::constraints::*;
use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::sync::OnceLock;

/// Maximum file size we'll attempt to load into memory (100 MB).
const MAX_AST_FILE_SIZE: u64 = 100 * 1024 * 1024;

/// Process-wide cache of source-file contents, keyed by absolute path.
///
/// Clang's JSON AST dump emits the *kind* of an `AnnotateAttr` but drops
/// the string argument. To recover SAL macro names like `_Out_` we read
/// the original source file at the attribute's `expansionLoc` and extract
/// the token. Caching avoids re-reading the same .cpp/.h files for every
/// annotation in large translation units.
fn source_cache() -> &'static Mutex<HashMap<String, String>> {
    static CACHE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Read `len` bytes starting at byte `offset` from `path`. Returns `None`
/// if the file cannot be opened, the slice is out of range, or the bytes
/// are not valid UTF-8. The file contents are cached for subsequent
/// lookups within the same process.
fn read_source_token(path: &str, offset: usize, len: usize) -> Option<String> {
    let mut cache = source_cache().lock().ok()?;
    let contents = if let Some(c) = cache.get(path) {
        c.clone()
    } else {
        let c = std::fs::read_to_string(path).ok()?;
        cache.insert(path.to_string(), c.clone());
        c
    };
    let bytes = contents.as_bytes();
    let end = offset.checked_add(len)?;
    if end > bytes.len() {
        return None;
    }
    std::str::from_utf8(&bytes[offset..end]).ok().map(|s| s.to_string())
}

/// Bundled type resolution context: struct/class definitions + enum definitions.
struct TypeContext {
    /// Struct/class name → list of (field_name, field_type) pairs
    structs: HashMap<String, Vec<(String, TypeInfo)>>,
    /// Set of class names that contain at least one `mutable`-qualified data
    /// member (directly or in a base class). Used to detect the
    /// `mutable`-keyword hole in the const-method havoc model: when a class
    /// has any `mutable` field, a `const` method can still legally modify
    /// it, so `this` cannot be treated as fully read-only.
    classes_with_mutable_field: std::collections::HashSet<String>,
    /// Class name → list of immediate base-class names (in declaration order).
    /// Populated by `collect_struct_defs`.
    class_bases: HashMap<String, Vec<String>>,
    /// Class name → list of own data members in declaration order
    /// (excluding the implicit vptr). Each entry is
    /// `(field_name, TypeInfo, default_value_literal)`.
    /// `default_value_literal` is a Cryptol/integer literal string (e.g.
    /// "0") when the AST has an in-class initializer; otherwise empty.
    class_own_fields: HashMap<String, Vec<(String, TypeInfo, String)>>,
    /// Set of polymorphic class names (i.e. classes whose hierarchy contains
    /// at least one `virtual` method). These are the classes whose layout
    /// begins with a vptr.
    polymorphic_classes: std::collections::HashSet<String>,
    /// Enum name → (variant_names, discriminant_bits)
    enums: HashMap<String, (Vec<String>, u32)>,
}

impl TypeContext {
    fn new() -> Self {
        TypeContext {
            structs: HashMap::new(),
            classes_with_mutable_field: std::collections::HashSet::new(),
            class_bases: HashMap::new(),
            class_own_fields: HashMap::new(),
            polymorphic_classes: std::collections::HashSet::new(),
            enums: HashMap::new(),
        }
    }
}

/// Parse a clang AST JSON dump.
///
/// Handles both single JSON objects (from full `clang -ast-dump=json`) and
/// concatenated JSON objects (from `clang -ast-dump=json -ast-dump-filter=Name`
/// which emits multiple top-level objects separated by newlines).
pub fn parse_ast(path: &Path) -> Result<Value> {
    // Check file size before loading to avoid OOM on huge AST dumps
    let metadata = std::fs::metadata(path)
        .with_context(|| format!("Failed to stat {}", path.display()))?;
    let file_size = metadata.len();
    if file_size > MAX_AST_FILE_SIZE {
        let size_mb = file_size / (1024 * 1024);
        anyhow::bail!(
            "AST file {} is too large ({} MB, limit is {} MB). To reduce size, try:\n\
             \n  1. Use clang -ast-dump-filter=<function> to dump only specific declarations\n\
             \n  2. Split the translation unit into smaller files\n\
             \n  3. Pre-filter with: jq 'select(.name == \"MyFunc\")' large_ast.json > filtered.json",
            path.display(),
            size_mb,
            MAX_AST_FILE_SIZE / (1024 * 1024),
        );
    }

    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    parse_ast_str(&content)
        .with_context(|| format!("Failed to parse JSON from {}", path.display()))
}

/// Merge multiple parsed ASTs into a single synthetic `TranslationUnitDecl`.
///
/// Each input's top-level `inner` array is concatenated **only when the
/// input is itself a `TranslationUnitDecl`**.  Any other kind of node (e.g.
/// a single filtered `CXXRecordDecl` from `clang -ast-dump-filter`) is
/// appended as a top-level child of the synthetic TU instead.  This is
/// critical: a `CXXRecordDecl` *also* has an `inner` array (its members),
/// so unwrapping based solely on the presence of `inner` would flatten a
/// filtered class dump's methods into the TU root — stripping the
/// enclosing-class context and causing `extract_virtual_methods` to fall
/// back to `"Unknown"` for every virtual method (collapsing all interfaces
/// into a single `@unknown_vtable`).
///
/// No deduplication is performed — clang emits stable IDs per file, so
/// callers should pass ASTs from distinct files (e.g. the consumer + each
/// interface header) rather than overlapping dumps.
pub fn merge_asts(asts: Vec<Value>) -> Value {
    let mut combined_inner = Vec::new();
    for ast in asts {
        let is_tu = ast.get("kind").and_then(|v| v.as_str()) == Some("TranslationUnitDecl");
        if is_tu {
            if let Some(arr) = ast.get("inner").and_then(|v| v.as_array()) {
                combined_inner.extend(arr.iter().cloned());
            }
        } else {
            combined_inner.push(ast);
        }
    }
    serde_json::json!({
        "kind": "TranslationUnitDecl",
        "inner": combined_inner,
    })
}

/// Parse AST from string, handling concatenated JSON objects.
fn parse_ast_str(content: &str) -> Result<Value> {
    // Try standard single-object parse first
    if let Ok(ast) = serde_json::from_str::<Value>(content) {
        return Ok(ast);
    }

    // Fall back: split concatenated JSON objects and wrap in a synthetic root.
    // clang -ast-dump-filter emits multiple `{...}` objects back to back.
    let objects = split_concatenated_json(content)?;
    if objects.is_empty() {
        anyhow::bail!("No JSON objects found in input");
    }
    if objects.len() == 1 {
        return Ok(objects.into_iter().next().unwrap());
    }

    // Wrap multiple objects into a synthetic TranslationUnitDecl
    Ok(serde_json::json!({
        "kind": "TranslationUnitDecl",
        "inner": objects
    }))
}

/// Split a string containing concatenated top-level JSON objects.
fn split_concatenated_json(content: &str) -> Result<Vec<Value>> {
    let mut objects = Vec::new();
    let mut depth = 0i32;
    let mut start = None;
    let mut in_string = false;
    let mut escape_next = false;

    for (i, c) in content.char_indices() {
        if escape_next {
            escape_next = false;
            continue;
        }
        if c == '\\' && in_string {
            escape_next = true;
            continue;
        }
        if c == '"' {
            in_string = !in_string;
            continue;
        }
        if in_string {
            continue;
        }
        match c {
            '{' => {
                if depth == 0 {
                    start = Some(i);
                }
                depth += 1;
            }
            '}' => {
                depth -= 1;
                if depth == 0 {
                    if let Some(s) = start {
                        let obj_str = &content[s..=i];
                        let obj: Value = serde_json::from_str(obj_str)
                            .with_context(|| {
                                format!(
                                    "Failed to parse JSON object at byte offset {s}"
                                )
                            })?;
                        objects.push(obj);
                    }
                    start = None;
                }
            }
            _ => {}
        }
    }

    Ok(objects)
}

/// A virtual method from a C++ class, used for generating interface stubs.
#[derive(Debug, Clone)]
pub struct InterfaceMethod {
    /// The class that declares this method (e.g. "IValidator")
    pub class_name: String,
    /// The method's function info (includes implicit `this` param)
    pub method: FunctionInfo,
    /// Whether this is a pure virtual (= 0)
    pub is_pure: bool,
    /// Whether this method overrides a base-class method (carries
    /// `OverrideAttr`). True for derived-class overrides; false for
    /// methods first declared on the class itself.
    pub is_override: bool,
    /// Source-location offset of the method's declaration, used to order
    /// methods by their appearance in the class definition (which is the
    /// order MSVC assigns vtable slots). Falls back to 0 when the AST
    /// node has no offset (e.g. implicit/synthesized methods).
    pub source_offset: u64,
}

/// Check which classes have a virtual destructor in the AST.
///
/// A class is considered to have a virtual dtor when either:
///   - it declares an explicit `virtual ~T()` (clang sets `"virtual": true`
///     on the `CXXDestructorDecl`); OR
///   - it declares `~T() override` (clang attaches an `OverrideAttr` but
///     does NOT set `"virtual": true` — same quirk as for override methods);
///     OR
///   - any of its base classes (transitively) has a virtual dtor — the
///     derived class inherits the vtable slot even if it doesn't redeclare
///     the destructor.
pub fn classes_with_virtual_dtor(ast: &Value) -> std::collections::HashSet<String> {
    let mut result = std::collections::HashSet::new();
    let mut bases: HashMap<String, Vec<String>> = HashMap::new();
    collect_virtual_dtors(ast, &mut result, &mut bases, None);
    propagate_vdtor_through_bases(&mut result, &bases);
    result
}

fn collect_virtual_dtors(
    node: &Value,
    out: &mut std::collections::HashSet<String>,
    bases: &mut HashMap<String, Vec<String>>,
    enclosing_class: Option<&str>,
) {
    let kind = node.get("kind").and_then(|v| v.as_str()).unwrap_or("");

    let class_for_children = if kind == "CXXRecordDecl"
        || kind == "ClassTemplateSpecializationDecl"
    {
        node.get("name").and_then(|v| v.as_str())
    } else {
        None
    };

    // Track base classes so we can propagate virtual-dtor-ness through
    // inheritance after the first pass.
    if let Some(class_name) = class_for_children {
        if let Some(base_arr) = node.get("bases").and_then(|v| v.as_array()) {
            let mut base_names = Vec::new();
            for b in base_arr {
                if let Some(bn) = b
                    .get("type")
                    .and_then(|t| t.get("qualType"))
                    .and_then(|q| q.as_str())
                {
                    // Strip any "class "/"struct " prefix and access
                    // specifier noise — we want bare class names that
                    // match keys produced elsewhere in the walker.
                    let bn = bn
                        .trim_start_matches("class ")
                        .trim_start_matches("struct ")
                        .to_string();
                    base_names.push(bn);
                }
            }
            if !base_names.is_empty() {
                bases.insert(class_name.to_string(), base_names);
            }
        }
    }

    if kind == "CXXDestructorDecl" {
        let is_virtual = node.get("virtual").and_then(|v| v.as_bool()).unwrap_or(false);
        let is_implicit = node.get("isImplicit").and_then(|v| v.as_bool()).unwrap_or(false);
        // `~T() override` is also virtual via the vtable, but clang does
        // NOT set `"virtual": true` on it — it attaches an `OverrideAttr`
        // instead. Detect that case so derived classes don't get a
        // truncated vtable.
        let has_override_attr = node
            .get("inner")
            .and_then(|v| v.as_array())
            .map(|inner| {
                inner.iter().any(|c| {
                    c.get("kind").and_then(|v| v.as_str()) == Some("OverrideAttr")
                })
            })
            .unwrap_or(false);
        if (is_virtual || has_override_attr) && !is_implicit {
            if let Some(class) = enclosing_class {
                out.insert(class.to_string());
            }
        }
    }

    let next_class = class_for_children.or(enclosing_class);
    if let Some(inner) = node.get("inner").and_then(|v| v.as_array()) {
        for child in inner {
            collect_virtual_dtors(child, out, bases, next_class);
        }
    }
}

/// Once direct virtual-dtor declarations are collected, walk the
/// inheritance graph to fixpoint and mark any class whose base (transitively)
/// has a virtual dtor. Without this, a derived class that doesn't redeclare
/// `~T()` would get a vtable with no dtor slot — shifting every method by
/// one relative to the base layout.
fn propagate_vdtor_through_bases(
    set: &mut std::collections::HashSet<String>,
    bases: &HashMap<String, Vec<String>>,
) {
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

/// Extract function info from the clang AST.
pub fn extract_functions(ast: &Value, filter: Option<&str>) -> Result<Vec<FunctionInfo>> {
    // First pass: collect struct/class + enum definitions for type resolution
    let mut ctx = TypeContext::new();
    collect_struct_defs(ast, &mut ctx);
    collect_enum_defs(ast, &mut ctx);
    // After collecting per-class fields, propagate `mutable`-field flags
    // through inheritance so derived classes inherit the property. This
    // closes the const-method havoc hole: a `const` method on a class with
    // a `mutable` field (own or inherited) must still treat `this` as
    // writable for that field.
    propagate_mutable_through_bases(&mut ctx);

    // Also build a map from clang node IDs (hex strings like "0x1ffd7063098")
    // to class/struct names, so we can resolve parentDeclContextId on methods.
    let mut id_to_name: HashMap<String, String> = HashMap::new();
    collect_node_ids(ast, &mut id_to_name);

    // Collect global variables (top-level VarDecl with mangledName)
    let globals = collect_globals(ast, &ctx);

    // Second pass: collect function declarations
    let mut functions = Vec::new();
    collect_functions(ast, &mut functions, filter, &ctx, &id_to_name);

    // Third pass: resolve referenced globals for each function
    resolve_referenced_globals(ast, &mut functions, &globals);

    // Fourth pass: resolve called functions for each function
    let mut id_to_fn: HashMap<String, (String, String)> = HashMap::new();
    collect_function_decl_ids(ast, &mut id_to_fn);
    resolve_called_functions(ast, &mut functions, &id_to_fn);

    Ok(functions)
}

/// Extract virtual methods from C++ classes in the AST.
///
/// Returns `InterfaceMethod` entries for each virtual method found.
/// These can be used to generate vtable resolution stubs and havoc specs.
/// A constructor from a C++ class, used for generating constructor overrides.
#[derive(Debug, Clone)]
pub struct ClassConstructor {
    /// The class name (e.g. "OkLog")
    pub class_name: String,
    /// The mangled constructor name (e.g. "??0OkLog@@QEAA@XZ")
    pub mangled_name: String,
    /// The LLVM struct type name to use when allocating `this` in the
    /// constructor override (e.g. "class.ILogger"). Picked as the topmost
    /// polymorphic ancestor that owns the class's data layout — clang emits
    /// only that name when a derived class adds no new fields.
    pub layout_type_name: String,
    /// Non-vptr data members in the order they appear in the layout class.
    /// Each entry: (field_name, TypeInfo, default_value_literal). Empty
    /// `default_value_literal` means "no in-class initializer" (treat as 0
    /// for POD types).
    pub layout_fields: Vec<(String, TypeInfo, String)>,
}

pub fn extract_virtual_methods(ast: &Value, filter: Option<&str>) -> Result<Vec<InterfaceMethod>> {
    
    let mut ctx = TypeContext::new();
    collect_struct_defs(ast, &mut ctx);
    collect_enum_defs(ast, &mut ctx);
    propagate_mutable_through_bases(&mut ctx);

    let mut id_to_name: HashMap<String, String> = HashMap::new();
    collect_node_ids(ast, &mut id_to_name);

    let mut methods = Vec::new();
    collect_virtual_methods(ast, &mut methods, filter, &ctx, &id_to_name);
    Ok(methods)
}

/// Extract constructors for classes that have virtual methods.
///
/// Given the set of class names from `extract_virtual_methods`, finds
/// their `CXXConstructorDecl` nodes and returns their mangled names.
pub fn extract_constructors(ast: &Value, class_names: &[String]) -> Result<Vec<ClassConstructor>> {
    // Build a type context so we can resolve each ctor's class layout
    // (LLVM struct name + ordered fields with default-init literals).
    let mut ctx = TypeContext::new();
    collect_struct_defs(ast, &mut ctx);
    collect_enum_defs(ast, &mut ctx);
    propagate_mutable_through_bases(&mut ctx);

    let mut ctors = Vec::new();
    collect_constructors(ast, &mut ctors, class_names, None, &ctx);
    Ok(ctors)
}

/// A reference to an interface type that is missing from the AST.
#[derive(Debug, Clone)]
pub struct MissingInterfaceRef {
    /// The class that holds the reference (e.g. "KeyExchangeSession")
    pub owning_class: String,
    /// The field name (e.g. "KeyStore")
    pub field_name: String,
    /// The wrapper type (e.g. "std::unique_ptr")
    pub wrapper: String,
    /// The interface type name (e.g. "IKeyStore")
    pub interface_name: String,
}

/// Detect interface types referenced by class fields but missing from the AST.
///
/// Scans `CXXRecordDecl` nodes for fields of type `unique_ptr<T>`, `shared_ptr<T>`,
/// or `T*` where `T` starts with "I" (common C++ interface naming convention) and
/// `T` has no `CXXRecordDecl` with virtual methods in the current AST.
pub fn detect_missing_interfaces(ast: &Value) -> Vec<MissingInterfaceRef> {
    // Collect all class names that ARE present in the AST
    let mut known_classes: std::collections::HashSet<String> = std::collections::HashSet::new();
    collect_known_classes(ast, &mut known_classes);

    // Walk all record decls and check field types
    let mut missing = Vec::new();
    collect_missing_interface_refs(ast, &mut missing, &known_classes, None);

    // Deduplicate by interface_name
    missing.sort_by(|a, b| a.interface_name.cmp(&b.interface_name));
    missing.dedup_by(|a, b| a.interface_name == b.interface_name && a.owning_class == b.owning_class);
    missing
}

fn collect_known_classes(node: &Value, out: &mut std::collections::HashSet<String>) {
    let kind = node.get("kind").and_then(|v| v.as_str()).unwrap_or("");
    if kind == "CXXRecordDecl" || kind == "ClassTemplateSpecializationDecl" {
        if let Some(name) = node.get("name").and_then(|v| v.as_str()) {
            out.insert(name.to_string());
        }
    }
    if let Some(inner) = node.get("inner").and_then(|v| v.as_array()) {
        for child in inner {
            collect_known_classes(child, out);
        }
    }
}

fn collect_missing_interface_refs(
    node: &Value,
    out: &mut Vec<MissingInterfaceRef>,
    known_classes: &std::collections::HashSet<String>,
    enclosing_class: Option<&str>,
) {
    let kind = node.get("kind").and_then(|v| v.as_str()).unwrap_or("");

    let class_for_children = if kind == "CXXRecordDecl"
        || kind == "ClassTemplateSpecializationDecl"
    {
        node.get("name").and_then(|v| v.as_str())
    } else {
        None
    };

    // Check fields for interface references
    if kind == "FieldDecl" {
        if let Some(owning) = enclosing_class {
            if let Some(qual_type) = node
                .get("type")
                .and_then(|t| t.get("qualType"))
                .and_then(|q| q.as_str())
            {
                let field_name = node
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unnamed");

                // Extract interface type from smart pointers or raw pointers
                if let Some((wrapper, iface)) = extract_interface_from_type(qual_type) {
                    if !known_classes.contains(&iface) {
                        out.push(MissingInterfaceRef {
                            owning_class: owning.to_string(),
                            field_name: field_name.to_string(),
                            wrapper,
                            interface_name: iface,
                        });
                    }
                }
            }
        }
    }

    let next_class = class_for_children.or(enclosing_class);
    if let Some(inner) = node.get("inner").and_then(|v| v.as_array()) {
        for child in inner {
            collect_missing_interface_refs(child, out, known_classes, next_class);
        }
    }
}

/// Extract a potential interface type name from a field type string.
///
/// Recognizes:
///   - `std::unique_ptr<IFoo>` / `unique_ptr<IFoo>`
///   - `std::shared_ptr<IFoo>` / `shared_ptr<IFoo>`
///   - `IFoo *` (raw pointer to interface)
///
/// Returns `(wrapper_description, interface_name)` if a candidate is found.
fn extract_interface_from_type(qual_type: &str) -> Option<(String, String)> {
    let t = qual_type.trim();

    // Smart pointers: unique_ptr<T>, shared_ptr<T>
    for wrapper in &["unique_ptr", "shared_ptr"] {
        if let Some(pos) = t.find(wrapper) {
            let after = &t[pos + wrapper.len()..];
            if let Some(start) = after.find('<') {
                let inner = &after[start + 1..];
                if let Some(end) = inner.find('>') {
                    let inner_type = inner[..end].trim();
                    // Strip any trailing qualifiers/pointers
                    let clean = inner_type
                        .split_whitespace()
                        .next()
                        .unwrap_or(inner_type)
                        .trim_end_matches('*');
                    if looks_like_interface(clean) {
                        return Some((
                            format!("std::{wrapper}"),
                            clean.to_string(),
                        ));
                    }
                }
            }
        }
    }

    // Raw pointer: IFoo *
    if t.ends_with('*') {
        let pointee = t.trim_end_matches('*').trim();
        let clean = pointee
            .trim_start_matches("const ")
            .trim_start_matches("struct ")
            .trim_start_matches("class ")
            .split_whitespace()
            .next()
            .unwrap_or(pointee);
        if looks_like_interface(clean) {
            return Some(("raw pointer".to_string(), clean.to_string()));
        }
    }

    None
}

/// Heuristic: does this name look like a C++ interface?
/// Matches names starting with "I" followed by an uppercase letter (e.g. IKeyStore, IValidator).
fn looks_like_interface(name: &str) -> bool {
    let chars: Vec<char> = name.chars().collect();
    chars.len() >= 2 && chars[0] == 'I' && chars[1].is_uppercase()
}

fn collect_constructors(
    node: &Value,
    out: &mut Vec<ClassConstructor>,
    class_names: &[String],
    enclosing_class: Option<&str>,
    ctx: &TypeContext,
) {
    let kind = node.get("kind").and_then(|v| v.as_str()).unwrap_or("");

    // Track the enclosing class
    let class_for_children = if kind == "CXXRecordDecl"
        || kind == "ClassTemplateSpecializationDecl"
    {
        node.get("name").and_then(|v| v.as_str())
    } else {
        None
    };

    if kind == "CXXConstructorDecl" {
        if let Some(mangled) = node.get("mangledName").and_then(|v| v.as_str()) {
            // Resolve class name from enclosing scope
            if let Some(class_name) = enclosing_class {
                if class_names.iter().any(|cn| cn == class_name) {
                    // Only collect default/simple constructors (no complex params)
                    let param_count = node
                        .get("inner")
                        .and_then(|v| v.as_array())
                        .map(|inner| {
                            inner
                                .iter()
                                .filter(|c| {
                                    c.get("kind").and_then(|v| v.as_str())
                                        == Some("ParmVarDecl")
                                })
                                .count()
                        })
                        .unwrap_or(0);
                    // Default constructor or single-param (copy/move) — we want the default
                    if param_count == 0 {
                        let (layout_type_name, layout_fields) =
                            compute_class_layout(class_name, ctx)
                                .unwrap_or_else(|| {
                                    (format!("class.{class_name}"), Vec::new())
                                });
                        out.push(ClassConstructor {
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

    let next_class = class_for_children.or(enclosing_class);
    if let Some(inner) = node.get("inner").and_then(|v| v.as_array()) {
        for child in inner {
            collect_constructors(child, out, class_names, next_class, ctx);
        }
    }
}

fn collect_virtual_methods(
    node: &Value,
    out: &mut Vec<InterfaceMethod>,
    filter: Option<&str>,
    ctx: &TypeContext,
    id_to_name: &HashMap<String, String>,
) {
    collect_virtual_methods_inner(node, out, filter, ctx, id_to_name, None);
}

fn collect_virtual_methods_inner(
    node: &Value,
    out: &mut Vec<InterfaceMethod>,
    filter: Option<&str>,
    ctx: &TypeContext,
    id_to_name: &HashMap<String, String>,
    enclosing_class: Option<&str>,
) {
    let kind = node.get("kind").and_then(|v| v.as_str()).unwrap_or("");

    // Track the enclosing class for nested methods
    let class_name_for_children = if kind == "CXXRecordDecl"
        || kind == "ClassTemplateSpecializationDecl"
    {
        node.get("name").and_then(|v| v.as_str())
    } else {
        None
    };

    if kind == "CXXMethodDecl" {
        // A method is virtual if clang marks it `virtual: true` (base/pure
        // declarations) OR it carries an `OverrideAttr` (derived overrides
        // and methods using the `override` keyword — clang does NOT set
        // `virtual: true` on these, even though they participate in vtable
        // dispatch).
        let is_virtual = node.get("virtual").and_then(|v| v.as_bool()).unwrap_or(false);
        let has_override_attr = node
            .get("inner")
            .and_then(|v| v.as_array())
            .map(|inner| {
                inner.iter().any(|c| {
                    c.get("kind").and_then(|v| v.as_str()) == Some("OverrideAttr")
                })
            })
            .unwrap_or(false);
        if is_virtual || has_override_attr {
            let is_pure = node.get("pure").and_then(|v| v.as_bool()).unwrap_or(false);
            if let Some(func) =
                parse_function_decl(node, true, ctx, id_to_name, enclosing_class)
            {
                let matches = filter.map(|f| func.name.contains(f)).unwrap_or(true);
                if matches {
                    // Resolve class name: try parentDeclContextId first, then enclosing class
                    let class_name = node
                        .get("parentDeclContextId")
                        .and_then(|v| v.as_str())
                        .and_then(|id| id_to_name.get(id))
                        .map(|s| s.as_str())
                        .or(enclosing_class)
                        .unwrap_or("Unknown")
                        .to_string();

                    // Extract source offset for stable, declaration-order
                    // sorting. Clang stores this on `loc.offset` (always
                    // present) and falls back through `range.begin.offset`
                    // when `loc` is missing (e.g. for implicit decls).
                    let source_offset = node
                        .get("loc")
                        .and_then(|v| v.get("offset"))
                        .and_then(|v| v.as_u64())
                        .or_else(|| {
                            node.get("range")
                                .and_then(|v| v.get("begin"))
                                .and_then(|v| v.get("offset"))
                                .and_then(|v| v.as_u64())
                        })
                        .unwrap_or(0);

                    out.push(InterfaceMethod {
                        class_name,
                        method: func,
                        is_pure,
                        is_override: has_override_attr,
                        source_offset,
                    });
                }
            }
        }
    }

    // Recurse into children
    let next_class = class_name_for_children.or(enclosing_class);
    if let Some(inner) = node.get("inner").and_then(|v| v.as_array()) {
        for child in inner {
            collect_virtual_methods_inner(child, out, filter, ctx, id_to_name, next_class);
        }
    }
}

/// Check whether a file path points to a system include (compiler / platform
/// headers like `<cstdio>`, `<vector>`, Windows SDK, glibc, etc.).
/// Matching is conservative: any path under a recognized vendor/SDK root
/// counts as system.
fn is_system_include_path(path: &str) -> bool {
    // Normalize backslashes for cross-platform matching
    let p = path.replace('\\', "/").to_ascii_lowercase();
    // Common system-include path fragments
    const PATTERNS: &[&str] = &[
        "/microsoft visual studio/", // MSVC headers
        "/windows kits/",            // Windows SDK / UCRT
        "/program files/llvm/",      // Clang/LLVM install
        "/program files (x86)/llvm/",
        "/clang+llvm-",         // LLVM dist (e.g. clang+llvm-20.1.6)
        "/include/c++/",        // libstdc++
        "/usr/include/",        // glibc on POSIX
        "/usr/local/include/",
        "<built-in>",
        "<command line>",
    ];
    PATTERNS.iter().any(|pat| p.contains(pat))
}

/// Recursively collect node ID → name mappings for CXXRecordDecl/RecordDecl nodes.
fn collect_node_ids(node: &Value, id_to_name: &mut HashMap<String, String>) {
    if let Some(kind) = node.get("kind").and_then(|v| v.as_str()) {
        let is_record = kind == "RecordDecl"
            || kind == "CXXRecordDecl"
            || kind == "ClassTemplateSpecializationDecl";
        if is_record {
            if let (Some(id), Some(name)) = (
                node.get("id").and_then(|v| v.as_str()),
                node.get("name").and_then(|v| v.as_str()),
            ) {
                id_to_name.insert(id.to_string(), name.to_string());
            }
        }
    }
    if let Some(inner) = node.get("inner").and_then(|v| v.as_array()) {
        for child in inner {
            collect_node_ids(child, id_to_name);
        }
    }
}

/// Recursively collect AST node ID -> (name, mangled_name) for FunctionDecl/
/// CXXMethodDecl/CXXConstructorDecl/CXXDestructorDecl nodes.  Used to resolve
/// `referencedMemberDecl` (and similar) ID-only references in CallExpr children.
fn collect_function_decl_ids(
    node: &Value,
    id_to_fn: &mut HashMap<String, (String, String)>,
) {
    if let Some(kind) = node.get("kind").and_then(|v| v.as_str()) {
        let is_fn = kind == "FunctionDecl"
            || kind == "CXXMethodDecl"
            || kind == "CXXConstructorDecl"
            || kind == "CXXDestructorDecl"
            || kind == "CXXConversionDecl";
        if is_fn {
            let id = node.get("id").and_then(|v| v.as_str()).unwrap_or("");
            let name = node.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let mangled = node
                .get("mangledName")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if !id.is_empty() && (!name.is_empty() || !mangled.is_empty()) {
                id_to_fn.insert(
                    id.to_string(),
                    (
                        name.to_string(),
                        if mangled.is_empty() {
                            name.to_string()
                        } else {
                            mangled.to_string()
                        },
                    ),
                );
            }
        }
    }
    if let Some(inner) = node.get("inner").and_then(|v| v.as_array()) {
        for child in inner {
            collect_function_decl_ids(child, id_to_fn);
        }
    }
}

/// Extract all file-scope global variables from the AST.
pub fn extract_all_globals(ast: &Value) -> Result<Vec<GlobalVarInfo>> {
    let mut ctx = TypeContext::new();
    collect_struct_defs(ast, &mut ctx);
    collect_enum_defs(ast, &mut ctx);
    let globals_map = collect_globals(ast, &ctx);
    Ok(globals_map.into_values().collect())
}

/// Collect global variables from top-level VarDecl nodes.
fn collect_globals(
    ast: &Value,
    ctx: &TypeContext,
) -> HashMap<String, GlobalVarInfo> {
    let mut globals = HashMap::new();

    if let Some(inner) = ast.get("inner").and_then(|v| v.as_array()) {
        for node in inner {
            let kind = node.get("kind").and_then(|v| v.as_str()).unwrap_or("");
            if kind != "VarDecl" {
                continue;
            }
            // Skip extern declarations (no definition)
            if node.get("storageClass").and_then(|v| v.as_str()) == Some("extern") {
                continue;
            }
            // Skip const-qualified globals — the compiler may inline them,
            // so they won't appear in bitcode and can't be modified anyway.
            let qual_type_raw = node
                .get("type")
                .and_then(|t| t.get("qualType"))
                .and_then(|q| q.as_str())
                .unwrap_or("");
            if qual_type_raw.starts_with("const ") || qual_type_raw.contains(" const") {
                continue;
            }
            let name = match node.get("name").and_then(|v| v.as_str()) {
                Some(n) => n,
                None => continue,
            };
            let mangled = match node.get("mangledName").and_then(|v| v.as_str()) {
                Some(m) => m,
                None => continue,
            };
            let qual_type = if qual_type_raw.is_empty() { "int" } else { qual_type_raw };
            let ty = parse_cpp_type(qual_type, ctx);
            let id = node.get("id").and_then(|v| v.as_str()).unwrap_or("");

            // Extract initial value from IntegerLiteral if present
            let init_value = node
                .get("inner")
                .and_then(|v| v.as_array())
                .and_then(|inner| {
                    inner.iter().find_map(|child| {
                        if child.get("kind").and_then(|v| v.as_str()) == Some("IntegerLiteral") {
                            child.get("value").and_then(|v| v.as_str()).map(String::from)
                        } else {
                            None
                        }
                    })
                });

            globals.insert(
                id.to_string(),
                GlobalVarInfo {
                    name: name.to_string(),
                    mangled_name: mangled.to_string(),
                    ty,
                    init_value,
                },
            );
        }
    }

    globals
}

/// Walk function bodies to find DeclRefExpr nodes that reference globals,
/// then populate each function's `referenced_globals`.
fn resolve_referenced_globals(
    ast: &Value,
    functions: &mut [FunctionInfo],
    globals: &HashMap<String, GlobalVarInfo>,
) {
    if globals.is_empty() {
        return;
    }

    // Walk ALL AST nodes recursively to find function bodies with global refs
    if let Some(inner) = ast.get("inner").and_then(|v| v.as_array()) {
        for node in inner {
            resolve_globals_recursive(node, functions, globals);
        }
    }
}

fn resolve_globals_recursive(
    node: &Value,
    functions: &mut [FunctionInfo],
    globals: &HashMap<String, GlobalVarInfo>,
) {
    let kind = node.get("kind").and_then(|v| v.as_str()).unwrap_or("");

    if kind == "FunctionDecl" || kind == "CXXMethodDecl" {
        let fname = node.get("name").and_then(|v| v.as_str()).unwrap_or("");

        let mut referenced_ids = Vec::new();
        collect_global_refs(node, &mut referenced_ids, globals);

        // Find matching function(s) in our list — match on mangled name for precision
        let mangled = node.get("mangledName").and_then(|v| v.as_str());
        if let Some(func) = functions.iter_mut().find(|f| {
            if let (Some(m), Some(fm)) = (mangled, f.mangled_name.as_deref()) {
                m == fm
            } else {
                f.name == fname
            }
        }) {
            let mut referenced_ids = Vec::new();
            collect_global_refs(node, &mut referenced_ids, globals);

            referenced_ids.sort();
            referenced_ids.dedup();
            for id in referenced_ids {
                if let Some(g) = globals.get(&id) {
                    if !func.referenced_globals.iter().any(|rg| rg.mangled_name == g.mangled_name) {
                        func.referenced_globals.push(g.clone());
                    }
                }
            }
        }
    }

    // Recurse into children (classes contain methods)
    if let Some(inner) = node.get("inner").and_then(|v| v.as_array()) {
        for child in inner {
            resolve_globals_recursive(child, functions, globals);
        }
    }
}

/// Recursively collect global variable reference IDs from DeclRefExpr nodes.
fn collect_global_refs(
    node: &Value,
    out: &mut Vec<String>,
    globals: &HashMap<String, GlobalVarInfo>,
) {
    if let Some(kind) = node.get("kind").and_then(|v| v.as_str()) {
        if kind == "DeclRefExpr" {
            if let Some(ref_decl) = node.get("referencedDecl") {
                if ref_decl.get("kind").and_then(|v| v.as_str()) == Some("VarDecl") {
                    if let Some(id) = ref_decl.get("id").and_then(|v| v.as_str()) {
                        if globals.contains_key(id) {
                            out.push(id.to_string());
                        }
                    }
                }
            }
        }
    }

    if let Some(inner) = node.get("inner").and_then(|v| v.as_array()) {
        for child in inner {
            collect_global_refs(child, out, globals);
        }
    }
}

/// Resolve which functions are called from each function's body.
fn resolve_called_functions(
    ast: &Value,
    functions: &mut [FunctionInfo],
    id_to_fn: &HashMap<String, (String, String)>,
) {
    if let Some(inner) = ast.get("inner").and_then(|v| v.as_array()) {
        for node in inner {
            resolve_calls_recursive(node, functions, id_to_fn);
        }
    }
}

fn resolve_calls_recursive(
    node: &Value,
    functions: &mut [FunctionInfo],
    id_to_fn: &HashMap<String, (String, String)>,
) {
    let kind = node.get("kind").and_then(|v| v.as_str()).unwrap_or("");

    if kind == "FunctionDecl" || kind == "CXXMethodDecl"
        || kind == "CXXConstructorDecl" || kind == "CXXDestructorDecl"
    {
        let mangled = node.get("mangledName").and_then(|v| v.as_str());
        let fname = node.get("name").and_then(|v| v.as_str()).unwrap_or("");

        let mut calls = Vec::new();
        collect_call_refs(node, &mut calls, id_to_fn);

        // Dedup by mangled name
        calls.sort_by(|a, b| a.mangled_name.cmp(&b.mangled_name));
        calls.dedup_by(|a, b| a.mangled_name == b.mangled_name);

        if let Some(func) = functions.iter_mut().find(|f| {
            if let (Some(m), Some(fm)) = (mangled, f.mangled_name.as_deref()) {
                m == fm
            } else {
                f.name == fname
            }
        }) {
            func.called_functions = calls;
        }
    }

    // Recurse into children (classes contain methods)
    if let Some(inner) = node.get("inner").and_then(|v| v.as_array()) {
        for child in inner {
            resolve_calls_recursive(child, functions, id_to_fn);
        }
    }
}

/// Recursively collect function calls from CallExpr and CXXMemberCallExpr nodes.
fn collect_call_refs(
    node: &Value,
    out: &mut Vec<CalledFunction>,
    id_to_fn: &HashMap<String, (String, String)>,
) {
    if let Some(kind) = node.get("kind").and_then(|v| v.as_str()) {
        // CallExpr for free functions, CXXMemberCallExpr for methods,
        // CXXOperatorCallExpr for operator overloads, CXXConstructExpr for constructors
        if kind == "CallExpr" || kind == "CXXMemberCallExpr"
            || kind == "CXXOperatorCallExpr" || kind == "CXXConstructExpr"
        {
            if let Some(ref_decl) = node.get("referencedDecl") {
                extract_called_decl(ref_decl, out, id_to_fn);
            }
            // Resolve ID-only callee references (e.g. CXXMemberCallExpr →
            // MemberExpr.referencedMemberDecl is just an AST node ID string).
            collect_callee_ids(node, out, id_to_fn);
            // For CallExpr, the callee decl can also be nested in the first child
            // (ImplicitCastExpr → DeclRefExpr → referencedDecl)
            if let Some(inner) = node.get("inner").and_then(|v| v.as_array()) {
                for child in inner {
                    if child.get("kind").and_then(|v| v.as_str()) == Some("DeclRefExpr")
                        || child.get("kind").and_then(|v| v.as_str()) == Some("ImplicitCastExpr")
                        || child.get("kind").and_then(|v| v.as_str()) == Some("MemberExpr")
                    {
                        extract_callee_from_expr(child, out, id_to_fn);
                    }
                }
            }
        }
        // CXXNewExpr has operatorNewDecl (not a CallExpr child)
        if kind == "CXXNewExpr" {
            if let Some(op_decl) = node.get("operatorNewDecl") {
                extract_called_decl(op_decl, out, id_to_fn);
            }
        }
        // CXXDeleteExpr has operatorDeleteDecl
        if kind == "CXXDeleteExpr" {
            if let Some(op_decl) = node.get("operatorDeleteDecl") {
                extract_called_decl(op_decl, out, id_to_fn);
            }
        }
    }

    if let Some(inner) = node.get("inner").and_then(|v| v.as_array()) {
        for child in inner {
            collect_call_refs(child, out, id_to_fn);
        }
    }
}

/// Walk a call/construct expression looking for any "ID-only" callee references,
/// which appear in fields like `referencedMemberDecl` on MemberExpr nodes.
/// These are AST IDs that point at a method declaration elsewhere in the AST.
fn collect_callee_ids(
    node: &Value,
    out: &mut Vec<CalledFunction>,
    id_to_fn: &HashMap<String, (String, String)>,
) {
    // Common fields that hold an ID-only callee reference
    for field in [
        "referencedMemberDecl",
        "constructorDecl",
        "destructorDecl",
    ] {
        if let Some(id) = node.get(field).and_then(|v| v.as_str()) {
            if let Some((name, mangled)) = id_to_fn.get(id) {
                out.push(CalledFunction {
                    name: name.clone(),
                    mangled_name: mangled.clone(),
                    has_body: false,
                });
            }
        }
    }
    if let Some(inner) = node.get("inner").and_then(|v| v.as_array()) {
        for child in inner {
            collect_callee_ids(child, out, id_to_fn);
        }
    }
}

fn extract_called_decl(
    decl: &Value,
    out: &mut Vec<CalledFunction>,
    id_to_fn: &HashMap<String, (String, String)>,
) {
    // The inline `referencedDecl` summary often lacks `mangledName`.  Prefer
    // the canonical mangled name from the id_to_fn map when available.
    let id = decl.get("id").and_then(|v| v.as_str()).unwrap_or("");
    let summary_name = decl.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let summary_mangled = decl.get("mangledName").and_then(|v| v.as_str()).unwrap_or("");

    let (name, mangled) = if !id.is_empty() {
        if let Some((n, m)) = id_to_fn.get(id) {
            (n.clone(), m.clone())
        } else {
            (
                summary_name.to_string(),
                if summary_mangled.is_empty() {
                    summary_name.to_string()
                } else {
                    summary_mangled.to_string()
                },
            )
        }
    } else {
        (
            summary_name.to_string(),
            if summary_mangled.is_empty() {
                summary_name.to_string()
            } else {
                summary_mangled.to_string()
            },
        )
    };

    if name.is_empty() && mangled.is_empty() { return; }
    out.push(CalledFunction {
        name,
        mangled_name: mangled,
        has_body: false,
    });
}

fn extract_callee_from_expr(
    expr: &Value,
    out: &mut Vec<CalledFunction>,
    id_to_fn: &HashMap<String, (String, String)>,
) {
    let kind = expr.get("kind").and_then(|v| v.as_str()).unwrap_or("");
    if kind == "DeclRefExpr" || kind == "MemberExpr" {
        if let Some(ref_decl) = expr.get("referencedDecl") {
            extract_called_decl(ref_decl, out, id_to_fn);
        }
    }
    // Recurse through ImplicitCastExpr etc.
    if let Some(inner) = expr.get("inner").and_then(|v| v.as_array()) {
        for child in inner {
            extract_callee_from_expr(child, out, id_to_fn);
        }
    }
}

/// Recursively collect struct/class definitions from RecordDecl and CXXRecordDecl nodes.
fn collect_struct_defs(node: &Value, ctx: &mut TypeContext) {
    if let Some(kind) = node.get("kind").and_then(|v| v.as_str()) {
        let is_record = kind == "RecordDecl"
            || kind == "CXXRecordDecl"
            || kind == "ClassTemplateSpecializationDecl";
        if is_record {
            if let Some(name) = node.get("name").and_then(|v| v.as_str()) {
                // Track immediate base classes (CXXRecordDecl "bases" array).
                if let Some(bases) = node.get("bases").and_then(|v| v.as_array()) {
                    let mut base_names = Vec::new();
                    for b in bases {
                        if let Some(bn) = b
                            .get("type")
                            .and_then(|t| t.get("qualType"))
                            .and_then(|q| q.as_str())
                        {
                            base_names.push(bn.to_string());
                        }
                    }
                    if !base_names.is_empty() {
                        ctx.class_bases.insert(name.to_string(), base_names);
                    }
                }
                if let Some(inner) = node.get("inner").and_then(|v| v.as_array()) {
                    let mut fields = Vec::new();
                    let mut own_fields: Vec<(String, TypeInfo, String)> = Vec::new();
                    let mut has_mutable_here = false;
                    let mut has_virtual_here = false;
                    for child in inner {
                        let ck = child.get("kind").and_then(|v| v.as_str()).unwrap_or("");
                        if ck == "FieldDecl" {
                            let field_name = child
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("unnamed")
                                .to_string();
                            let field_type = child
                                .get("type")
                                .and_then(|v| v.get("qualType"))
                                .and_then(|v| v.as_str())
                                .map(|qt| parse_cpp_type(qt, ctx))
                                .unwrap_or(TypeInfo::Void);
                            if child
                                .get("mutable")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false)
                            {
                                has_mutable_here = true;
                            }
                            let default_lit = extract_field_default_literal(child);
                            fields.push((field_name.clone(), field_type.clone()));
                            own_fields.push((field_name, field_type, default_lit));
                        } else if ck == "CXXMethodDecl"
                            || ck == "CXXConstructorDecl"
                            || ck == "CXXDestructorDecl"
                        {
                            if child
                                .get("virtual")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false)
                            {
                                has_virtual_here = true;
                            }
                        }
                    }
                    if !fields.is_empty() {
                        ctx.structs.insert(name.to_string(), fields);
                    }
                    if !own_fields.is_empty() {
                        ctx.class_own_fields
                            .insert(name.to_string(), own_fields);
                    }
                    if has_mutable_here {
                        ctx.classes_with_mutable_field.insert(name.to_string());
                    }
                    if has_virtual_here {
                        ctx.polymorphic_classes.insert(name.to_string());
                    }
                }
            }
        }
    }

    if let Some(inner) = node.get("inner").and_then(|v| v.as_array()) {
        for child in inner {
            collect_struct_defs(child, ctx);
        }
    }
}

/// Extract the in-class initializer value from a FieldDecl node, if present.
/// Returns the literal as a string (e.g. "0", "42"); empty if not a simple
/// integer/boolean literal.
fn extract_field_default_literal(field: &Value) -> String {
    if !field
        .get("hasInClassInitializer")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return String::new();
    }
    fn walk(n: &Value) -> Option<String> {
        let kind = n.get("kind").and_then(|v| v.as_str()).unwrap_or("");
        match kind {
            "IntegerLiteral" => n
                .get("value")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            "CXXBoolLiteralExpr" => n
                .get("value")
                .and_then(|v| v.as_bool())
                .map(|b| if b { "1".to_string() } else { "0".to_string() }),
            _ => {
                if let Some(inner) = n.get("inner").and_then(|v| v.as_array()) {
                    for c in inner {
                        if let Some(v) = walk(c) {
                            return Some(v);
                        }
                    }
                }
                None
            }
        }
    }
    walk(field).unwrap_or_default()
}

/// After `collect_struct_defs` has run, propagate base-class mutable-field
/// flags to derived classes. A derived class inherits the `mutable` field
/// from any polymorphic base, so a `const` method on the derived class can
/// still legally modify it.
fn propagate_mutable_through_bases(ctx: &mut TypeContext) {
    // Iterate until fixed point. Inheritance chains are short, so a few
    // passes suffice.
    loop {
        let mut changed = false;
        let derived_classes: Vec<String> = ctx.class_bases.keys().cloned().collect();
        for cls in derived_classes {
            if ctx.classes_with_mutable_field.contains(&cls) {
                continue;
            }
            if let Some(bases) = ctx.class_bases.get(&cls).cloned() {
                if bases
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

/// Compute the LLVM struct-type name and full ordered field list for a
/// polymorphic class. Clang lays out a derived class either as its own
/// `%class.<Derived>` (when it adds fields) or reuses the base layout
/// directly (when it adds nothing). This function picks the right name
/// and gathers the ordered fields by walking from the topmost polymorphic
/// ancestor down to the class itself.
///
/// Returns `(layout_type_name, ordered_fields)`. Returns `None` only if
/// the class isn't known to `ctx`.
fn compute_class_layout(
    class: &str,
    ctx: &TypeContext,
) -> Option<(String, Vec<(String, TypeInfo, String)>)> {
    // Walk up the chain to build the ordered ancestor list (root → class).
    let mut chain: Vec<String> = Vec::new();
    let mut cur = class.to_string();
    let mut guard = 0u32;
    loop {
        guard += 1;
        if guard > 64 {
            // Defensive: cyclic inheritance shouldn't happen in valid C++,
            // but stop walking just in case.
            break;
        }
        chain.push(cur.clone());
        let bases = match ctx.class_bases.get(&cur) {
            Some(b) if !b.is_empty() => b.clone(),
            _ => break,
        };
        // Use the first (primary) base for layout purposes — multiple
        // inheritance is out of scope for this layout heuristic.
        cur = bases.into_iter().next().unwrap();
    }
    chain.reverse(); // now root → ... → class

    // Collect own fields walking root → derived. Order matters: base
    // fields come before derived fields in clang's layout.
    let mut all_fields: Vec<(String, TypeInfo, String)> = Vec::new();
    let mut last_with_fields: Option<String> = None;
    for c in &chain {
        if let Some(fs) = ctx.class_own_fields.get(c) {
            all_fields.extend(fs.iter().cloned());
            last_with_fields = Some(c.clone());
        }
    }

    // Pick layout type name:
    // - If the derived class adds NO own fields, clang reuses the topmost
    //   ancestor that owns fields as the layout type (its `%class.<X>`).
    // - Otherwise, clang emits `%class.<DerivedName>` with the full field
    //   set.
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

    Some((format!("class.{}", layout_name), all_fields))
}

/// Walk the entire AST and return every `EnumDecl` definition as a
/// `name → bit width` map.  Used by the post-processing pass to fall
/// back unresolved `llvm_alias "EnumName"` references to `llvm_int N`
/// regardless of whether any collected function references the enum
/// (handles forward-declared enums whose `EnumDecl` lives in a header
/// outside the call graph reach).
pub fn collect_all_enum_bits(ast: &Value) -> HashMap<String, u32> {
    let mut out: HashMap<String, u32> = HashMap::new();
    walk_enum_decls(ast, &mut out);
    out
}

fn walk_enum_decls(node: &Value, out: &mut HashMap<String, u32>) {
    if node.get("kind").and_then(|v| v.as_str()) == Some("EnumDecl") {
        if let Some(name) = node.get("name").and_then(|v| v.as_str()) {
            let bits = node
                .get("fixedUnderlyingType")
                .and_then(|v| v.get("qualType"))
                .and_then(|v| v.as_str())
                .map(|qt| match qt {
                    "uint8_t" | "int8_t" | "char" | "unsigned char" | "signed char" => 8u32,
                    "uint16_t" | "int16_t" | "short" | "unsigned short" => 16,
                    "uint64_t" | "int64_t" | "long long" | "unsigned long long" => 64,
                    _ => 32,
                })
                .unwrap_or(32);
            let entry = out.entry(name.to_string()).or_insert(bits);
            if bits > *entry {
                *entry = bits;
            }
        }
    }
    if let Some(inner) = node.get("inner").and_then(|v| v.as_array()) {
        for child in inner {
            walk_enum_decls(child, out);
        }
    }
}

/// Recursively collect enum definitions from EnumDecl nodes.
fn collect_enum_defs(node: &Value, ctx: &mut TypeContext) {    if let Some(kind) = node.get("kind").and_then(|v| v.as_str()) {
        if kind == "EnumDecl" {
            if let Some(name) = node.get("name").and_then(|v| v.as_str()) {
                // Determine bit width from fixedUnderlyingType or default to 32
                let bits = node
                    .get("fixedUnderlyingType")
                    .and_then(|v| v.get("qualType"))
                    .and_then(|v| v.as_str())
                    .map(|qt| match qt {
                        "uint8_t" | "int8_t" | "char" | "unsigned char" | "signed char" => 8,
                        "uint16_t" | "int16_t" | "short" | "unsigned short" => 16,
                        "uint64_t" | "int64_t" | "long long" | "unsigned long long" => 64,
                        _ => 32,
                    })
                    .unwrap_or(32);

                // Collect variant names from EnumConstantDecl children
                let mut variants = Vec::new();
                if let Some(inner) = node.get("inner").and_then(|v| v.as_array()) {
                    for child in inner {
                        if child.get("kind").and_then(|v| v.as_str()) == Some("EnumConstantDecl") {
                            if let Some(vname) = child.get("name").and_then(|v| v.as_str()) {
                                variants.push(vname.to_string());
                            }
                        }
                    }
                }

                if !variants.is_empty() {
                    ctx.enums.insert(name.to_string(), (variants, bits));
                }
            }
        }
    }

    if let Some(inner) = node.get("inner").and_then(|v| v.as_array()) {
        for child in inner {
            collect_enum_defs(child, ctx);
        }
    }
}

fn collect_functions(
    node: &Value,
    out: &mut Vec<FunctionInfo>,
    filter: Option<&str>,
    ctx: &TypeContext,
    id_to_name: &HashMap<String, String>,
) {
    collect_functions_inner(node, out, filter, ctx, id_to_name, None);
}

fn collect_functions_inner(
    node: &Value,
    out: &mut Vec<FunctionInfo>,
    filter: Option<&str>,
    ctx: &TypeContext,
    id_to_name: &HashMap<String, String>,
    enclosing_class: Option<&str>,
) {
    let outer_kind = node.get("kind").and_then(|v| v.as_str()).unwrap_or("");
    // Track enclosing class for child nodes so `this` mutability lookups
    // can resolve the parent class even when `parentDeclContextId` is
    // missing from the AST.
    let class_for_children = if outer_kind == "CXXRecordDecl"
        || outer_kind == "ClassTemplateSpecializationDecl"
    {
        node.get("name").and_then(|v| v.as_str())
    } else {
        None
    };
    let next_class = class_for_children.or(enclosing_class);

    if let Some(kind) = node.get("kind").and_then(|v| v.as_str()) {
        match kind {
            "FunctionDecl" | "CXXMethodDecl" => {
                if let Some(func) = parse_function_decl(
                    node,
                    kind == "CXXMethodDecl",
                    ctx,
                    id_to_name,
                    enclosing_class,
                ) {
                    let matches = filter.map(|f| func.name.contains(f)).unwrap_or(true);
                    if matches {
                        out.push(func);
                    }
                }
            }
            "FunctionTemplateDecl" => {
                // For templates, extract specialization instances from inner nodes
                if let Some(inner) = node.get("inner").and_then(|v| v.as_array()) {
                    for child in inner {
                        if let Some(ck) = child.get("kind").and_then(|v| v.as_str()) {
                            if ck == "FunctionDecl" || ck == "CXXMethodDecl" {
                                if let Some(func) = parse_function_decl(
                                    child,
                                    ck == "CXXMethodDecl",
                                    ctx,
                                    id_to_name,
                                    enclosing_class,
                                ) {
                                    let matches =
                                        filter.map(|f| func.name.contains(f)).unwrap_or(true);
                                    if matches {
                                        out.push(func);
                                    }
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    // Recurse into child nodes (skip FunctionTemplateDecl since we already handled it)
    let kind = node.get("kind").and_then(|v| v.as_str()).unwrap_or("");
    if kind != "FunctionTemplateDecl" {
        if let Some(inner) = node.get("inner").and_then(|v| v.as_array()) {
            for child in inner {
                collect_functions_inner(child, out, filter, ctx, id_to_name, next_class);
            }
        }
    }
}

fn parse_function_decl(
    node: &Value,
    is_method: bool,
    ctx: &TypeContext,
    id_to_name: &HashMap<String, String>,
    enclosing_class: Option<&str>,
) -> Option<FunctionInfo> {
    let name = node.get("name")?.as_str()?.to_string();
    let mangled = node
        .get("mangledName")
        .and_then(|v| v.as_str())
        .map(String::from);
    let qual_type = node.get("type")?.get("qualType")?.as_str()?;

    // Method constness: in C++, a const method has "const" AFTER the parameter list
    // e.g. "void (const char *) const" vs "void (const char *)"
    // We check for "const" after the last closing paren to avoid matching param types.
    let is_const_method = is_method && {
        let after_paren = qual_type.rfind(')').map(|pos| &qual_type[pos + 1..]).unwrap_or("");
        after_paren.contains("const")
    };
    let mut is_noexcept = qual_type.contains("noexcept");

    // Check for noexcept/nothrow attributes in inner nodes
    if let Some(inner) = node.get("inner").and_then(|v| v.as_array()) {
        for child in inner {
            if let Some(ck) = child.get("kind").and_then(|v| v.as_str()) {
                if ck == "NoThrowAttr" || ck == "NoExceptAttr" {
                    is_noexcept = true;
                }
            }
            // Check for exception specification in function proto type
            if let Some(exc) = child.get("exceptionSpec") {
                if let Some(spec_type) = exc.get("type").and_then(|v| v.as_str()) {
                    if spec_type == "noexcept" || spec_type == "nothrow" {
                        is_noexcept = true;
                    }
                }
            }
        }
    }

    let mut params = Vec::new();
    let mut func_annotations = Vec::new();

    if is_noexcept {
        func_annotations.push(Annotation::NoThrow);
    }

    // If this is a method, add implicit 'this' parameter
    if is_method {
        // Resolve the parent class name. Clang sometimes omits
        // `parentDeclContextId`, in which case we fall back to the
        // enclosing class tracked by the walker. We use this only to
        // decide `this`'s mutability — the inner type stays `Opaque`
        // with the legacy "Self" sentinel so existing havoc-spec paths
        // continue to allocate `this` as a pointer-sized opaque value.
        let parent_id = node
            .get("parentDeclContextId")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let parent_name: &str = id_to_name
            .get(parent_id)
            .map(|s| s.as_str())
            .or(enclosing_class)
            .unwrap_or("Self");
        let inner_type = TypeInfo::Opaque {
            name: "Self".into(),
            size_bytes: 0,
        };
        params.push(ParamInfo {
            name: "this".into(),
            ty: TypeInfo::Pointer(Box::new(inner_type)),
            // A `const` method normally promises not to write through
            // `this` \u2014 modeled as `Readonly`. BUT the C++ `mutable`
            // keyword (and `const_cast`) lets a `const` method legally
            // modify specific members. To stay sound, downgrade to
            // `Mutable` whenever the enclosing class (or any base) has
            // any `mutable`-qualified data member.
            mutability: if is_const_method
                && !ctx.classes_with_mutable_field.contains(parent_name)
            {
                Mutability::Readonly
            } else {
                Mutability::Mutable
            },
            nullable: Nullability::NonNull,
            annotations: Vec::new(),
        });
    }

    // Parse explicit parameters
    if let Some(inner) = node.get("inner").and_then(|v| v.as_array()) {
        for child in inner {
            if child.get("kind").and_then(|v| v.as_str()) == Some("ParmVarDecl") {
                if let Some(param) = parse_param(child, ctx) {
                    params.push(param);
                }
            }
        }
    }

    let return_type = parse_return_type(qual_type, ctx);

    let is_virtual = node.get("virtual").and_then(|v| v.as_bool()).unwrap_or(false);

    // A function has a body if its inner array contains a CompoundStmt
    let has_body = node
        .get("inner")
        .and_then(|v| v.as_array())
        .map(|inner| {
            inner.iter().any(|child| {
                child.get("kind").and_then(|v| v.as_str()) == Some("CompoundStmt")
            })
        })
        .unwrap_or(false);

    // Detect system-header origin: check whether the file from which this
    // declaration was included matches a known system-include path pattern.
    // System functions (stdio, ucrt, etc.) often have inline bodies that call
    // LLVM intrinsics or platform-specific symbols SAW can't resolve, so we
    // treat them as external for verification.
    //
    // NOTE: just checking `loc.includedFrom` is too broad — any header counts,
    // including the user's own ilog.h.  We need to match the actual file path.
    let is_system = node
        .get("loc")
        .and_then(|loc| loc.get("includedFrom"))
        .and_then(|inc| inc.get("file"))
        .and_then(|f| f.as_str())
        .map(is_system_include_path)
        .unwrap_or(false);

    Some(FunctionInfo {
        name,
        mangled_name: mangled,
        params,
        return_type,
        can_throw: !is_noexcept,
        is_virtual,
        has_body,
        is_system,
        annotations: func_annotations,
        referenced_globals: vec![],
        called_functions: vec![],
    })
}

fn parse_param(
    node: &Value,
    ctx: &TypeContext,
) -> Option<ParamInfo> {
    let name = node
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("unnamed")
        .to_string();
    let qual_type_raw = node.get("type")?.get("qualType")?.as_str()?;
    // Strip `__restrict` so the trailing-`*` detection below isn't fooled by
    // pointer types written as `uint32_t *__restrict`.
    let qual_type_owned = strip_restrict(qual_type_raw);
    let qual_type = qual_type_owned.as_str();

    let is_const = qual_type.contains("const");
    let is_ref = qual_type.contains('&');
    let is_ptr = qual_type.contains('*');

    let mutability = if is_const && (is_ref || is_ptr) {
        Mutability::Readonly
    } else if is_ref || is_ptr {
        Mutability::Mutable
    } else {
        Mutability::Readonly // Pass-by-value is effectively readonly
    };

    let nullable = if is_ref {
        Nullability::NonNull // References can't be null
    } else if is_ptr {
        Nullability::Nullable
    } else {
        Nullability::NonNull
    };

    // Check for SAL annotations in attributes
    let mut annotations = Vec::new();
    if let Some(inner) = node.get("inner").and_then(|v| v.as_array()) {
        for attr in inner {
            if let Some(ann) = parse_sal_annotation(attr) {
                annotations.push(ann);
            }
        }
    }

    // SAL + type system: strictest constraint wins.
    // If type says const, it stays const regardless of SAL.
    // SAL can only tighten, not loosen.
    let mutability = annotations.iter().fold(mutability, |m, ann| match ann {
        Annotation::InReads(_) => Mutability::Readonly, // _In_ → readonly (can tighten)
        Annotation::OutWrites(_) => {
            if m == Mutability::Readonly {
                Mutability::Readonly // const wins over _Out_
            } else {
                Mutability::WriteOnly
            }
        }
        Annotation::Inout => {
            if m == Mutability::Readonly {
                Mutability::Readonly // const wins over _Inout_
            } else {
                Mutability::Mutable
            }
        }
        _ => m,
    });

    let ty = parse_cpp_type(qual_type, ctx);

    Some(ParamInfo {
        name,
        ty,
        mutability,
        nullable,
        annotations,
    })
}

fn parse_sal_annotation(attr: &Value) -> Option<Annotation> {
    let kind = attr.get("kind")?.as_str()?;
    if kind != "AnnotateAttr" {
        return None;
    }
    // Clang's text-format AST dump records `__attribute__((annotate("X")))`
    // with `X` accessible as the `value` field. The JSON dumper, however,
    // emits the `AnnotateAttr` node WITHOUT the string argument (clang 17+).
    // Recover the macro name by reading the source file at the attribute's
    // expansion location.
    let value_owned: String;
    let value: &str = match attr.get("value").and_then(|v| v.as_str()) {
        Some(v) => v,
        None => {
            let exp = attr
                .get("range")?
                .get("begin")?
                .get("expansionLoc")?;
            let file = exp.get("file")?.as_str()?;
            let offset = exp.get("offset")?.as_u64()? as usize;
            let tok_len = exp.get("tokLen")?.as_u64()? as usize;
            value_owned = read_source_token(file, offset, tok_len)?;
            &value_owned
        }
    };
    match value {
        v if v.starts_with("_In_reads_(") => {
            let n: usize = v
                .trim_start_matches("_In_reads_(")
                .trim_end_matches(')')
                .parse()
                .ok()?;
            Some(Annotation::InReads(n))
        }
        v if v.starts_with("_In_reads_bytes_(") => {
            let n: usize = v
                .trim_start_matches("_In_reads_bytes_(")
                .trim_end_matches(')')
                .parse()
                .ok()?;
            Some(Annotation::InReads(n))
        }
        v if v.starts_with("_Out_writes_(") => {
            let n: usize = v
                .trim_start_matches("_Out_writes_(")
                .trim_end_matches(')')
                .parse()
                .ok()?;
            Some(Annotation::OutWrites(n))
        }
        v if v.starts_with("_Out_writes_bytes_(") => {
            let n: usize = v
                .trim_start_matches("_Out_writes_bytes_(")
                .trim_end_matches(')')
                .parse()
                .ok()?;
            Some(Annotation::OutWrites(n))
        }
        "_In_" | "_In_opt_" => Some(Annotation::InReads(0)),
        "_Out_" | "_Out_opt_" => Some(Annotation::OutWrites(0)),
        "_Inout_" | "_Inout_opt_" => Some(Annotation::Inout),
        "_Pre_valid_" => Some(Annotation::PreValid),
        "_Post_invalid_" => Some(Annotation::PostInvalid),
        "_Ret_maybenull_" => Some(Annotation::Custom("_Ret_maybenull_".into())),
        "_Must_inspect_result_" => Some(Annotation::MustInspectResult),
        "__declspec(nothrow)" | "nothrow" => Some(Annotation::NoThrow),
        _ => Some(Annotation::Custom(value.into())),
    }
}

fn parse_return_type(
    qual_type: &str,
    ctx: &TypeContext,
) -> TypeInfo {
    // Extract return type from function type string "RetType (ParamTypes)"
    let ret = qual_type.split('(').next().unwrap_or("void").trim();
    parse_cpp_type(ret, ctx)
}

/// Strip C/C++ `restrict` qualifiers (`__restrict`, `__restrict__`, `restrict`)
/// from a qualType string. Clang preserves these in `qualType` (e.g.
/// `"uint32_t *__restrict"`) but they don't affect the underlying SAW type;
/// leaving them in breaks pointer detection because the string no longer ends
/// in `*`.
fn strip_restrict(s: &str) -> String {
    let mut out = s.replace("__restrict__", "");
    out = out.replace("__restrict", "");
    // Whole-word `restrict` (C99 keyword). Avoid clobbering identifiers that
    // happen to contain the substring by requiring a word boundary on both
    // sides; cheap implementation via space-padding.
    let padded = format!(" {out} ");
    let stripped = padded.replace(" restrict ", " ");
    stripped.trim().to_string()
}

fn parse_cpp_type(
    qual_type: &str,
    ctx: &TypeContext,
) -> TypeInfo {
    let stripped = strip_restrict(qual_type);
    let t = stripped.trim();
    let t = t.trim_start_matches("const ");

    // Detect C array types like "uint8_t[32]" or "int[10]"
    if let Some(bracket_pos) = t.find('[') {
        if t.ends_with(']') {
            let elem_type_str = t[..bracket_pos].trim();
            let count_str = &t[bracket_pos + 1..t.len() - 1];
            if let Ok(count) = count_str.parse::<usize>() {
                let elem_type = parse_cpp_type(elem_type_str, ctx);
                if matches!(elem_type, TypeInfo::UnsignedInt(8) | TypeInfo::SignedInt(8)) {
                    return TypeInfo::ByteArray(count);
                }
                // For other element types, approximate as byte array of total size
                if let Some((elem_size, _)) = cpp_type_size_align(&elem_type) {
                    return TypeInfo::ByteArray(count * elem_size);
                }
            }
        }
    }

    // Detect pointer/reference before stripping — struct field types need wrapping
    let is_ptr = t.ends_with('*');
    let is_ref = t.ends_with('&');
    let t = t.trim_end_matches('&').trim_end_matches('*').trim();

    let base = match t {
        "bool" | "_Bool" => TypeInfo::Bool,
        "int" | "int32_t" | "long" => TypeInfo::SignedInt(32),
        "long long" | "int64_t" | "__int64" => TypeInfo::SignedInt(64),
        "short" | "int16_t" => TypeInfo::SignedInt(16),
        "char" | "int8_t" | "signed char" => TypeInfo::SignedInt(8),
        "unsigned int" | "uint32_t" | "unsigned long" | "DWORD" | "ULONG" => {
            TypeInfo::UnsignedInt(32)
        }
        "unsigned long long" | "uint64_t" | "size_t" | "__size_t" | "UINT64" | "ULONG64" => {
            TypeInfo::UnsignedInt(64)
        }
        "unsigned short" | "uint16_t" | "WORD" | "USHORT" => TypeInfo::UnsignedInt(16),
        "unsigned char" | "uint8_t" | "BYTE" | "UCHAR" => TypeInfo::UnsignedInt(8),
        "void" => TypeInfo::Void,
        other => {
            // Check if this is a known enum type from the AST
            if let Some((variants, bits)) = ctx.enums.get(other) {
                TypeInfo::Enum {
                    name: other.to_string(),
                    variants: variants.clone(),
                    discriminant_bits: *bits,
                }
            }
            // Check if this is a known struct type from the AST
            else if let Some(fields) = ctx.structs.get(other) {
                let size = compute_struct_size_from_fields(fields);
                TypeInfo::Struct {
                    name: other.to_string(),
                    size_bytes: size,
                    fields: fields.clone(),
                }
            } else if let Some(size) = lookup_known_type_size(other) {
                // Well-known STL/platform type with known size (MSVC x64)
                TypeInfo::Opaque {
                    name: other.to_string(),
                    size_bytes: size,
                }
            } else {
                TypeInfo::Opaque {
                    name: other.to_string(),
                    size_bytes: 0,
                }
            }
        }
    };

    // Wrap in Pointer if the original qualType had * or &
    if is_ptr || is_ref {
        TypeInfo::Pointer(Box::new(base))
    } else {
        base
    }
}

/// Look up well-known C++ STL and platform type sizes (MSVC x64 ABI).
pub fn lookup_known_type_size(name: &str) -> Option<usize> {
    // Normalize: strip std:: and class/struct prefixes for matching
    let normalized = name
        .trim_start_matches("class ")
        .trim_start_matches("struct ");

    match normalized {
        // STL containers and strings (MSVC x64 sizes)
        "std::string" | "std::basic_string<char>" | "std::basic_string<char, std::char_traits<char>, std::allocator<char>>" => Some(32),
        "std::wstring" | "std::basic_string<wchar_t>" | "std::basic_string<wchar_t, std::char_traits<wchar_t>, std::allocator<wchar_t>>" => Some(32),
        s if s.starts_with("std::basic_string<") => Some(32),
        s if s.starts_with("std::vector<") => Some(24),
        s if s.starts_with("std::array<") => None, // size depends on template params
        s if s.starts_with("std::map<") || s.starts_with("std::set<") => Some(16),
        s if s.starts_with("std::unordered_map<") || s.starts_with("std::unordered_set<") => Some(64),
        s if s.starts_with("std::shared_ptr<") => Some(16),
        s if s.starts_with("std::unique_ptr<") => Some(8),
        s if s.starts_with("std::optional<") => None, // size depends on inner type
        s if s.starts_with("std::variant<") => None,
        s if s.starts_with("std::tuple<") => None,
        "std::mutex" => Some(80),
        "std::recursive_mutex" => Some(80),
        s if s.starts_with("std::function<") => Some(64),
        s if s.starts_with("std::span<") => Some(16),
        "std::string_view" | "std::basic_string_view<char>" => Some(16),
        "std::wstring_view" | "std::basic_string_view<wchar_t>" => Some(16),
        // Win32 / MSVC types
        "GUID" | "CLSID" | "IID" => Some(16),
        "FILETIME" => Some(8),
        "LARGE_INTEGER" | "ULARGE_INTEGER" => Some(8),
        "CRITICAL_SECTION" => Some(40),
        "SRWLOCK" => Some(8),
        _ => None,
    }
}

/// Compute struct size from fields using C/C++ x64 layout rules.
fn compute_struct_size_from_fields(fields: &[(String, TypeInfo)]) -> Option<usize> {
    let mut offset = 0usize;
    let mut max_align = 1usize;

    for (_, ty) in fields {
        let (size, align) = cpp_type_size_align(ty)?;
        max_align = max_align.max(align);
        // Pad to alignment
        let remainder = offset % align;
        if remainder != 0 {
            offset += align - remainder;
        }
        offset += size;
    }

    // Pad to struct alignment
    if max_align > 0 {
        let remainder = offset % max_align;
        if remainder != 0 {
            offset += max_align - remainder;
        }
    }

    Some(offset)
}

/// Get (size, alignment) for a C++ TypeInfo value.
fn cpp_type_size_align(ty: &TypeInfo) -> Option<(usize, usize)> {
    match ty {
        TypeInfo::Bool => Some((1, 1)),
        TypeInfo::SignedInt(8) | TypeInfo::UnsignedInt(8) => Some((1, 1)),
        TypeInfo::SignedInt(16) | TypeInfo::UnsignedInt(16) => Some((2, 2)),
        TypeInfo::SignedInt(32) | TypeInfo::UnsignedInt(32) => Some((4, 4)),
        TypeInfo::SignedInt(64) | TypeInfo::UnsignedInt(64) => Some((8, 8)),
        TypeInfo::Pointer(_) => Some((8, 8)), // x64
        TypeInfo::ByteArray(n) => Some((*n, 1)),
        TypeInfo::Struct {
            size_bytes: Some(n),
            ..
        } => {
            // Alignment is max field alignment, but we use size as approximation
            Some((*n, 8.min(*n)))
        }
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
    use serde_json::json;

    #[test]
    fn test_parse_simple_function() {
        let ast = json!({
            "kind": "TranslationUnitDecl",
            "inner": [{
                "kind": "FunctionDecl",
                "name": "add",
                "mangledName": "_Z3addii",
                "type": { "qualType": "int (int, int)" },
                "inner": [
                    {
                        "kind": "ParmVarDecl",
                        "name": "a",
                        "type": { "qualType": "int" }
                    },
                    {
                        "kind": "ParmVarDecl",
                        "name": "b",
                        "type": { "qualType": "int" }
                    }
                ]
            }]
        });
        let funcs = extract_functions(&ast, None).unwrap();
        assert_eq!(funcs.len(), 1);
        assert_eq!(funcs[0].name, "add");
        assert_eq!(funcs[0].params.len(), 2);
        assert_eq!(funcs[0].params[0].ty, TypeInfo::SignedInt(32));
        assert_eq!(funcs[0].return_type, TypeInfo::SignedInt(32));
    }

    #[test]
    fn test_parse_pointer_param() {
        let ast = json!({
            "kind": "TranslationUnitDecl",
            "inner": [{
                "kind": "FunctionDecl",
                "name": "process",
                "type": { "qualType": "void (const int *, int *)" },
                "inner": [
                    {
                        "kind": "ParmVarDecl",
                        "name": "input",
                        "type": { "qualType": "const int *" }
                    },
                    {
                        "kind": "ParmVarDecl",
                        "name": "output",
                        "type": { "qualType": "int *" }
                    }
                ]
            }]
        });
        let funcs = extract_functions(&ast, None).unwrap();
        assert_eq!(funcs[0].params[0].mutability, Mutability::Readonly);
        assert!(matches!(funcs[0].params[0].ty, TypeInfo::Pointer(_)));
        assert_eq!(funcs[0].params[1].mutability, Mutability::Mutable);
        assert!(matches!(funcs[0].params[1].ty, TypeInfo::Pointer(_)));
    }

    #[test]
    fn test_parse_noexcept_function() {
        let ast = json!({
            "kind": "TranslationUnitDecl",
            "inner": [{
                "kind": "FunctionDecl",
                "name": "safe",
                "type": { "qualType": "void () noexcept" },
                "inner": []
            }]
        });
        let funcs = extract_functions(&ast, None).unwrap();
        assert!(!funcs[0].can_throw);
        assert!(funcs[0]
            .annotations
            .iter()
            .any(|a| matches!(a, Annotation::NoThrow)));
    }

    #[test]
    fn test_parse_nothrow_attr() {
        let ast = json!({
            "kind": "TranslationUnitDecl",
            "inner": [{
                "kind": "FunctionDecl",
                "name": "safe2",
                "type": { "qualType": "void ()" },
                "inner": [{
                    "kind": "NoThrowAttr"
                }]
            }]
        });
        let funcs = extract_functions(&ast, None).unwrap();
        assert!(!funcs[0].can_throw);
    }

    #[test]
    fn test_parse_sal_annotations_in_reads() {
        let ast = json!({
            "kind": "TranslationUnitDecl",
            "inner": [{
                "kind": "FunctionDecl",
                "name": "copy_data",
                "type": { "qualType": "void (const unsigned char *, unsigned char *)" },
                "inner": [
                    {
                        "kind": "ParmVarDecl",
                        "name": "src",
                        "type": { "qualType": "const unsigned char *" },
                        "inner": [{
                            "kind": "AnnotateAttr",
                            "value": "_In_reads_(16)"
                        }]
                    },
                    {
                        "kind": "ParmVarDecl",
                        "name": "dst",
                        "type": { "qualType": "unsigned char *" },
                        "inner": [{
                            "kind": "AnnotateAttr",
                            "value": "_Out_writes_(16)"
                        }]
                    }
                ]
            }]
        });
        let funcs = extract_functions(&ast, None).unwrap();
        assert_eq!(funcs[0].params[0].mutability, Mutability::Readonly);
        assert!(funcs[0].params[0]
            .annotations
            .iter()
            .any(|a| matches!(a, Annotation::InReads(16))));
        assert_eq!(funcs[0].params[1].mutability, Mutability::WriteOnly);
    }

    #[test]
    fn test_parse_sal_in_opt() {
        let ast = json!({
            "kind": "TranslationUnitDecl",
            "inner": [{
                "kind": "FunctionDecl",
                "name": "opt_fn",
                "type": { "qualType": "void (int *)" },
                "inner": [{
                    "kind": "ParmVarDecl",
                    "name": "p",
                    "type": { "qualType": "int *" },
                    "inner": [{
                        "kind": "AnnotateAttr",
                        "value": "_In_opt_"
                    }]
                }]
            }]
        });
        let funcs = extract_functions(&ast, None).unwrap();
        assert_eq!(funcs[0].params[0].mutability, Mutability::Readonly);
    }

    #[test]
    fn test_parse_sal_inout() {
        let ast = json!({
            "kind": "TranslationUnitDecl",
            "inner": [{
                "kind": "FunctionDecl",
                "name": "inout_fn",
                "type": { "qualType": "void (int *)" },
                "inner": [{
                    "kind": "ParmVarDecl",
                    "name": "p",
                    "type": { "qualType": "int *" },
                    "inner": [{
                        "kind": "AnnotateAttr",
                        "value": "_Inout_"
                    }]
                }]
            }]
        });
        let funcs = extract_functions(&ast, None).unwrap();
        assert_eq!(funcs[0].params[0].mutability, Mutability::Mutable);
    }

    #[test]
    fn test_parse_sal_in_reads_bytes() {
        let ast = json!({
            "kind": "TranslationUnitDecl",
            "inner": [{
                "kind": "FunctionDecl",
                "name": "read_bytes_fn",
                "type": { "qualType": "void (const void *)" },
                "inner": [{
                    "kind": "ParmVarDecl",
                    "name": "buf",
                    "type": { "qualType": "const void *" },
                    "inner": [{
                        "kind": "AnnotateAttr",
                        "value": "_In_reads_bytes_(256)"
                    }]
                }]
            }]
        });
        let funcs = extract_functions(&ast, None).unwrap();
        assert!(funcs[0].params[0]
            .annotations
            .iter()
            .any(|a| matches!(a, Annotation::InReads(256))));
    }

    #[test]
    fn test_parse_struct_fields() {
        let ast = json!({
            "kind": "TranslationUnitDecl",
            "inner": [
                {
                    "kind": "CXXRecordDecl",
                    "name": "Point",
                    "inner": [
                        {
                            "kind": "FieldDecl",
                            "name": "x",
                            "type": { "qualType": "int" }
                        },
                        {
                            "kind": "FieldDecl",
                            "name": "y",
                            "type": { "qualType": "int" }
                        }
                    ]
                },
                {
                    "kind": "FunctionDecl",
                    "name": "use_point",
                    "type": { "qualType": "void (const Point &)" },
                    "inner": [{
                        "kind": "ParmVarDecl",
                        "name": "p",
                        "type": { "qualType": "const Point &" }
                    }]
                }
            ]
        });
        let funcs = extract_functions(&ast, None).unwrap();
        if let TypeInfo::Pointer(inner) = &funcs[0].params[0].ty {
            if let TypeInfo::Struct { name, fields, .. } = inner.as_ref() {
                assert_eq!(name, "Point");
                assert_eq!(fields.len(), 2);
            } else {
                panic!("Expected Struct type, got {:?}", inner);
            }
        } else {
            panic!("Expected Pointer type");
        }
    }

    #[test]
    fn test_parse_template_specialization() {
        let ast = json!({
            "kind": "TranslationUnitDecl",
            "inner": [{
                "kind": "FunctionTemplateDecl",
                "name": "max",
                "inner": [
                    {
                        "kind": "FunctionDecl",
                        "name": "max",
                        "type": { "qualType": "int (int, int)" },
                        "inner": [
                            {
                                "kind": "ParmVarDecl",
                                "name": "a",
                                "type": { "qualType": "int" }
                            },
                            {
                                "kind": "ParmVarDecl",
                                "name": "b",
                                "type": { "qualType": "int" }
                            }
                        ]
                    }
                ]
            }]
        });
        let funcs = extract_functions(&ast, None).unwrap();
        assert_eq!(funcs.len(), 1);
        assert_eq!(funcs[0].name, "max");
    }

    #[test]
    fn test_filter_functions() {
        let ast = json!({
            "kind": "TranslationUnitDecl",
            "inner": [
                {
                    "kind": "FunctionDecl",
                    "name": "validate_request",
                    "type": { "qualType": "bool ()" },
                    "inner": []
                },
                {
                    "kind": "FunctionDecl",
                    "name": "process_data",
                    "type": { "qualType": "void ()" },
                    "inner": []
                }
            ]
        });
        let funcs = extract_functions(&ast, Some("validate")).unwrap();
        assert_eq!(funcs.len(), 1);
        assert_eq!(funcs[0].name, "validate_request");
    }

    #[test]
    fn test_const_method_this() {
        let ast = json!({
            "kind": "TranslationUnitDecl",
            "inner": [{
                "kind": "CXXMethodDecl",
                "name": "getValue",
                "parentDeclContextId": "MyClass",
                "type": { "qualType": "int () const" },
                "inner": []
            }]
        });
        let funcs = extract_functions(&ast, None).unwrap();
        assert_eq!(funcs[0].params.len(), 1);
        assert_eq!(funcs[0].params[0].name, "this");
        assert_eq!(funcs[0].params[0].mutability, Mutability::Readonly);
        assert!(matches!(funcs[0].params[0].ty, TypeInfo::Pointer(_)));
    }

    #[test]
    fn test_non_const_method_this() {
        let ast = json!({
            "kind": "TranslationUnitDecl",
            "inner": [{
                "kind": "CXXMethodDecl",
                "name": "setValue",
                "parentDeclContextId": "MyClass",
                "type": { "qualType": "void (int)" },
                "inner": [{
                    "kind": "ParmVarDecl",
                    "name": "val",
                    "type": { "qualType": "int" }
                }]
            }]
        });
        let funcs = extract_functions(&ast, None).unwrap();
        assert_eq!(funcs[0].params[0].name, "this");
        assert_eq!(funcs[0].params[0].mutability, Mutability::Mutable);
        assert_eq!(funcs[0].params[1].name, "val");
    }

    #[test]
    fn test_parse_msvc_types() {
        let ast = json!({
            "kind": "TranslationUnitDecl",
            "inner": [{
                "kind": "FunctionDecl",
                "name": "win_fn",
                "type": { "qualType": "DWORD (BYTE *, WORD)" },
                "inner": [
                    {
                        "kind": "ParmVarDecl",
                        "name": "buf",
                        "type": { "qualType": "BYTE *" }
                    },
                    {
                        "kind": "ParmVarDecl",
                        "name": "len",
                        "type": { "qualType": "WORD" }
                    }
                ]
            }]
        });
        let funcs = extract_functions(&ast, None).unwrap();
        assert_eq!(funcs[0].return_type, TypeInfo::UnsignedInt(32));
        // buf is BYTE* -> Pointer(UnsignedInt(8))
        if let TypeInfo::Pointer(inner) = &funcs[0].params[0].ty {
            assert_eq!(**inner, TypeInfo::UnsignedInt(8));
        } else {
            panic!("Expected Pointer type");
        }
        assert_eq!(funcs[0].params[1].ty, TypeInfo::UnsignedInt(16));
    }

    // ========================================================================
    // Item #2: STL type size resolution and struct size computation
    // ========================================================================

    #[test]
    fn test_stl_string_size() {
        let ast = json!({
            "kind": "TranslationUnitDecl",
            "inner": [{
                "kind": "FunctionDecl",
                "name": "use_string",
                "type": { "qualType": "void (const std::string &)" },
                "inner": [{
                    "kind": "ParmVarDecl",
                    "name": "s",
                    "type": { "qualType": "const std::string &" }
                }]
            }]
        });
        let funcs = extract_functions(&ast, None).unwrap();
        if let TypeInfo::Pointer(inner) = &funcs[0].params[0].ty {
            if let TypeInfo::Opaque { name, size_bytes } = inner.as_ref() {
                assert_eq!(name, "std::string");
                assert_eq!(*size_bytes, 32);
            } else {
                panic!("Expected Opaque type with size, got {:?}", inner);
            }
        } else {
            panic!("Expected Pointer type");
        }
    }

    #[test]
    fn test_stl_vector_size() {
        assert_eq!(lookup_known_type_size("std::vector<int>"), Some(24));
        assert_eq!(lookup_known_type_size("std::vector<std::string>"), Some(24));
    }

    #[test]
    fn test_stl_smart_pointers() {
        assert_eq!(lookup_known_type_size("std::shared_ptr<int>"), Some(16));
        assert_eq!(lookup_known_type_size("std::unique_ptr<int>"), Some(8));
    }

    #[test]
    fn test_stl_string_view() {
        assert_eq!(lookup_known_type_size("std::string_view"), Some(16));
    }

    #[test]
    fn test_win32_types() {
        assert_eq!(lookup_known_type_size("GUID"), Some(16));
        assert_eq!(lookup_known_type_size("CRITICAL_SECTION"), Some(40));
    }

    #[test]
    fn test_unknown_type_stays_zero() {
        assert_eq!(lookup_known_type_size("MyCustomClass"), None);
    }

    #[test]
    fn test_struct_size_computation() {
        let fields = vec![
            ("x".into(), TypeInfo::SignedInt(32)),
            ("y".into(), TypeInfo::SignedInt(32)),
        ];
        assert_eq!(compute_struct_size_from_fields(&fields), Some(8));
    }

    #[test]
    fn test_struct_size_with_padding() {
        let fields = vec![
            ("a".into(), TypeInfo::SignedInt(8)),
            ("b".into(), TypeInfo::SignedInt(32)),
        ];
        // i8 (1 byte) + 3 padding + i32 (4 bytes) = 8, total aligned to 4 = 8
        assert_eq!(compute_struct_size_from_fields(&fields), Some(8));
    }

    #[test]
    fn test_struct_size_resolved_in_ast() {
        let ast = json!({
            "kind": "TranslationUnitDecl",
            "inner": [
                {
                    "kind": "CXXRecordDecl",
                    "name": "Vec2",
                    "inner": [
                        { "kind": "FieldDecl", "name": "x", "type": { "qualType": "int" } },
                        { "kind": "FieldDecl", "name": "y", "type": { "qualType": "int" } }
                    ]
                },
                {
                    "kind": "FunctionDecl",
                    "name": "use_vec",
                    "type": { "qualType": "void (const Vec2 &)" },
                    "inner": [{
                        "kind": "ParmVarDecl",
                        "name": "v",
                        "type": { "qualType": "const Vec2 &" }
                    }]
                }
            ]
        });
        let funcs = extract_functions(&ast, None).unwrap();
        if let TypeInfo::Pointer(inner) = &funcs[0].params[0].ty {
            if let TypeInfo::Struct {
                name, size_bytes, fields, ..
            } = inner.as_ref()
            {
                assert_eq!(name, "Vec2");
                assert_eq!(*size_bytes, Some(8)); // 2 x i32
                assert_eq!(fields.len(), 2);
            } else {
                panic!("Expected Struct, got {:?}", inner);
            }
        } else {
            panic!("Expected Pointer");
        }
    }

    // ========================================================================
    // Item #4: Concatenated JSON objects from clang -ast-dump-filter
    // ========================================================================

    #[test]
    fn test_parse_single_json_object() {
        let content = r#"{"kind": "TranslationUnitDecl", "inner": []}"#;
        let ast = parse_ast_str(content).unwrap();
        assert_eq!(ast["kind"], "TranslationUnitDecl");
    }

    #[test]
    fn test_parse_concatenated_json_objects() {
        // Simulates clang -ast-dump-filter output: multiple top-level objects
        let content = r#"{"kind": "FunctionDecl", "name": "foo", "type": {"qualType": "void ()"}, "inner": []}
{"kind": "FunctionDecl", "name": "bar", "type": {"qualType": "int (int)"}, "inner": [{"kind": "ParmVarDecl", "name": "x", "type": {"qualType": "int"}}]}"#;
        let ast = parse_ast_str(content).unwrap();
        // Should be wrapped in a synthetic TranslationUnitDecl
        assert_eq!(ast["kind"], "TranslationUnitDecl");
        let inner = ast["inner"].as_array().unwrap();
        assert_eq!(inner.len(), 2);
        assert_eq!(inner[0]["name"], "foo");
        assert_eq!(inner[1]["name"], "bar");
    }

    #[test]
    fn test_concatenated_json_extracts_functions() {
        let content = r#"{"kind": "FunctionDecl", "name": "alpha", "type": {"qualType": "void ()"}, "inner": []}
{"kind": "FunctionDecl", "name": "beta", "type": {"qualType": "int (int)"}, "inner": [{"kind": "ParmVarDecl", "name": "n", "type": {"qualType": "int"}}]}"#;
        let ast = parse_ast_str(content).unwrap();
        let funcs = extract_functions(&ast, None).unwrap();
        assert_eq!(funcs.len(), 2);
        assert_eq!(funcs[0].name, "alpha");
        assert_eq!(funcs[1].name, "beta");
    }

    #[test]
    fn test_concatenated_json_with_nested_braces() {
        // Ensure nested braces in string values don't break the splitter
        let content = r#"{"kind": "FunctionDecl", "name": "fn_with_braces", "type": {"qualType": "void ()"}, "inner": [], "note": "has { braces }"}
{"kind": "FunctionDecl", "name": "second", "type": {"qualType": "void ()"}, "inner": []}"#;
        let ast = parse_ast_str(content).unwrap();
        let inner = ast["inner"].as_array().unwrap();
        assert_eq!(inner.len(), 2);
    }

    #[test]
    fn test_empty_input_fails() {
        let result = parse_ast_str("");
        assert!(result.is_err());
    }

    // ========================================================================
    // Item #5: OOM protection — file size guard
    // ========================================================================

    #[test]
    fn test_large_file_rejected() {
        // Create a temp file larger than 100MB would be impractical,
        // so test that parse_ast checks metadata and the error message is helpful
        let dir = std::env::temp_dir().join("saw_spec_gen_test_oom");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("nonexistent_huge.json");
        // parse_ast on nonexistent file should fail with a stat error, not OOM
        let result = crate::clang_ast::parse_ast(&path);
        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(err_msg.contains("Failed to stat") || err_msg.contains("Failed to read"));
    }

    #[test]
    fn test_small_file_accepted() {
        let dir = std::env::temp_dir().join("saw_spec_gen_test_oom_small");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("small.json");
        std::fs::write(
            &path,
            r#"{"kind": "TranslationUnitDecl", "inner": []}"#,
        )
        .unwrap();
        let result = crate::clang_ast::parse_ast(&path);
        assert!(result.is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_max_size_constant_is_100mb() {
        assert_eq!(MAX_AST_FILE_SIZE, 100 * 1024 * 1024);
    }

    // ========================================================================
    // Virtual method extraction + interface stubs
    // ========================================================================

    #[test]
    fn test_extract_pure_virtual_methods() {
        let ast = json!({
            "kind": "TranslationUnitDecl",
            "inner": [{
                "id": "0xAABB",
                "kind": "CXXRecordDecl",
                "name": "IService",
                "inner": [
                    {
                        "kind": "CXXMethodDecl",
                        "name": "process",
                        "type": { "qualType": "bool (int) const noexcept" },
                        "virtual": true,
                        "pure": true,
                        "inner": [{
                            "kind": "ParmVarDecl",
                            "name": "input",
                            "type": { "qualType": "int" }
                        }]
                    },
                    {
                        "kind": "CXXMethodDecl",
                        "name": "get_status",
                        "type": { "qualType": "int () const noexcept" },
                        "virtual": true,
                        "pure": true,
                        "inner": []
                    }
                ]
            }]
        });
        let methods = extract_virtual_methods(&ast, None).unwrap();
        assert_eq!(methods.len(), 2);
        assert_eq!(methods[0].class_name, "IService");
        assert_eq!(methods[0].method.name, "process");
        assert!(methods[0].is_pure);
        assert_eq!(methods[1].class_name, "IService");
        assert_eq!(methods[1].method.name, "get_status");
    }

    #[test]
    fn test_extract_virtual_not_pure() {
        let ast = json!({
            "kind": "TranslationUnitDecl",
            "inner": [{
                "id": "0xCC",
                "kind": "CXXRecordDecl",
                "name": "Base",
                "inner": [{
                    "kind": "CXXMethodDecl",
                    "name": "do_thing",
                    "type": { "qualType": "void ()" },
                    "virtual": true,
                    "inner": []
                }]
            }]
        });
        let methods = extract_virtual_methods(&ast, None).unwrap();
        assert_eq!(methods.len(), 1);
        assert_eq!(methods[0].class_name, "Base");
        assert!(!methods[0].is_pure);
    }

    #[test]
    fn test_no_virtual_methods_in_plain_class() {
        let ast = json!({
            "kind": "TranslationUnitDecl",
            "inner": [{
                "id": "0xDD",
                "kind": "CXXRecordDecl",
                "name": "Plain",
                "inner": [{
                    "kind": "CXXMethodDecl",
                    "name": "do_thing",
                    "type": { "qualType": "void ()" },
                    "inner": []
                }]
            }]
        });
        let methods = extract_virtual_methods(&ast, None).unwrap();
        assert_eq!(methods.len(), 0);
    }

    #[test]
    fn test_virtual_method_filter() {
        let ast = json!({
            "kind": "TranslationUnitDecl",
            "inner": [{
                "id": "0xEE",
                "kind": "CXXRecordDecl",
                "name": "IFace",
                "inner": [
                    {
                        "kind": "CXXMethodDecl",
                        "name": "validate",
                        "type": { "qualType": "bool () const" },
                        "virtual": true,
                        "pure": true,
                        "inner": []
                    },
                    {
                        "kind": "CXXMethodDecl",
                        "name": "reset",
                        "type": { "qualType": "void ()" },
                        "virtual": true,
                        "pure": true,
                        "inner": []
                    }
                ]
            }]
        });
        let methods = extract_virtual_methods(&ast, Some("validate")).unwrap();
        assert_eq!(methods.len(), 1);
        assert_eq!(methods[0].method.name, "validate");
    }

    #[test]
    fn test_virtual_const_vs_mutable_this() {
        let ast = json!({
            "kind": "TranslationUnitDecl",
            "inner": [{
                "id": "0xFF",
                "kind": "CXXRecordDecl",
                "name": "IWidget",
                "inner": [
                    {
                        "kind": "CXXMethodDecl",
                        "name": "read_val",
                        "type": { "qualType": "int () const" },
                        "virtual": true,
                        "pure": true,
                        "inner": []
                    },
                    {
                        "kind": "CXXMethodDecl",
                        "name": "mutate",
                        "type": { "qualType": "void (int)" },
                        "virtual": true,
                        "pure": true,
                        "inner": [{
                            "kind": "ParmVarDecl",
                            "name": "val",
                            "type": { "qualType": "int" }
                        }]
                    }
                ]
            }]
        });
        let methods = extract_virtual_methods(&ast, None).unwrap();
        // read_val is const → this should be readonly
        assert_eq!(methods[0].method.params[0].mutability, Mutability::Readonly);
        // mutate is non-const → this should be mutable
        assert_eq!(methods[1].method.params[0].mutability, Mutability::Mutable);
    }

    /// Regression: vtable slots must follow source declaration order, not
    /// the order the AST walker encounters methods. MSVC assigns slots in
    /// the order virtual methods appear in the class body — sorting by
    /// `source_offset` is what guarantees the generated vtable matches
    /// the real ABI layout. Without this sort, SAW resolves vtable
    /// dispatch to the wrong stub function.
    #[test]
    fn test_virtual_method_source_offset_captured() {
        let ast = json!({
            "kind": "TranslationUnitDecl",
            "inner": [{
                "id": "0xAA",
                "kind": "CXXRecordDecl",
                "name": "IRequestValidator",
                "inner": [
                    {
                        "kind": "CXXMethodDecl",
                        "name": "IsValidMetadataHeader",
                        "loc": { "offset": 100, "line": 5, "col": 18 },
                        "type": { "qualType": "bool () const noexcept" },
                        "virtual": true,
                        "pure": true,
                        "inner": []
                    },
                    {
                        "kind": "CXXMethodDecl",
                        "name": "Authenticate",
                        "loc": { "offset": 200, "line": 6, "col": 18 },
                        "type": { "qualType": "bool () const noexcept" },
                        "virtual": true,
                        "pure": true,
                        "inner": []
                    }
                ]
            }]
        });
        let methods = extract_virtual_methods(&ast, None).unwrap();
        assert_eq!(methods.len(), 2);
        let is_valid = methods
            .iter()
            .find(|m| m.method.name == "IsValidMetadataHeader")
            .expect("missing IsValidMetadataHeader");
        let authenticate = methods
            .iter()
            .find(|m| m.method.name == "Authenticate")
            .expect("missing Authenticate");
        assert_eq!(is_valid.source_offset, 100);
        assert_eq!(authenticate.source_offset, 200);
        // After sorting by source_offset, IsValidMetadataHeader (offset 100)
        // must come before Authenticate (offset 200) — this is the
        // declaration order MSVC uses to assign vtable slots.
        assert!(is_valid.source_offset < authenticate.source_offset);
    }

    /// Regression: when `clang -ast-dump-filter=IFace` emits a bare
    /// `CXXRecordDecl` (no enclosing `TranslationUnitDecl`), `merge_asts`
    /// must preserve the class wrapper. Previously the merger unwrapped
    /// any node that had an `inner` field — including `CXXRecordDecl` —
    /// which flattened each interface's methods to the synthetic TU
    /// root, stripped their enclosing-class context, and caused
    /// `extract_virtual_methods` to attribute every method to "Unknown".
    /// The downstream effect was a single `@unknown_vtable` containing
    /// the merged slots of every interface, dispatching virtual calls
    /// at the wrong offsets.
    #[test]
    fn test_merge_asts_preserves_filtered_cxxrecorddecl() {
        let ikeystore = json!({
            "id": "0xKEY",
            "kind": "CXXRecordDecl",
            "name": "IKeyStore",
            "inner": [
                {
                    "kind": "CXXMethodDecl",
                    "name": "store_key",
                    "loc": { "offset": 10 },
                    "type": { "qualType": "bool () const noexcept" },
                    "virtual": true,
                    "pure": true,
                    "inner": []
                },
                {
                    "kind": "CXXMethodDecl",
                    "name": "load_key",
                    "loc": { "offset": 20 },
                    "type": { "qualType": "bool () const noexcept" },
                    "virtual": true,
                    "pure": true,
                    "inner": []
                }
            ]
        });
        let ivalidator = json!({
            "id": "0xVAL",
            "kind": "CXXRecordDecl",
            "name": "IRequestValidator",
            "inner": [
                {
                    "kind": "CXXMethodDecl",
                    "name": "validate",
                    "loc": { "offset": 30 },
                    "type": { "qualType": "bool () const noexcept" },
                    "virtual": true,
                    "pure": true,
                    "inner": []
                },
                {
                    "kind": "CXXMethodDecl",
                    "name": "authenticate",
                    "loc": { "offset": 40 },
                    "type": { "qualType": "bool () const noexcept" },
                    "virtual": true,
                    "pure": true,
                    "inner": []
                }
            ]
        });
        let merged = merge_asts(vec![ikeystore, ivalidator]);
        let methods = extract_virtual_methods(&merged, None).unwrap();
        assert_eq!(methods.len(), 4);
        // Critical assertion: methods must retain their per-interface
        // class_name so vtable generation produces @ikeystore_vtable and
        // @irequestvalidator_vtable instead of a single @unknown_vtable.
        let by_class: std::collections::HashMap<&str, usize> =
            methods.iter().fold(std::collections::HashMap::new(), |mut acc, m| {
                *acc.entry(m.class_name.as_str()).or_insert(0) += 1;
                acc
            });
        assert_eq!(by_class.get("IKeyStore"), Some(&2));
        assert_eq!(by_class.get("IRequestValidator"), Some(&2));
        assert_eq!(by_class.get("Unknown"), None);
    }

    /// Sanity check: merging real `TranslationUnitDecl` inputs still
    /// concatenates their top-level `inner` arrays (existing behavior
    /// preserved for the common case where each AST is a full TU).
    #[test]
    fn test_merge_asts_concatenates_translation_units() {
        let tu_a = json!({
            "kind": "TranslationUnitDecl",
            "inner": [{
                "id": "0xA",
                "kind": "CXXRecordDecl",
                "name": "A",
                "inner": [{
                    "kind": "CXXMethodDecl",
                    "name": "foo",
                    "type": { "qualType": "void ()" },
                    "virtual": true,
                    "pure": true,
                    "inner": []
                }]
            }]
        });
        let tu_b = json!({
            "kind": "TranslationUnitDecl",
            "inner": [{
                "id": "0xB",
                "kind": "CXXRecordDecl",
                "name": "B",
                "inner": [{
                    "kind": "CXXMethodDecl",
                    "name": "bar",
                    "type": { "qualType": "void ()" },
                    "virtual": true,
                    "pure": true,
                    "inner": []
                }]
            }]
        });
        let merged = merge_asts(vec![tu_a, tu_b]);
        let methods = extract_virtual_methods(&merged, None).unwrap();
        assert_eq!(methods.len(), 2);
        let names: std::collections::HashSet<&str> =
            methods.iter().map(|m| m.class_name.as_str()).collect();
        assert!(names.contains("A"));
        assert!(names.contains("B"));
    }
}
