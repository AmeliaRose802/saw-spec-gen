//! Parse LLVM IR text (.ll files) and extract function signatures.
//!
//! This provides a language-agnostic way to extract type information
//! from any language that compiles to LLVM IR. Works with output from
//! `llvm-dis` (converts .bc to .ll).

use crate::constraints::*;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::Path;

/// Maximum file size we'll attempt to load into memory (200 MB).
/// LLVM IR text is typically larger than JSON but compresses well, so higher limit.
const MAX_IR_FILE_SIZE: u64 = 200 * 1024 * 1024;

/// Parse LLVM IR text file and extract function declarations/definitions.
pub fn parse_llvm_ir(path: &Path) -> Result<String> {
    // Check file size before loading to avoid OOM
    let metadata = std::fs::metadata(path)
        .with_context(|| format!("Failed to stat {}", path.display()))?;
    let file_size = metadata.len();
    if file_size > MAX_IR_FILE_SIZE {
        let size_mb = file_size / (1024 * 1024);
        anyhow::bail!(
            "LLVM IR file {} is too large ({} MB, limit is {} MB). To reduce size, try:\n\
             \n  1. Use opt --passes=internalize,globaldce to strip unused functions\n\
             \n  2. Use llvm-extract to pull out specific functions\n\
             \n  3. Use --filter to process only matching functions after loading",
            path.display(),
            size_mb,
            MAX_IR_FILE_SIZE / (1024 * 1024),
        );
    }

    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    Ok(content)
}

/// Extract function info from LLVM IR text.
pub fn extract_functions(ir: &str, filter: Option<&str>) -> Result<Vec<FunctionInfo>> {
    // First pass: collect struct type definitions for size computation
    let struct_map = parse_struct_types(ir);

    let mut functions = Vec::new();

    for line in ir.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("declare ") || trimmed.starts_with("define ") {
            if let Some(func) = parse_ir_function(trimmed, &struct_map) {
                let matches = filter.map(|f| func.name.contains(f)).unwrap_or(true);
                if matches {
                    functions.push(func);
                }
            }
        }
    }

    Ok(functions)
}

/// Parsed LLVM struct type fields.
#[derive(Debug, Clone)]
struct IrStructDef {
    fields: Vec<String>,
    is_packed: bool,
}

/// Parse all `%name = type { ... }` and `%name = type <{ ... }>` definitions.
fn parse_struct_types(ir: &str) -> HashMap<String, IrStructDef> {
    let mut map = HashMap::new();
    for line in ir.lines() {
        let trimmed = line.trim();
        // Match: %struct.Name = type { i32, i64, i8* }
        // or:    %struct.Name = type <{ i32, i64 }>  (packed)
        if let Some(eq_pos) = trimmed.find(" = type ") {
            let name = trimmed[..eq_pos].trim();
            if !name.starts_with('%') {
                continue;
            }
            let body = trimmed[eq_pos + 8..].trim();
            let (is_packed, inner) = if body.starts_with("<{") && body.ends_with("}>") {
                (true, &body[2..body.len() - 2])
            } else if body.starts_with('{') && body.ends_with('}') {
                (false, &body[1..body.len() - 1])
            } else {
                continue;
            };
            let fields: Vec<String> = split_ir_params(inner)
                .into_iter()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            map.insert(
                name.to_string(),
                IrStructDef { fields, is_packed },
            );
        }
    }
    map
}

