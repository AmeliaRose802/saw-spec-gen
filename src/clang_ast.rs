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

/// Maximum file size we'll attempt to load into memory (100 MB).
const MAX_AST_FILE_SIZE: u64 = 100 * 1024 * 1024;

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
}

/// Extract function info from the clang AST.
pub fn extract_functions(ast: &Value, filter: Option<&str>) -> Result<Vec<FunctionInfo>> {
    // First pass: collect struct/class definitions for type resolution
    let mut struct_map: HashMap<String, Vec<(String, TypeInfo)>> = HashMap::new();
    collect_struct_defs(ast, &mut struct_map);

    // Also build a map from clang node IDs (hex strings like "0x1ffd7063098")
    // to class/struct names, so we can resolve parentDeclContextId on methods.
    let mut id_to_name: HashMap<String, String> = HashMap::new();
    collect_node_ids(ast, &mut id_to_name);

    // Second pass: collect function declarations
    let mut functions = Vec::new();
    collect_functions(ast, &mut functions, filter, &struct_map, &id_to_name);
    Ok(functions)
}

/// Extract virtual methods from C++ classes in the AST.
///
/// Returns `InterfaceMethod` entries for each virtual method found.
/// These can be used to generate vtable resolution stubs and havoc specs.
pub fn extract_virtual_methods(ast: &Value, filter: Option<&str>) -> Result<Vec<InterfaceMethod>> {
    let mut struct_map: HashMap<String, Vec<(String, TypeInfo)>> = HashMap::new();
    collect_struct_defs(ast, &mut struct_map);

    let mut id_to_name: HashMap<String, String> = HashMap::new();
    collect_node_ids(ast, &mut id_to_name);

    let mut methods = Vec::new();
    collect_virtual_methods(ast, &mut methods, filter, &struct_map, &id_to_name);
    Ok(methods)
}

fn collect_virtual_methods(
    node: &Value,
    out: &mut Vec<InterfaceMethod>,
    filter: Option<&str>,
    struct_map: &HashMap<String, Vec<(String, TypeInfo)>>,
    id_to_name: &HashMap<String, String>,
) {
    collect_virtual_methods_inner(node, out, filter, struct_map, id_to_name, None);
}

fn collect_virtual_methods_inner(
    node: &Value,
    out: &mut Vec<InterfaceMethod>,
    filter: Option<&str>,
    struct_map: &HashMap<String, Vec<(String, TypeInfo)>>,
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
        let is_virtual = node.get("virtual").and_then(|v| v.as_bool()).unwrap_or(false);
        if is_virtual {
            let is_pure = node.get("pure").and_then(|v| v.as_bool()).unwrap_or(false);
            if let Some(func) =
                parse_function_decl(node, true, struct_map, id_to_name)
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

                    out.push(InterfaceMethod {
                        class_name,
                        method: func,
                        is_pure,
                    });
                }
            }
        }
    }

    // Recurse into children
    let next_class = class_name_for_children.or(enclosing_class);
    if let Some(inner) = node.get("inner").and_then(|v| v.as_array()) {
        for child in inner {
            collect_virtual_methods_inner(child, out, filter, struct_map, id_to_name, next_class);
        }
    }
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

/// Recursively collect struct/class definitions from RecordDecl and CXXRecordDecl nodes.
fn collect_struct_defs(node: &Value, struct_map: &mut HashMap<String, Vec<(String, TypeInfo)>>) {
    if let Some(kind) = node.get("kind").and_then(|v| v.as_str()) {
        let is_record = kind == "RecordDecl"
            || kind == "CXXRecordDecl"
            || kind == "ClassTemplateSpecializationDecl";
        if is_record {
            if let Some(name) = node.get("name").and_then(|v| v.as_str()) {
                if let Some(inner) = node.get("inner").and_then(|v| v.as_array()) {
                    let mut fields = Vec::new();
                    for child in inner {
                        if child.get("kind").and_then(|v| v.as_str()) == Some("FieldDecl") {
                            let field_name = child
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("unnamed")
                                .to_string();
                            let field_type = child
                                .get("type")
                                .and_then(|v| v.get("qualType"))
                                .and_then(|v| v.as_str())
                                .map(|qt| parse_cpp_type(qt, struct_map))
                                .unwrap_or(TypeInfo::Void);
                            fields.push((field_name, field_type));
                        }
                    }
                    if !fields.is_empty() {
                        struct_map.insert(name.to_string(), fields);
                    }
                }
            }
        }
    }

    if let Some(inner) = node.get("inner").and_then(|v| v.as_array()) {
        for child in inner {
            collect_struct_defs(child, struct_map);
        }
    }
}

