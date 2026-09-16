//! Exact LLVM storage leaves and deliberately limited source scalar spellings.

use super::super::{clang::normalize_name, data_layout::DataLayout};
use crate::llvm_ir::IrStructDef;
use anyhow::{ensure, Context, Result};
use std::collections::{BTreeMap, HashMap};

pub(super) const MAX_DEPTH: usize = 128;
pub(super) const MAX_ITEMS: usize = 100_000;

#[derive(Debug, Clone)]
pub(super) struct Leaf {
    pub offset: usize,
    /// Store size, not allocation stride (e.g. i24 occupies three bytes).
    pub size: usize,
    pub ty: String,
    pub pointer: bool,
}

#[derive(Debug, Clone)]
pub(super) struct Node {
    pub offset: usize,
    pub size: usize,
    pub alignment: usize,
    pub ty: String,
}

pub(super) struct Storage {
    pub leaves: BTreeMap<usize, Leaf>,
    pub nodes: Vec<Node>,
}

pub(super) fn named_ref(name: &str) -> String {
    // Cleaned keys retain LLVM hexadecimal escapes; never escape them again.
    format!("%\"{name}\"")
}

pub(super) fn inline_saw_type(
    ty: &str,
    defs: &HashMap<String, IrStructDef>,
    depth: usize,
) -> Result<String> {
    ensure!(depth < MAX_DEPTH, "inline compiler type nesting limit");
    if let Some(name) = named_key(ty) {
        let def = defs.get(name).context("missing compiler record type")?;
        let fields = def
            .fields
            .iter()
            .map(|f| inline_saw_type(f, defs, depth + 1))
            .collect::<Result<Vec<_>>>()?;
        return Ok(format!(
            "{} [{}]",
            if def.is_packed {
                "llvm_packed_struct_type"
            } else {
                "llvm_struct_type"
            },
            fields.join(", ")
        ));
    }
    if let Some((count, element)) = llvm_array(ty) {
        return Ok(format!(
            "llvm_array {count} ({})",
            inline_saw_type(element, defs, depth + 1)?
        ));
    }
    if ty == "ptr" {
        return Ok("llvm_pointer (llvm_int 8)".into());
    }
    if let Some(width) = ty
        .strip_prefix('i')
        .filter(|w| !w.is_empty() && w.bytes().all(|b| b.is_ascii_digit()))
    {
        return Ok(format!("llvm_int {width}"));
    }
    anyhow::bail!("unsupported inline compiler storage type {ty}")
}

pub(super) fn named_key(ty: &str) -> Option<&str> {
    let name = ty.trim().strip_prefix('%')?;
    if name.ends_with('*') {
        return None;
    }
    Some(
        name.strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .unwrap_or(name),
    )
}

pub(super) fn name_matches(llvm: &str, source: &str) -> bool {
    let llvm = ["class.", "struct.", "union."]
        .into_iter()
        .find_map(|tag| llvm.strip_prefix(tag))
        .unwrap_or(llvm);
    let llvm = normalize_name(llvm);
    let source = normalize_name(source);
    let template_base = source.split_once('<').map(|(base, _)| base);
    llvm == source
        || template_base.is_some_and(|base| llvm == base)
        || llvm.rsplit_once('.').is_some_and(|(base, suffix)| {
            !suffix.is_empty()
                && suffix.bytes().all(|b| b.is_ascii_digit())
                && (base == source || template_base == Some(base))
        })
}

pub(super) fn unique_name(defs: &HashMap<String, IrStructDef>, source: &str) -> Result<String> {
    let mut names: Vec<_> = defs
        .keys()
        .filter(|name| name_matches(name, source))
        .collect();
    names.sort();
    ensure!(
        !names.is_empty(),
        "missing LLVM named struct for source type {source}"
    );
    ensure!(
        names.len() == 1,
        "ambiguous LLVM named structs for {source}: {names:?}"
    );
    Ok(names[0].clone())
}