/// Compute the size in bytes of an LLVM IR type, given the struct definitions.
fn compute_ir_type_size(ty: &str, struct_map: &HashMap<String, IrStructDef>) -> Option<usize> {
    let t = ty.trim();
    match t {
        "void" => Some(0),
        "i1" => Some(1),
        "i8" => Some(1),
        "i16" => Some(2),
        "i32" => Some(4),
        "i64" => Some(8),
        "i128" => Some(16),
        "float" => Some(4),
        "double" => Some(8),
        "ptr" => Some(8), // 64-bit pointers
        t if t.ends_with('*') => Some(8),
        t if t.starts_with('[') => {
            // [N x type]
            if let Some(rest) = t.strip_prefix('[') {
                if let Some((count_str, inner_str)) = rest.split_once(" x ") {
                    if let Some(inner_str) = inner_str.strip_suffix(']') {
                        if let Ok(count) = count_str.trim().parse::<usize>() {
                            return compute_ir_type_size(inner_str.trim(), struct_map)
                                .map(|sz| count * sz);
                        }
                    }
                }
            }
            None
        }
        t if t.starts_with('%') => {
            if let Some(def) = struct_map.get(t) {
                compute_struct_size(def, struct_map)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Compute size of a struct with alignment padding.
fn compute_struct_size(
    def: &IrStructDef,
    struct_map: &HashMap<String, IrStructDef>,
) -> Option<usize> {
    if def.is_packed {
        // Packed structs have no padding
        let mut total = 0usize;
        for field in &def.fields {
            total += compute_ir_type_size(field, struct_map)?;
        }
        Some(total)
    } else {
        // Normal structs: fields aligned to their natural alignment
        let mut offset = 0usize;
        let mut max_align = 1usize;
        for field in &def.fields {
            let size = compute_ir_type_size(field, struct_map)?;
            let align = type_alignment(field, struct_map)?;
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
}

/// Get the natural alignment of an LLVM IR type.
fn type_alignment(ty: &str, struct_map: &HashMap<String, IrStructDef>) -> Option<usize> {
    let t = ty.trim();
    match t {
        "void" => Some(1),
        "i1" | "i8" => Some(1),
        "i16" => Some(2),
        "i32" | "float" => Some(4),
        "i64" | "double" | "ptr" => Some(8),
        "i128" => Some(16),
        t if t.ends_with('*') => Some(8),
        t if t.starts_with('[') => {
            // Array alignment = element alignment
            if let Some(rest) = t.strip_prefix('[') {
                if let Some((_, inner_str)) = rest.split_once(" x ") {
                    if let Some(inner_str) = inner_str.strip_suffix(']') {
                        return type_alignment(inner_str.trim(), struct_map);
                    }
                }
            }
            None
        }
        t if t.starts_with('%') => {
            if let Some(def) = struct_map.get(t) {
                if def.is_packed {
                    Some(1)
                } else {
                    def.fields
                        .iter()
                        .map(|f| type_alignment(f, struct_map))
                        .try_fold(1usize, |acc, a| a.map(|a| acc.max(a)))
                }
            } else {
                None
            }
        }
        _ => None,
    }
}

fn parse_ir_function(line: &str, struct_map: &HashMap<String, IrStructDef>) -> Option<FunctionInfo> {
    let _is_define = line.starts_with("define ");

    // Extract attributes that appear before the return type
    let has_nounwind = line.contains("nounwind");

    // Parse: declare/define [linkage] [visibility] [ret_type] @name(params) [attrs]
    let at_pos = line.find('@')?;
    let paren_open = line[at_pos..].find('(')? + at_pos;
    // Find matching close paren, handling nested parens (e.g. sret(%struct.Foo))
    let paren_close = find_matching_paren(line, paren_open)?;

    let name = line[at_pos + 1..paren_open].to_string();
    let params_str = &line[paren_open + 1..paren_close];

    // Extract return type: everything between the keyword+linkage and @name
    let ret_str = extract_return_type(line, at_pos);
    let return_type = parse_ir_type_resolved(&ret_str, struct_map);

    // Parse parameters
    let params = parse_ir_params_resolved(params_str, struct_map);

    // Detect sret parameter: first param with sret attribute means the function
    // actually returns via pointer. Split it out as an output parameter.
    let (sret_param, regular_params, effective_return) = extract_sret(&params, &return_type);

    let mut final_params = Vec::new();
    if let Some(sp) = sret_param {
        final_params.push(sp);
    }
    final_params.extend(regular_params);

    // Extract function attributes (after closing paren)
    let attrs_str = &line[paren_close + 1..];
    let mut annotations = Vec::new();

    if has_nounwind || attrs_str.contains("nounwind") {
        annotations.push(Annotation::NoThrow);
    }

    Some(FunctionInfo {
        name,
        mangled_name: None,
        params: final_params,
        return_type: effective_return,
        can_throw: !has_nounwind,
        annotations,
    })
}

/// Find the matching close paren for an open paren, handling nesting.
fn find_matching_paren(s: &str, open_pos: usize) -> Option<usize> {
    let mut depth = 0;
    for (i, c) in s[open_pos..].char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open_pos + i);
                }
            }
            _ => {}
        }
    }
    None
}

/// Detect and extract sret parameter from parameter list.
/// Returns (sret_param_as_output, remaining_params, effective_return_type).
fn extract_sret(
    params: &[ParamInfo],
    original_return: &TypeInfo,
) -> (Option<ParamInfo>, Vec<ParamInfo>, TypeInfo) {
    if let Some((idx, sret_param)) = params
        .iter()
        .enumerate()
        .find(|(_, p)| p.annotations.iter().any(|a| matches!(a, Annotation::Custom(s) if s == "sret")))
    {
        // The sret parameter is an output pointer for the struct return
        let out_param = ParamInfo {
            name: sret_param.name.clone(),
            ty: sret_param.ty.clone(),
            mutability: Mutability::WriteOnly,
            nullable: Nullability::NonNull,
            annotations: sret_param
                .annotations
                .iter()
                .filter(|a| !matches!(a, Annotation::Custom(s) if s == "sret"))
                .cloned()
                .collect(),
        };
        let remaining: Vec<ParamInfo> = params
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != idx)
            .map(|(_, p)| p.clone())
            .collect();
        // The effective return type is void since the result goes through the sret pointer
        (Some(out_param), remaining, TypeInfo::Void)
    } else {
        (None, params.to_vec(), original_return.clone())
    }
}

