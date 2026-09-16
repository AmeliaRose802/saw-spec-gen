//! Compiler-layout checks and the deliberately small legacy SAW shape grammar.

use super::super::data_layout::DataLayout;
use super::{covers, FieldLayout, ObjectPlan, Projection};
use crate::llvm_ir::{struct_defs, IrStructDef};
use anyhow::{bail, ensure, Context, Result};
use std::collections::{BTreeMap, HashMap};

#[path = "validate_bitfields.rs"]
mod bitfields;
pub(super) use bitfields::{check_alias_sizes, guards, is_path};
use bitfields::{check_fields, integer_bits};

const MAX_DEPTH: usize = 128;
const MAX_ITEMS: usize = 100_000;

/// Check the semantic leaves and any explicit allocation shape atomically.
/// Selectors use `$` for the base: e.g. `($.0 @ 2).1`, never `$.0 @ 2.1`.
pub fn validate_object(object: &mut ObjectPlan, ir: &str) -> Result<()> {
    let lines: Vec<_> = ir
        .lines()
        .filter(|line| {
            line.trim()
                .split_once('=')
                .is_some_and(|(key, _)| key.trim() == "target datalayout")
        })
        .collect();
    ensure!(
        lines.len() == 1,
        "object {} requires one LLVM target datalayout directive",
        object.region
    );
    let dl = DataLayout::parse(lines[0])?;
    let defs = struct_defs(ir); // Keys are cleaned; never use parse_struct_types here.
    check_fields(object, &dl, &defs)?;
    let mut projection = object.projection.clone();
    let mut selectors = BTreeMap::new();
    if let Some(shape) = object
        .configured_shape
        .as_ref()
        .or(object.inferred_shape.as_ref())
    {
        let mut parser = SawType { rest: shape };
        let ty = parser
            .ty(0)
            .with_context(|| format!("configured shape for {}", object.region))?;
        ensure!(
            parser.rest.trim().is_empty(),
            "trailing configured shape syntax: {}",
            parser.rest
        );
        let layout = dl.layout_of(&ty, &defs)?;
        ensure!(layout.size == object.layout.size,
            "configured shape for {} has {} bytes; compiler size is exactly {} (neither undersize nor oversize is allowed)",
            object.region, layout.size, object.layout.size);
        ensure!(
            layout.alignment <= object.layout.alignment,
            "configured shape alignment {} exceeds compiler alignment {} for {}",
            layout.alignment,
            object.layout.alignment,
            object.region
        );
        if array_parts(&ty).is_some_and(|(_, element)| element == "i8") {
            // An explicit byte count may assert extent while fields projection
            // still supplies typed leaves. It does not force a record to bytes.
            if projection == Projection::Llvm {
                projection = Projection::Bytes;
            }
        } else {
            projection = Projection::Llvm;
            ensure!(!object.layout.fields.iter().any(|f| f.bit_width.is_some()),
                "bitfields in legacy non-byte LLVM shapes are unsupported; use bytes or fields projection");
            ensure!(!object.layout.fields.iter().any(|f| f.guard.is_some()),
                "guarded optionals in legacy LLVM shapes are unsupported; use bytes or fields projection to avoid asserting inactive payloads");
            ensure!(!object.layout.fields.iter().any(|f| f.is_pointer),
                "legacy LLVM shapes cannot freshen pointer fields as Cryptol terms; use framed fields projection");
            let fields = object.layout.fields.iter().map(|f| (f.offset, f)).collect();
            let mut walker = ShapeWalk {
                dl: &dl,
                defs: &defs,
                object,
                fields,
                selectors: BTreeMap::new(),
                visits: 0,
            };
            walker.walk(&ty, Some(0), "$", 0)?;
            ensure!(
                walker.selectors.len() == object.layout.fields.len(),
                "configured shape for {} omits semantic fields: {:?}",
                object.region,
                object
                    .layout
                    .fields
                    .iter()
                    .filter(|f| !walker.selectors.contains_key(&f.path))
                    .map(|f| &f.path)
                    .collect::<Vec<_>>()
            );
            selectors = walker.selectors;
        }
    } else {
        ensure!(
            projection != Projection::Llvm,
            "LLVM projection requires an explicit validated non-byte shape"
        );
    }
    ensure!(
        projection != Projection::Bytes || dl.little_endian,
        "byte projection requires a little-endian target"
    );
    for field in object.layout.fields.iter().filter(|f| f.is_pointer) {
        ensure!(projection == Projection::Fields && object.region != "return"
            && object.framed.iter().any(|p| covers(p, &field.path)),
            "pointer field {}.{} must be framed unchanged in fields projection, never encoded as integer bytes or fresh LLVM terms",
            object.region, field.path);
    }
    object.projection = projection;
    object.selectors = selectors;
    Ok(())
}

