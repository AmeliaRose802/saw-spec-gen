//! Parse LLVM struct type definitions and compute their byte sizes /
//! natural alignments under the x64 ABI.

use super::tokens::split_ir_params;
use std::collections::HashMap;

/// A parsed `%name = type { ... }` (or packed `<{ ... }>`) declaration.
#[derive(Debug, Clone)]
pub struct IrStructDef {
    pub fields: Vec<String>,
    pub is_packed: bool,
}

/// Parse every `%name = type { ... }` / `%name = type <{ ... }>`
/// declaration in the IR text into a name → definition map.
pub fn parse_struct_types(ir: &str) -> HashMap<String, IrStructDef> {
    let mut map = HashMap::new();
    for line in ir.lines() {
        let trimmed = line.trim();
        let Some(eq_pos) = trimmed.find(" = type ") else {
            continue;
        };
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
        map.insert(name.to_string(), IrStructDef { fields, is_packed });
    }
    map
}

/// Return `name → size_in_bytes` for every struct definition, with the
/// `%` prefix and any surrounding `"` quotes stripped so callers can
/// match on the canonical form they see in SAW (`struct.X`).
pub fn struct_sizes(ir: &str) -> HashMap<String, usize> {
    let defs = parse_struct_types(ir);
    let mut sizes = HashMap::new();
    for (raw_name, def) in &defs {
        if let Some(size) = compute_struct_size(def, &defs) {
            let cleaned = raw_name
                .trim_start_matches('%')
                .trim_matches('"')
                .to_string();
            sizes.insert(cleaned, size);
        }
    }
    sizes
}

/// Compute the size in bytes of an LLVM IR type. Returns `None` for
/// unknown types or unresolved struct references.
pub fn compute_ir_type_size(ty: &str, struct_map: &HashMap<String, IrStructDef>) -> Option<usize> {
    let t = ty.trim();
    match t {
        "void" => Some(0),
        "i1" | "i8" => Some(1),
        "i16" => Some(2),
        "i32" | "float" => Some(4),
        "i64" | "double" | "ptr" => Some(8),
        "i128" => Some(16),
        t if t.ends_with('*') => Some(8),
        t if t.starts_with('[') => array_size(t, struct_map),
        t if t.starts_with('%') => struct_map.get(t).and_then(|d| compute_struct_size(d, struct_map)),
        _ => None,
    }
}

/// Natural alignment (1, 2, 4, 8, or 16 bytes) for an LLVM IR type.
pub fn type_alignment(ty: &str, struct_map: &HashMap<String, IrStructDef>) -> Option<usize> {
    let t = ty.trim();
    match t {
        "void" | "i1" | "i8" => Some(1),
        "i16" => Some(2),
        "i32" | "float" => Some(4),
        "i64" | "double" | "ptr" => Some(8),
        "i128" => Some(16),
        t if t.ends_with('*') => Some(8),
        t if t.starts_with('[') => array_alignment(t, struct_map),
        t if t.starts_with('%') => struct_map.get(t).and_then(|def| {
            if def.is_packed {
                Some(1)
            } else {
                def.fields
                    .iter()
                    .map(|f| type_alignment(f, struct_map))
                    .try_fold(1usize, |acc, a| a.map(|a| acc.max(a)))
            }
        }),
        _ => None,
    }
}

/// Compute the size of a struct definition. Honors `is_packed` and pads
/// to natural alignment otherwise.
pub fn compute_struct_size(
    def: &IrStructDef,
    struct_map: &HashMap<String, IrStructDef>,
) -> Option<usize> {
    if def.is_packed {
        let mut total = 0usize;
        for field in &def.fields {
            total += compute_ir_type_size(field, struct_map)?;
        }
        return Some(total);
    }
    let mut offset = 0usize;
    let mut max_align = 1usize;
    for field in &def.fields {
        let size = compute_ir_type_size(field, struct_map)?;
        let align = type_alignment(field, struct_map)?;
        max_align = max_align.max(align);
        let remainder = offset % align;
        if remainder != 0 {
            offset += align - remainder;
        }
        offset += size;
    }
    let remainder = offset % max_align;
    if remainder != 0 {
        offset += max_align - remainder;
    }
    Some(offset)
}

/// `[N x T]` size = N * sizeof(T).
fn array_size(t: &str, struct_map: &HashMap<String, IrStructDef>) -> Option<usize> {
    let (count, inner) = parse_array_shape(t)?;
    compute_ir_type_size(inner, struct_map).map(|sz| count * sz)
}