fn extract_return_type(line: &str, at_pos: usize) -> String {
    // Skip "declare " or "define " prefix
    let after_keyword = if line.starts_with("declare ") {
        &line[8..at_pos]
    } else if line.starts_with("define ") {
        &line[7..at_pos]
    } else {
        return "void".to_string();
    };

    // Skip optional linkage/visibility/etc keywords
    let parts: Vec<&str> = after_keyword.split_whitespace().collect();
    let linkage_keywords = [
        "private",
        "internal",
        "available_externally",
        "linkonce",
        "weak",
        "common",
        "appending",
        "extern_weak",
        "linkonce_odr",
        "weak_odr",
        "external",
        "default",
        "hidden",
        "protected",
        "dso_local",
        "dso_preemptable",
        "ccc",
        "fastcc",
        "coldcc",
        "x86_stdcallcc",
        "win64cc",
        "swiftcc",
        "zeroext",
        "signext",
        "noundef",
    ];

    let type_parts: Vec<&str> = parts
        .into_iter()
        .filter(|p| !linkage_keywords.contains(p) && !p.starts_with('#'))
        .collect();

    type_parts.join(" ").trim().to_string()
}

fn parse_ir_params_resolved(
    params_str: &str,
    struct_map: &HashMap<String, IrStructDef>,
) -> Vec<ParamInfo> {
    if params_str.trim().is_empty() || params_str.trim() == "..." {
        return Vec::new();
    }

    let mut params = Vec::new();
    let mut param_idx = 0;

    for param_str in split_ir_params(params_str) {
        let param_str = param_str.trim();
        if param_str == "..." {
            continue;
        }
        if let Some(param) = parse_ir_param_resolved(param_str, param_idx, struct_map) {
            params.push(param);
        }
        param_idx += 1;
    }

    params
}

/// Split parameter list respecting nested types like { i32, i64 }.
fn split_ir_params(s: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut current = String::new();
    let mut depth = 0;

    for c in s.chars() {
        match c {
            '{' | '<' | '(' => {
                depth += 1;
                current.push(c);
            }
            '}' | '>' | ')' => {
                depth -= 1;
                current.push(c);
            }
            ',' if depth == 0 => {
                result.push(current.trim().to_string());
                current.clear();
            }
            _ => current.push(c),
        }
    }
    if !current.trim().is_empty() {
        result.push(current.trim().to_string());
    }
    result
}