struct SawType<'a> {
    rest: &'a str,
}
impl SawType<'_> {
    fn take(&mut self, text: &str) -> bool {
        self.rest = self.rest.trim_start();
        if let Some(rest) = self.rest.strip_prefix(text) {
            self.rest = rest;
            true
        } else {
            false
        }
    }
    fn expect(&mut self, text: &str) -> Result<()> {
        ensure!(self.take(text), "expected {text:?} near {:?}", self.rest);
        Ok(())
    }
    fn word(&mut self) -> Result<String> {
        self.rest = self.rest.trim_start();
        let end = self
            .rest
            .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
            .unwrap_or(self.rest.len());
        ensure!(end > 0, "expected SAW type token near {:?}", self.rest);
        let word = self.rest[..end].to_owned();
        self.rest = &self.rest[end..];
        Ok(word)
    }
    fn number(&mut self) -> Result<usize> {
        let word = self.word()?;
        ensure!(
            word.bytes().all(|b| b.is_ascii_digit()),
            "expected unsigned type count/width, got {word}"
        );
        word.parse().context("SAW type count/width overflow")
    }
    fn ty(&mut self, depth: usize) -> Result<String> {
        ensure!(depth < MAX_DEPTH, "SAW type nesting limit exceeded");
        if self.take("(") {
            let ty = self.ty(depth + 1)?;
            self.expect(")")?;
            return Ok(ty);
        }
        match self.word()?.as_str() {
            "llvm_int" => Ok(format!("i{}", self.number()?)),
            "llvm_array" => {
                let count = self.number()?;
                self.expect("(")?;
                let element = self.ty(depth + 1)?;
                self.expect(")")?;
                Ok(format!("[{count} x {element}]"))
            }
            kind @ ("llvm_struct_type" | "llvm_packed_struct_type") => {
                self.expect("[")?;
                let mut fields = Vec::new();
                if !self.take("]") {
                    loop {
                        ensure!(fields.len() < MAX_ITEMS, "SAW struct member limit exceeded");
                        fields.push(self.ty(depth + 1)?);
                        if self.take("]") { break; }
                        self.expect(",")?;
                    }
                }
                let body = format!("{{ {} }}", fields.join(", "));
                Ok(if kind == "llvm_packed_struct_type" { format!("<{body}>") } else { body })
            }
            "llvm_alias" | "llvm_struct" => {
                self.rest = self.rest.trim_start();
                ensure!(self.rest.starts_with('"'), "expected quoted LLVM struct name");
                let (mut end, bytes) = (1, self.rest.as_bytes());
                while end < bytes.len() && bytes[end] != b'"' {
                    if bytes[end] == b'\\' { end += 1; }
                    end += 1;
                }
                ensure!(end < bytes.len(), "unterminated LLVM struct name");
                let name: String = serde_json::from_str(&self.rest[..=end]).context("invalid quoted SAW struct name")?;
                self.rest = &self.rest[end + 1..];
                ensure!(!name.is_empty(), "empty LLVM struct name");
                Ok(format!("%\"{name}\""))
            }
            other => bail!("unsupported SAW allocation type {other}; expected an integer, array or struct shape"),
        }
    }
}

fn array_parts(ty: &str) -> Option<(usize, &str)> {
    let inner = ty.strip_prefix('[')?.strip_suffix(']')?.trim();
    let end = inner.bytes().take_while(u8::is_ascii_digit).count();
    let count = inner[..end].parse().ok()?;
    Some((count, inner[end..].trim_start().strip_prefix('x')?.trim()))
}