impl Storage {
    pub fn new(dl: &DataLayout, defs: &HashMap<String, IrStructDef>, ty: &str) -> Result<Self> {
        let mut result = Self {
            leaves: BTreeMap::new(),
            nodes: Vec::new(),
        };
        result.flatten(dl, defs, ty, 0, 0)?;
        Ok(result)
    }

    fn flatten(
        &mut self,
        dl: &DataLayout,
        defs: &HashMap<String, IrStructDef>,
        ty: &str,
        offset: usize,
        depth: usize,
    ) -> Result<()> {
        ensure!(depth < MAX_DEPTH, "LLVM storage nesting limit exceeded");
        ensure!(
            self.nodes.len() + self.leaves.len() < MAX_ITEMS,
            "LLVM storage expansion limit exceeded"
        );
        let ty = ty.trim();
        let layout = dl.layout_of(ty, defs)?;
        offset
            .checked_add(layout.size)
            .context("LLVM storage offset overflow")?;
        if is_pointer(ty) {
            ensure!(
                pointer_space(ty)? == 0,
                "nondefault-address-space pointer storage is unsupported: {ty}"
            );
            return self.leaf(offset, dl.pointer_size, "ptr", true);
        }
        let fields = if let Some(name) = named_key(ty) {
            Some(
                defs.get(name)
                    .context("missing LLVM named definition")?
                    .fields
                    .clone(),
            )
        } else {
            ty.strip_prefix("<{")
                .and_then(|s| s.strip_suffix("}>"))
                .or_else(|| ty.strip_prefix('{').and_then(|s| s.strip_suffix('}')))
                .map(split_fields)
        };
        if let Some(fields) = fields {
            self.nodes.push(Node {
                offset,
                size: layout.size,
                alignment: layout.alignment,
                ty: ty.into(),
            });
            ensure!(
                fields.len() == layout.offsets.len(),
                "LLVM struct field/offset mismatch: {ty}"
            );
            for (field, relative) in fields.iter().zip(layout.offsets) {
                self.flatten(
                    dl,
                    defs,
                    field,
                    offset
                        .checked_add(relative)
                        .context("LLVM field offset overflow")?,
                    depth + 1,
                )?;
            }
        } else if let Some((count, element)) = llvm_array(ty) {
            self.nodes.push(Node {
                offset,
                size: layout.size,
                alignment: layout.alignment,
                ty: ty.into(),
            });
            ensure!(count <= MAX_ITEMS, "LLVM array expansion limit exceeded");
            let stride = dl.layout_of(element, defs)?.size;
            for index in 0..count {
                let relative = index
                    .checked_mul(stride)
                    .context("LLVM array stride overflow")?;
                self.flatten(
                    dl,
                    defs,
                    element,
                    offset
                        .checked_add(relative)
                        .context("LLVM array offset overflow")?,
                    depth + 1,
                )?;
            }
        } else {
            let size = scalar_store_size(ty)
                .with_context(|| format!("unsupported LLVM storage leaf {ty}"))?;
            self.leaf(offset, size, ty, false)?;
        }
        Ok(())
    }

    fn leaf(&mut self, offset: usize, size: usize, ty: &str, pointer: bool) -> Result<()> {
        let previous = self.leaves.insert(
            offset,
            Leaf {
                offset,
                size,
                ty: ty.into(),
                pointer,
            },
        );
        ensure!(
            previous.is_none(),
            "overlapping LLVM storage leaves at offset {offset}"
        );
        Ok(())
    }

    pub fn named_at(&self, source: &str, offset: usize) -> Result<Option<&Node>> {
        let nodes: Vec<_> = self
            .nodes
            .iter()
            .filter(|node| {
                node.offset == offset
                    && named_key(&node.ty).is_some_and(|key| name_matches(key, source))
            })
            .collect();
        ensure!(
            nodes.len() <= 1,
            "ambiguous LLVM subobject for {source} at offset {offset}"
        );
        Ok(nodes.first().copied())
    }
}