fn parse_ir_param_resolved(
    param_str: &str,
    idx: usize,
    struct_map: &HashMap<String, IrStructDef>,
) -> Option<ParamInfo> {
    let parts: Vec<&str> = param_str.split_whitespace().collect();
    if parts.is_empty() {
        return None;
    }

    // Detect attributes
    let mut is_readonly = false;
    let mut is_writeonly = false;
    let mut is_noalias = false;
    let mut is_nocapture = false;
    let mut is_nonnull = false;
    let mut is_sret = false;
    let mut deref_size: Option<usize> = None;
    let mut type_str = String::new();
    let mut name = format!("arg{idx}");

    for part in &parts {
        match *part {
            "readonly" => is_readonly = true,
            "writeonly" => is_writeonly = true,
            "noalias" => is_noalias = true,
            "nocapture" => is_nocapture = true,
            "nonnull" => is_nonnull = true,
            "zeroext" | "signext" | "noundef" | "inreg" | "align" => {}
            p if p.starts_with("sret(") || p == "sret" => {
                is_sret = true;
            }
            p if p.starts_with("dereferenceable(") => {
                let n = p
                    .trim_start_matches("dereferenceable(")
                    .trim_end_matches(')')
                    .parse()
                    .ok();
                deref_size = n;
            }
            p if (p.starts_with('%') || p.starts_with('@'))
                && !p.ends_with('*')
                && !type_str.is_empty() =>
            {
                // This is a parameter name (e.g. %x, %retval) — only if we
                // already have a type. If type_str is empty, treat as type ref.
                name = p.trim_start_matches('%').to_string();
            }
            _ => {
                if !type_str.is_empty() {
                    type_str.push(' ');
                }
                type_str.push_str(part);
            }
        }
    }

    let is_ptr = type_str.contains('*') || type_str == "ptr";
    let inner_ty = parse_ir_type_resolved(&type_str, struct_map);
    let ty = if is_ptr {
        TypeInfo::Pointer(Box::new(inner_ty))
    } else {
        inner_ty
    };

    let mutability = if is_writeonly || is_sret {
        Mutability::WriteOnly
    } else if is_readonly {
        Mutability::Readonly
    } else if is_ptr {
        Mutability::Mutable
    } else {
        Mutability::Readonly
    };

    let nullable = if is_nonnull || is_sret || !is_ptr {
        Nullability::NonNull
    } else {
        Nullability::Nullable
    };

    let mut annotations = Vec::new();
    if is_noalias {
        annotations.push(Annotation::NoAlias);
    }
    if is_nocapture {
        annotations.push(Annotation::NoCapture);
    }
    if let Some(n) = deref_size {
        annotations.push(Annotation::Dereferenceable(n));
    }
    if is_sret {
        annotations.push(Annotation::Custom("sret".into()));
    }

    Some(ParamInfo {
        name,
        ty,
        mutability,
        nullable,
        annotations,
    })
}