/// `[N x T]` alignment = alignment(T).
fn array_alignment(t: &str, struct_map: &HashMap<String, IrStructDef>) -> Option<usize> {
    let (_, inner) = parse_array_shape(t)?;
    type_alignment(inner, struct_map)
}

/// Split `[N x T]` into `(N, T)`. Shared by every callable that needs
/// to recurse into the element type. Returns `None` on malformed input.
pub fn parse_array_shape(t: &str) -> Option<(usize, &str)> {
    let rest = t.strip_prefix('[')?;
    let (count_str, inner_str) = rest.split_once(" x ")?;
    let inner = inner_str.strip_suffix(']')?;
    let count: usize = count_str.trim().parse().ok()?;
    Some((count, inner.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_struct() {
        let ir = "%struct.Point = type { i32, i32 }\n";
        let m = parse_struct_types(ir);
        assert_eq!(m.len(), 1);
        let def = &m["%struct.Point"];
        assert_eq!(def.fields.len(), 2);
        assert!(!def.is_packed);
    }

    #[test]
    fn parses_packed_struct() {
        let ir = "%struct.Packed = type <{ i8, i64 }>\n";
        let m = parse_struct_types(ir);
        assert!(m["%struct.Packed"].is_packed);
    }

    #[test]
    fn struct_size_no_padding_for_aligned_fields() {
        let mut m = HashMap::new();
        m.insert(
            "%S".into(),
            IrStructDef {
                fields: vec!["i32".into(), "i32".into()],
                is_packed: false,
            },
        );
        assert_eq!(compute_struct_size(&m["%S"], &m), Some(8));
    }

    #[test]
    fn struct_size_with_padding() {
        let mut m = HashMap::new();
        m.insert(
            "%S".into(),
            IrStructDef {
                fields: vec!["i8".into(), "i64".into()],
                is_packed: false,
            },
        );
        // i8 (1) + 7 padding + i64 (8) = 16
        assert_eq!(compute_struct_size(&m["%S"], &m), Some(16));
    }

    #[test]
    fn packed_struct_has_no_padding() {
        let mut m = HashMap::new();
        m.insert(
            "%S".into(),
            IrStructDef {
                fields: vec!["i8".into(), "i64".into()],
                is_packed: true,
            },
        );
        assert_eq!(compute_struct_size(&m["%S"], &m), Some(9));
    }

    #[test]
    fn nested_struct_sizes_propagate() {
        let mut m = HashMap::new();
        m.insert(
            "%inner".into(),
            IrStructDef {
                fields: vec!["i32".into(), "i32".into()],
                is_packed: false,
            },
        );
        m.insert(
            "%outer".into(),
            IrStructDef {
                fields: vec!["%inner".into(), "i64".into()],
                is_packed: false,
            },
        );
        // inner = 8 (aligned to 4); outer = inner(8) + i64(8) = 16
        assert_eq!(compute_struct_size(&m["%outer"], &m), Some(16));
    }

    #[test]
    fn struct_sizes_strips_prefix_and_quotes() {
        let ir = r#"%"struct.foo::Bar" = type { i32 }
"#;
        let sizes = struct_sizes(ir);
        assert_eq!(sizes.get("struct.foo::Bar"), Some(&4));
    }

    #[test]
    fn array_size_handles_byte_arrays() {
        let m = HashMap::new();
        assert_eq!(compute_ir_type_size("[16 x i8]", &m), Some(16));
        assert_eq!(compute_ir_type_size("[4 x i32]", &m), Some(16));
    }

    #[test]
    fn type_alignment_per_primitive() {
        let m = HashMap::new();
        assert_eq!(type_alignment("i8", &m), Some(1));
        assert_eq!(type_alignment("i16", &m), Some(2));
        assert_eq!(type_alignment("i32", &m), Some(4));
        assert_eq!(type_alignment("i64", &m), Some(8));
        assert_eq!(type_alignment("ptr", &m), Some(8));
        assert_eq!(type_alignment("i128", &m), Some(16));
        assert_eq!(type_alignment("[3 x i16]", &m), Some(2));
    }

    #[test]
    fn parse_array_shape_extracts_count_and_inner() {
        assert_eq!(parse_array_shape("[4 x i32]"), Some((4, "i32")));
        assert_eq!(parse_array_shape("[16 x i8]"), Some((16, "i8")));
        assert_eq!(parse_array_shape("not an array"), None);
    }
}