fn is_pointer(ty: &str) -> bool {
    ty.ends_with('*')
        || ty
            .strip_prefix("ptr")
            .is_some_and(|s| s.is_empty() || s.starts_with(char::is_whitespace))
}

fn pointer_space(ty: &str) -> Result<u32> {
    let suffix = if let Some(suffix) = ty.strip_prefix("ptr") {
        suffix.trim()
    } else {
        let pointee = ty.strip_suffix('*').context("invalid pointer type")?.trim();
        if !pointee.ends_with(')') {
            return Ok(0);
        }
        let Some(index) = pointee.rfind("addrspace") else {
            return Ok(0);
        };
        &pointee[index..]
    };
    if suffix.is_empty() {
        return Ok(0);
    }
    let compact: String = suffix.chars().filter(|c| !c.is_whitespace()).collect();
    compact
        .strip_prefix("addrspace(")
        .and_then(|s| s.strip_suffix(')'))
        .context("unsupported pointer address space syntax")?
        .parse()
        .context("invalid pointer address space")
}

fn scalar_store_size(ty: &str) -> Option<usize> {
    if let Some(bits) = ty.strip_prefix('i').and_then(|s| s.parse::<usize>().ok()) {
        return Some(bits.div_ceil(8));
    }
    match ty {
        "half" | "bfloat" => Some(2),
        "float" => Some(4),
        "double" => Some(8),
        "x86_fp80" => Some(10),
        "fp128" | "ppc_fp128" => Some(16),
        _ => None,
    }
}

fn split_fields(text: &str) -> Vec<String> {
    let mut result = Vec::new();
    let (mut start, mut depth, mut quoted) = (0, 0usize, false);
    for (index, ch) in text.char_indices() {
        if ch == '"' {
            quoted = !quoted;
        } else if !quoted {
            match ch {
                '{' | '[' | '<' | '(' => depth += 1,
                '}' | ']' | '>' | ')' => depth = depth.saturating_sub(1),
                ',' if depth == 0 => {
                    result.push(text[start..index].trim().into());
                    start = index + 1;
                }
                _ => {}
            }
        }
    }
    if !text[start..].trim().is_empty() {
        result.push(text[start..].trim().into());
    }
    result
}

pub(super) fn llvm_array(ty: &str) -> Option<(usize, &str)> {
    let inner = ty.trim().strip_prefix('[')?.strip_suffix(']')?.trim();
    let end = inner.bytes().take_while(|b| b.is_ascii_digit()).count();
    let count = inner[..end].parse().ok()?;
    let element = inner[end..].trim_start().strip_prefix('x')?.trim();
    Some((count, element))
}

pub(super) fn clean_source(source: &str) -> String {
    let normalized = normalize_name(source);
    let mut source = normalized.as_str();
    let qualifiers = [
        "const",
        "volatile",
        "restrict",
        "__restrict",
        "__restrict__",
        "mutable",
    ];
    loop {
        if let Some(tail) = qualifiers.iter().find_map(|word| {
            source
                .strip_prefix(*word)
                .filter(|tail| tail.starts_with(char::is_whitespace))
        }) {
            source = tail.trim_start();
        } else if let Some(head) = qualifiers.iter().find_map(|word| {
            source
                .strip_suffix(*word)
                .filter(|head| head.ends_with(char::is_whitespace))
        }) {
            source = head.trim_end();
        } else {
            return source.to_owned();
        }
    }
}

/// C arrays are outermost-dimension-first: T[2][3] contains two T[3]s.
pub(super) fn source_array(source: &str) -> Result<Option<(usize, String)>> {
    let source = clean_source(source);
    let mut rest = source.as_str();
    let mut counts = Vec::new();
    while let Some(head) = rest.strip_suffix(']') {
        let (base, count) = head.rsplit_once('[').context("malformed source array")?;
        ensure!(
            !count.is_empty() && count.bytes().all(|b| b.is_ascii_digit()),
            "unknown source array bound: {source}"
        );
        counts.push(
            count
                .parse::<usize>()
                .context("source array count overflow")?,
        );
        rest = base.trim_end();
    }
    if counts.is_empty() || (rest.ends_with(')') && (rest.contains('*') || rest.contains('&'))) {
        return Ok(None); // Pointer/reference to an array, not array storage.
    }
    ensure!(!rest.is_empty(), "missing source array element type");
    let count = counts.pop().context("missing source array count")?;
    let mut element = rest.to_owned();
    for dimension in counts.into_iter().rev() {
        element.push_str(&format!("[{dimension}]"));
    }
    Ok(Some((count, element)))
}