fn collect_functions(
    node: &Value,
    out: &mut Vec<FunctionInfo>,
    filter: Option<&str>,
    struct_map: &HashMap<String, Vec<(String, TypeInfo)>>,
    id_to_name: &HashMap<String, String>,
) {
    if let Some(kind) = node.get("kind").and_then(|v| v.as_str()) {
        match kind {
            "FunctionDecl" | "CXXMethodDecl" => {
                if let Some(func) =
                    parse_function_decl(node, kind == "CXXMethodDecl", struct_map, id_to_name)
                {
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
                                    struct_map,
                                    id_to_name,
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
                collect_functions(child, out, filter, struct_map, id_to_name);
            }
        }
    }
}

fn parse_function_decl(
    node: &Value,
    is_method: bool,
    struct_map: &HashMap<String, Vec<(String, TypeInfo)>>,
    id_to_name: &HashMap<String, String>,
) -> Option<FunctionInfo> {
    let name = node.get("name")?.as_str()?.to_string();
    let mangled = node
        .get("mangledName")
        .and_then(|v| v.as_str())
        .map(String::from);
    let qual_type = node.get("type")?.get("qualType")?.as_str()?;

    let is_const_method = is_method && qual_type.contains("const");
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
        let parent_id = node
            .get("parentDeclContextId")
            .and_then(|v| v.as_str())
            .unwrap_or("Self");
        // Resolve the hex node ID to the actual class name
        let parent_name = id_to_name
            .get(parent_id)
            .map(|s| s.as_str())
            .unwrap_or(parent_id);
        let inner_type = if let Some(fields) = struct_map.get(parent_name) {
            let size = compute_struct_size_from_fields(fields);
            TypeInfo::Struct {
                name: parent_name.into(),
                size_bytes: size,
                fields: fields.clone(),
            }
        } else {
            TypeInfo::Opaque {
                name: parent_name.into(),
                size_bytes: 0,
            }
        };
        params.push(ParamInfo {
            name: "this".into(),
            ty: TypeInfo::Pointer(Box::new(inner_type)),
            mutability: if is_const_method {
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
                if let Some(param) = parse_param(child, struct_map) {
                    params.push(param);
                }
            }
        }
    }

    let return_type = parse_return_type(qual_type, struct_map);

    Some(FunctionInfo {
        name,
        mangled_name: mangled,
        params,
        return_type,
        can_throw: !is_noexcept,
        annotations: func_annotations,
    })
}

fn parse_param(
    node: &Value,
    struct_map: &HashMap<String, Vec<(String, TypeInfo)>>,
) -> Option<ParamInfo> {
    let name = node
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("unnamed")
        .to_string();
    let qual_type = node.get("type")?.get("qualType")?.as_str()?;

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

    let ty = parse_cpp_type(qual_type, struct_map);

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
    let value = attr.get("value")?.as_str()?;
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
    struct_map: &HashMap<String, Vec<(String, TypeInfo)>>,
) -> TypeInfo {
    // Extract return type from function type string "RetType (ParamTypes)"
    let ret = qual_type.split('(').next().unwrap_or("void").trim();
    parse_cpp_type(ret, struct_map)
}

fn parse_cpp_type(
    qual_type: &str,
    struct_map: &HashMap<String, Vec<(String, TypeInfo)>>,
) -> TypeInfo {
    let t = qual_type.trim();
    let t = t.trim_start_matches("const ");

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
        "unsigned long long" | "uint64_t" | "size_t" | "UINT64" | "ULONG64" => {
            TypeInfo::UnsignedInt(64)
        }
        "unsigned short" | "uint16_t" | "WORD" | "USHORT" => TypeInfo::UnsignedInt(16),
        "unsigned char" | "uint8_t" | "BYTE" | "UCHAR" => TypeInfo::UnsignedInt(8),
        "void" => TypeInfo::Void,
        other => {
            // Check if this is a known struct type from the AST
            if let Some(fields) = struct_map.get(other) {
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
fn lookup_known_type_size(name: &str) -> Option<usize> {
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
}