fn struct_fields(ty: &str, defs: &HashMap<String, IrStructDef>) -> Result<Option<Vec<String>>> {
    if let Some(name) = ty.strip_prefix('%') {
        return Ok(Some(
            defs.get(name.trim_matches('"'))
                .context("unknown LLVM struct alias")?
                .fields
                .clone(),
        ));
    }
    let Some(inner) = ty
        .strip_prefix("<{")
        .and_then(|s| s.strip_suffix("}>"))
        .or_else(|| ty.strip_prefix('{').and_then(|s| s.strip_suffix('}')))
    else {
        return Ok(None);
    };
    let (mut result, mut start, mut depth, mut quoted) = (Vec::new(), 0, 0usize, false);
    for (i, ch) in inner.char_indices() {
        if ch == '"' {
            quoted = !quoted;
        } else if !quoted {
            match ch {
                '[' | '{' | '<' | '(' => depth += 1,
                ']' | '}' | '>' | ')' => {
                    depth = depth.checked_sub(1).context("unbalanced LLVM struct")?
                }
                ',' if depth == 0 => {
                    result.push(inner[start..i].trim().into());
                    start = i + 1;
                }
                _ => {}
            }
        }
    }
    if !inner[start..].trim().is_empty() {
        result.push(inner[start..].trim().into());
    }
    Ok(Some(result))
}

struct ShapeWalk<'a> {
    dl: &'a DataLayout,
    defs: &'a HashMap<String, IrStructDef>,
    object: &'a ObjectPlan,
    fields: BTreeMap<usize, &'a FieldLayout>,
    selectors: BTreeMap<String, String>,
    visits: usize,
}
impl ShapeWalk<'_> {
    // None walks a zero-length array's element type without inventing a value.
    fn walk(
        &mut self,
        ty: &str,
        offset: Option<usize>,
        selector: &str,
        depth: usize,
    ) -> Result<()> {
        self.visits += 1;
        ensure!(
            depth < MAX_DEPTH && self.visits <= MAX_ITEMS,
            "legacy LLVM shape expansion limit exceeded"
        );
        let ty = ty.trim();
        let layout = self.dl.layout_of(ty, self.defs)?;
        if let Some(bits) = integer_bits(ty) {
            if let Some(offset) = offset {
                let field = self.fields.get(&offset)
                    .with_context(|| format!("configured shape contains an extra value-bearing padding/inactive scalar {ty} at offset {offset}"))?;
                ensure!(integer_bits(field.llvm_type.trim()) == Some(bits) && field.size == bits.div_ceil(8),
                    "configured scalar {ty} at offset {offset} does not match semantic field {}.{} ({}; {} bytes)",
                    self.object.region, field.path, field.llvm_type, field.size);
                ensure!(
                    self.selectors
                        .insert(field.path.clone(), selector.into())
                        .is_none(),
                    "duplicate configured scalar for {}",
                    field.path
                );
            }
        } else if let Some((count, element)) = array_parts(ty) {
            ensure!(count <= MAX_ITEMS, "legacy array expansion limit exceeded");
            let stride = self.dl.layout_of(element, self.defs)?.size;
            if count == 0 {
                self.walk(element, None, selector, depth + 1)?;
            }
            for index in 0..count {
                let next = offset
                    .map(|base| {
                        index
                            .checked_mul(stride)
                            .and_then(|n| base.checked_add(n))
                            .context("legacy array offset overflow")
                    })
                    .transpose()?;
                self.walk(element, next, &format!("({selector} @ {index})"), depth + 1)?;
            }
        } else if let Some(fields) = struct_fields(ty, self.defs)? {
            ensure!(
                fields.len() == layout.offsets.len(),
                "LLVM struct offset/member mismatch"
            );
            for (index, (field, relative)) in fields.iter().zip(layout.offsets).enumerate() {
                let next = offset
                    .map(|base| {
                        base.checked_add(relative)
                            .context("legacy struct offset overflow")
                    })
                    .transpose()?;
                self.walk(field, next, &format!("{selector}.{index}"), depth + 1)?;
            }
        } else {
            bail!("legacy LLVM shape cannot freshen {ty}; pointer and floating-point semantic terms are unsupported");
        }
        Ok(())
    }
}