pub(super) struct Scalar {
    pub ty: String,
    pub size: usize,
    pub boolean: bool,
    pub pointer: bool,
}

impl Scalar {
    pub fn matches(&self, leaf: &Leaf) -> bool {
        self.size == leaf.size
            && self.pointer == leaf.pointer
            && (self.ty == leaf.ty || (self.boolean && leaf.ty == "i1"))
    }
}

pub(super) fn source_scalar(source: &str, triple: &str, pointer_size: usize) -> Option<Scalar> {
    let source = clean_source(source);
    let mut angles = 0usize;
    for (index, ch) in source.char_indices() {
        match ch {
            '<' => angles += 1,
            '>' => angles = angles.saturating_sub(1),
            '*' | '&' if angles == 0 => {
                if source[..index].ends_with("::") {
                    return None;
                }
                return Some(Scalar {
                    ty: "ptr".into(),
                    size: pointer_size,
                    boolean: false,
                    pointer: true,
                });
            }
            _ => {}
        }
    }
    let boolean = matches!(source.as_str(), "bool" | "_Bool");
    let primitive = match source.as_str() {
        "float" => Some(("float".to_owned(), 4)),
        "double" => Some(("double".to_owned(), 8)),
        _ => None,
    };
    if let Some((ty, size)) = primitive {
        return Some(Scalar {
            ty,
            size,
            boolean: false,
            pointer: false,
        });
    }
    let words: Vec<_> = source.split_whitespace().collect();
    let stem = words
        .iter()
        .copied()
        .filter(|word| !matches!(*word, "signed" | "unsigned" | "int"))
        .collect::<Vec<_>>()
        .join(" ");
    let windows = triple
        .split('-')
        .any(|part| matches!(part, "windows" | "win32" | "msvc" | "mingw32"));
    let unix = triple.split('-').any(|part| {
        matches!(
            part,
            "linux" | "darwin" | "freebsd" | "netbsd" | "openbsd" | "android"
        )
    });
    let bits = if boolean {
        8
    } else {
        match stem.as_str() {
            "char" | "char8_t" | "__int8" => 8,
            "short" | "char16_t" | "__int16" => 16,
            "" if !words.is_empty() => 32,
            "char32_t" | "__int32" => 32,
            "long" if windows || (unix && pointer_size == 4) => 32,
            "long" if unix && pointer_size == 8 => 64,
            "long long" | "__int64" => 64,
            "__int128" => 128,
            "wchar_t" if windows => 16,
            "wchar_t" if unix => 32,
            _ => return None,
        }
    };
    Some(Scalar {
        ty: format!("i{bits}"),
        size: bits / 8,
        boolean,
        pointer: false,
    })
}

pub(super) fn shifted(
    member: &super::RecordMember,
    old: usize,
    new: usize,
) -> Result<super::RecordMember> {
    fn shift(
        member: &super::RecordMember,
        old: usize,
        new: usize,
        depth: usize,
    ) -> Result<super::RecordMember> {
        ensure!(depth < MAX_DEPTH, "Clang member nesting limit exceeded");
        let mut result = member.clone();
        result.offset = member
            .offset
            .checked_sub(old)
            .and_then(|n| n.checked_add(new))
            .context("Clang descendant offset precedes its subobject or overflows")?;
        result.children = member
            .children
            .iter()
            .map(|child| shift(child, old, new, depth + 1))
            .collect::<Result<_>>()?;
        Ok(result)
    }
    shift(member, old, new, 0)
}