fn parse_ir_type_resolved(type_str: &str, struct_map: &HashMap<String, IrStructDef>) -> TypeInfo {
    let t = type_str.trim().trim_end_matches('*');
    match t {
        "void" => TypeInfo::Void,
        "i1" => TypeInfo::Bool,
        "i8" => TypeInfo::SignedInt(8),
        "i16" => TypeInfo::SignedInt(16),
        "i32" => TypeInfo::SignedInt(32),
        "i64" => TypeInfo::SignedInt(64),
        "i128" => TypeInfo::SignedInt(128),
        "ptr" => TypeInfo::Pointer(Box::new(TypeInfo::UnsignedInt(8))),
        t if t.starts_with('[') => {
            // Array type: [N x type]
            if let Some(rest) = t.strip_prefix('[') {
                if let Some((count_str, inner_str)) = rest.split_once(" x ") {
                    if let Some(inner_str) = inner_str.strip_suffix(']') {
                        if let Ok(count) = count_str.parse::<usize>() {
                            let inner = parse_ir_type_resolved(inner_str, struct_map);
                            if matches!(inner, TypeInfo::SignedInt(8) | TypeInfo::UnsignedInt(8)) {
                                return TypeInfo::ByteArray(count);
                            }
                        }
                    }
                }
            }
            TypeInfo::Opaque {
                name: t.into(),
                size_bytes: 0,
            }
        }
        t if t.starts_with('%') => {
            let display_name = t.trim_start_matches('%').trim_matches('"');
            // Look up in struct_map to get fields and size
            let key = t.to_string();
            if let Some(def) = struct_map.get(&key) {
                let size = compute_struct_size(def, struct_map);
                let fields: Vec<(String, TypeInfo)> = def
                    .fields
                    .iter()
                    .enumerate()
                    .map(|(i, f)| {
                        let field_ty = parse_ir_type_resolved(f, struct_map);
                        (format!("field{i}"), field_ty)
                    })
                    .collect();
                TypeInfo::Struct {
                    name: display_name.into(),
                    size_bytes: size,
                    fields,
                }
            } else {
                TypeInfo::Opaque {
                    name: display_name.into(),
                    size_bytes: 0,
                }
            }
        }
        _ => TypeInfo::Opaque {
            name: t.into(),
            size_bytes: 0,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_declare() {
        let ir = "declare i32 @strlen(i8* nocapture readonly) nounwind\n";
        let funcs = extract_functions(ir, None).unwrap();
        assert_eq!(funcs.len(), 1);
        assert_eq!(funcs[0].name, "strlen");
        assert!(!funcs[0].can_throw);
        assert_eq!(funcs[0].return_type, TypeInfo::SignedInt(32));
    }

    #[test]
    fn test_parse_define() {
        let ir = "define dso_local i32 @add(i32 %a, i32 %b) {\n";
        let funcs = extract_functions(ir, None).unwrap();
        assert_eq!(funcs.len(), 1);
        assert_eq!(funcs[0].name, "add");
        assert_eq!(funcs[0].params.len(), 2);
    }

    #[test]
    fn test_parse_void_function() {
        let ir = "declare void @free(i8*)\n";
        let funcs = extract_functions(ir, None).unwrap();
        assert_eq!(funcs[0].return_type, TypeInfo::Void);
    }

    #[test]
    fn test_parse_nounwind() {
        let ir = "declare void @safe_fn() nounwind\n";
        let funcs = extract_functions(ir, None).unwrap();
        assert!(!funcs[0].can_throw);
        assert!(funcs[0]
            .annotations
            .iter()
            .any(|a| matches!(a, Annotation::NoThrow)));
    }

    #[test]
    fn test_parse_readonly_param() {
        let ir = "declare i32 @read_fn(i8* readonly)\n";
        let funcs = extract_functions(ir, None).unwrap();
        assert_eq!(funcs[0].params[0].mutability, Mutability::Readonly);
    }

    #[test]
    fn test_parse_noalias_nocapture() {
        let ir = "declare void @process(i8* noalias nocapture)\n";
        let funcs = extract_functions(ir, None).unwrap();
        assert!(funcs[0].params[0]
            .annotations
            .iter()
            .any(|a| matches!(a, Annotation::NoAlias)));
        assert!(funcs[0].params[0]
            .annotations
            .iter()
            .any(|a| matches!(a, Annotation::NoCapture)));
    }

    #[test]
    fn test_parse_ptr_type() {
        let ir = "declare void @write_fn(i32* writeonly)\n";
        let funcs = extract_functions(ir, None).unwrap();
        assert!(matches!(funcs[0].params[0].ty, TypeInfo::Pointer(_)));
        assert_eq!(funcs[0].params[0].mutability, Mutability::WriteOnly);
    }

    #[test]
    fn test_parse_nonnull() {
        let ir = "declare void @nonnull_fn(i8* nonnull)\n";
        let funcs = extract_functions(ir, None).unwrap();
        assert_eq!(funcs[0].params[0].nullable, Nullability::NonNull);
    }

    #[test]
    fn test_filter() {
        let ir = "declare void @foo()\ndeclare void @bar()\n";
        let funcs = extract_functions(ir, Some("foo")).unwrap();
        assert_eq!(funcs.len(), 1);
        assert_eq!(funcs[0].name, "foo");
    }

    #[test]
    fn test_parse_ir_type_scalars() {
        let empty = HashMap::new();
        assert_eq!(parse_ir_type_resolved("void", &empty), TypeInfo::Void);
        assert_eq!(parse_ir_type_resolved("i1", &empty), TypeInfo::Bool);
        assert_eq!(parse_ir_type_resolved("i32", &empty), TypeInfo::SignedInt(32));
        assert_eq!(parse_ir_type_resolved("i64", &empty), TypeInfo::SignedInt(64));
    }

    #[test]
    fn test_parse_ir_type_array() {
        assert_eq!(parse_ir_type_resolved("[16 x i8]", &HashMap::new()), TypeInfo::ByteArray(16));
    }

    #[test]
    fn test_parse_ir_type_named() {
        if let TypeInfo::Opaque { name, .. } = parse_ir_type_resolved("%struct.MyData", &HashMap::new()) {
            assert_eq!(name, "struct.MyData");
        } else {
            panic!("Expected Opaque type");
        }
    }

    #[test]
    fn test_split_ir_params() {
        let params = split_ir_params("i32, i64, i8*");
        assert_eq!(params.len(), 3);

        let params = split_ir_params("{ i32, i64 }, i8*");
        assert_eq!(params.len(), 2);
    }

    #[test]
    fn test_parse_multiple_functions() {
        let ir = "\
declare void @fn1(i32)
declare void @fn2(i64)
define dso_local void @fn3(i32 %x) {
ret void
}
";
        let funcs = extract_functions(ir, None).unwrap();
        assert_eq!(funcs.len(), 3);
    }

    // ========================================================================
    // Item #1: Rich type layout from LLVM IR struct definitions
    // ========================================================================

    #[test]
    fn test_parse_struct_type_definition() {
        let ir = "\
%struct.Point = type { i32, i32 }
declare void @use_point(%struct.Point*)
";
        let funcs = extract_functions(ir, None).unwrap();
        assert_eq!(funcs.len(), 1);
        // The parameter should reference a Struct with resolved fields and size
        if let TypeInfo::Pointer(inner) = &funcs[0].params[0].ty {
            if let TypeInfo::Struct {
                name,
                size_bytes,
                fields,
            } = inner.as_ref()
            {
                assert_eq!(name, "struct.Point");
                assert_eq!(*size_bytes, Some(8)); // 2 x i32 = 8 bytes
                assert_eq!(fields.len(), 2);
            } else {
                panic!("Expected Struct type, got {:?}", inner);
            }
        } else {
            panic!("Expected Pointer type");
        }
    }

    #[test]
    fn test_parse_struct_with_padding() {
        let ir = "\
%struct.Padded = type { i8, i64 }
declare void @use_padded(%struct.Padded*)
";
        let funcs = extract_functions(ir, None).unwrap();
        if let TypeInfo::Pointer(inner) = &funcs[0].params[0].ty {
            if let TypeInfo::Struct { size_bytes, .. } = inner.as_ref() {
                // i8 (1 byte) + 7 bytes padding + i64 (8 bytes) = 16 bytes
                assert_eq!(*size_bytes, Some(16));
            } else {
                panic!("Expected Struct");
            }
        } else {
            panic!("Expected Pointer");
        }
    }

    #[test]
    fn test_parse_packed_struct() {
        let ir = "\
%struct.Packed = type <{ i8, i64 }>
declare void @use_packed(%struct.Packed*)
";
        let funcs = extract_functions(ir, None).unwrap();
        if let TypeInfo::Pointer(inner) = &funcs[0].params[0].ty {
            if let TypeInfo::Struct { size_bytes, .. } = inner.as_ref() {
                // packed: i8 (1) + i64 (8) = 9 bytes, no padding
                assert_eq!(*size_bytes, Some(9));
            } else {
                panic!("Expected Struct");
            }
        } else {
            panic!("Expected Pointer");
        }
    }

    #[test]
    fn test_parse_nested_struct() {
        let ir = "\
%struct.Inner = type { i32, i32 }
%struct.Outer = type { %struct.Inner, i64 }
declare void @use_outer(%struct.Outer*)
";
        let funcs = extract_functions(ir, None).unwrap();
        if let TypeInfo::Pointer(inner) = &funcs[0].params[0].ty {
            if let TypeInfo::Struct {
                name, size_bytes, ..
            } = inner.as_ref()
            {
                assert_eq!(name, "struct.Outer");
                // Inner: { i32, i32 } = 8 bytes, align 4
                // Outer: { Inner(8), i64(8) } = 16 bytes
                assert_eq!(*size_bytes, Some(16));
            } else {
                panic!("Expected Struct");
            }
        } else {
            panic!("Expected Pointer");
        }
    }

    #[test]
    fn test_struct_type_used_in_return() {
        let ir = "\
%struct.Result = type { i32, i32 }
define %struct.Result @get_result() {
ret %struct.Result zeroinitializer
}
";
        let funcs = extract_functions(ir, None).unwrap();
        if let TypeInfo::Struct {
            name, size_bytes, ..
        } = &funcs[0].return_type
        {
            assert_eq!(name, "struct.Result");
            assert_eq!(*size_bytes, Some(8));
        } else {
            panic!(
                "Expected Struct return type, got {:?}",
                funcs[0].return_type
            );
        }
    }

    // ========================================================================
    // Item #3: sret calling convention
    // ========================================================================

    #[test]
    fn test_sret_param_detection() {
        let ir = "\
%struct.BigResult = type { i64, i64, i64, i64 }
define void @get_big_result(ptr sret(%struct.BigResult) %retval, i32 %input) {
ret void
}
";
        let funcs = extract_functions(ir, None).unwrap();
        assert_eq!(funcs.len(), 1);
        let f = &funcs[0];
        // sret param should be first, modeled as writeonly output
        assert_eq!(f.params[0].name, "retval");
        assert_eq!(f.params[0].mutability, Mutability::WriteOnly);
        assert_eq!(f.params[0].nullable, Nullability::NonNull);
        // Second param is the regular input
        assert_eq!(f.params[1].name, "input");
        // Return type should be void since sret handles it
        assert_eq!(f.return_type, TypeInfo::Void);
    }

    #[test]
    fn test_sret_with_noalias() {
        let ir = "define void @make_thing(ptr noalias sret(%struct.Thing) %agg.result, i32 %x) {\nret void\n}\n";
        let funcs = extract_functions(ir, None).unwrap();
        let f = &funcs[0];
        assert_eq!(f.params[0].mutability, Mutability::WriteOnly);
        assert!(f.params[0]
            .annotations
            .iter()
            .any(|a| matches!(a, Annotation::NoAlias)));
        // sret annotation itself should be stripped from the output
        assert!(!f.params[0]
            .annotations
            .iter()
            .any(|a| matches!(a, Annotation::Custom(s) if s == "sret")));
    }

    #[test]
    fn test_no_sret_leaves_return_alone() {
        let ir = "define i32 @normal_fn(i32 %x) {\nret i32 0\n}\n";
        let funcs = extract_functions(ir, None).unwrap();
        assert_eq!(funcs[0].return_type, TypeInfo::SignedInt(32));
        assert_eq!(funcs[0].params.len(), 1);
    }

    #[test]
    fn test_compute_struct_sizes() {
        let mut map = HashMap::new();
        map.insert(
            "%struct.S1".to_string(),
            IrStructDef {
                fields: vec!["i32".into(), "i32".into()],
                is_packed: false,
            },
        );
        assert_eq!(compute_ir_type_size("%struct.S1", &map), Some(8));

        map.insert(
            "%struct.S2".to_string(),
            IrStructDef {
                fields: vec!["i8".into(), "i32".into()],
                is_packed: false,
            },
        );
        // i8 + 3 padding + i32 = 8
        assert_eq!(compute_ir_type_size("%struct.S2", &map), Some(8));

        map.insert(
            "%struct.S3".to_string(),
            IrStructDef {
                fields: vec!["i8".into(), "i32".into()],
                is_packed: true,
            },
        );
        // packed: i8 + i32 = 5
        assert_eq!(compute_ir_type_size("%struct.S3", &map), Some(5));
    }
}
