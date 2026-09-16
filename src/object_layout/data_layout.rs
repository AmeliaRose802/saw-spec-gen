//! Checked LLVM allocation layouts, independent of source-language `alignas`.
//! Unsupported types/specifiers are errors, not target-dependent guesses.

use crate::llvm_ir::IrStructDef;
use anyhow::{bail, ensure, Context, Result};
use std::collections::{BTreeMap, HashMap, HashSet};

const MAX_DEPTH: usize = 128;
const MAX_INTEGER_BITS: usize = (1 << 23) - 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeLayout {
    /// Allocation size in bytes, including ABI padding (the array stride).
    pub size: usize,
    /// LLVM ABI alignment in bytes, not preferred or source alignment.
    pub alignment: usize,
    /// Immediate struct member offsets; empty for scalars, pointers and arrays.
    pub offsets: Vec<usize>,
}

#[derive(Debug, Clone)]
pub struct DataLayout {
    pub little_endian: bool,
    /// Address-space-zero pointer representation size in bytes.
    pub pointer_size: usize,
    // Representation size and ABI alignment in bytes, keyed by address space.
    pointer_layout: HashMap<u32, (usize, usize)>,
    integer_align: BTreeMap<usize, usize>,
    float_align: BTreeMap<usize, usize>,
    aggregate_align: usize,
}

impl DataLayout {
    /// Parse a layout string (optionally quoted or a `target datalayout` line).
    /// Endianness must be explicit. Other defaults are LLVM's, not host ABI's.
    pub fn parse(text: &str) -> Result<Self> {
        let text = text.trim();
        let text = if let Some(rest) = text.strip_prefix("target datalayout") {
            rest.trim_start()
                .strip_prefix('=')
                .context("expected '=' after target datalayout")?
                .trim()
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .context("target datalayout requires a quoted string")?
        } else if text.starts_with('"') {
            text.strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .context("unterminated data layout string")?
        } else {
            text
        };
        let mut layout = Self {
            little_endian: false,
            pointer_size: 8,
            pointer_layout: HashMap::from([(0, (8, 8))]),
            integer_align: BTreeMap::from([(1, 1), (8, 1), (16, 2), (32, 4), (64, 4)]),
            float_align: BTreeMap::from([(16, 2), (32, 4), (64, 8), (128, 16)]),
            aggregate_align: 1, // a:0:64: preferred alignment is not ABI alignment.
        };
        let mut endian = None;
        for spec in text.split('-') {
            ensure!(!spec.is_empty(), "empty data layout component");
            if matches!(spec, "e" | "E") {
                ensure!(
                    endian.replace(spec == "e").is_none(),
                    "duplicate endianness"
                );
            } else {
                layout
                    .parse_spec(spec)
                    .with_context(|| format!("data layout '{spec}'"))?;
            }
        }
        layout.little_endian = endian.context("data layout requires explicit endianness (e/E)")?;
        layout.pointer_size = layout.pointer_layout[&0].0;
        Ok(layout)
    }

    fn parse_spec(&mut self, spec: &str) -> Result<()> {
        let parts: Vec<_> = spec.split(':').collect();
        let head = parts[0];
        if let Some(space) = head.strip_prefix('p') {
            ensure!(
                (3..=5).contains(&parts.len()),
                "invalid pointer specification"
            );
            let space = if space.is_empty() {
                0
            } else {
                address_space(space)?
            };
            let bits = number(parts[1])?;
            ensure!(
                bits > 0 && bits <= u32::MAX as usize && bits % 8 == 0,
                "invalid pointer size"
            );
            let alignment = abi_alignment(&parts[2..parts.len().min(4)], false)?;
            if let Some(index) = parts.get(4) {
                let index = number(index)?;
                ensure!(
                    index > 0 && index <= bits && index % 8 == 0,
                    "invalid pointer index size"
                );
            }
            self.pointer_layout.insert(space, (bits / 8, alignment));
        } else if matches!(head.as_bytes().first(), Some(b'i' | b'f' | b'v')) {
            let bits = number(&head[1..])?;
            ensure!(bits > 0 && bits < (1 << 24), "invalid alignment width");
            let alignment = abi_alignment(&parts[1..], false)?;
            match head.as_bytes()[0] {
                b'i' => {
                    ensure!(
                        bits != 8 || alignment == 1,
                        "i8 must have byte ABI alignment"
                    );
                    self.integer_align.insert(bits, alignment);
                }
                b'f' => {
                    self.float_align.insert(bits, alignment);
                }
                _ => {} // Vector specs are validated, but vector types are unsupported.
            }
        } else if matches!(head, "a" | "a0") {
            self.aggregate_align = abi_alignment(&parts[1..], true)?.max(1);
        } else if head == "ni" {
            ensure!(parts.len() > 1, "missing non-integral address spaces");
            for space in &parts[1..] {
                ensure!(
                    address_space(space)? != 0,
                    "non-integral address space zero"
                );
            }
        } else if let Some(first) = head.strip_prefix('n') {
            for width in std::iter::once(first).chain(parts[1..].iter().copied()) {
                let width = number(width)?;
                ensure!(
                    width > 0 && width <= u32::MAX as usize,
                    "invalid native width"
                );
            }
        } else if head == "m" {
            ensure!(
                parts.len() == 2 && matches!(parts[1], "e" | "m" | "o" | "w" | "x" | "a" | "l"),
                "unsupported mangling specification"
            );
        } else if let Some(alignment) = head.strip_prefix('S') {
            ensure!(parts.len() == 1, "invalid stack alignment specification");
            byte_alignment(alignment, true)?;
        } else if let Some(alignment) = head.strip_prefix("Fi").or_else(|| head.strip_prefix("Fn"))
        {
            ensure!(
                parts.len() == 1,
                "invalid function pointer alignment specification"
            );
            byte_alignment(alignment, true)?;
        } else if matches!(head.as_bytes().first(), Some(b'P' | b'G' | b'A')) {
            ensure!(
                parts.len() == 1,
                "invalid default address space specification"
            );
            address_space(&head[1..])?;
        } else {
            bail!("unsupported data layout component");
        }
        // m/n/ni/S/F/P/G/A do not change the allocation layout of a given type.
        Ok(())
    }

    /// Calculate allocation layout using ONLY cleaned `llvm_ir::struct_defs` keys.
    /// Even zero-length arrays validate their element. Named typed-pointer
    /// pointees must be declared, but their layouts are not traversed.
    pub fn layout_of(&self, ty: &str, defs: &HashMap<String, IrStructDef>) -> Result<TypeLayout> {
        let ty = TypeParser::parse(ty, defs)?;
        let mut state = LayoutState {
            defs,
            active: HashSet::new(),
            cache: HashMap::new(),
        };
        self.calculate(&ty, &mut state, 0)
    }

    /// Allocation layout for an address space; unspecified spaces inherit p0.
    pub fn pointer_layout(&self, space: u32) -> Result<TypeLayout> {
        ensure!(space < (1 << 24), "address space out of range");
        let &(size, alignment) = self
            .pointer_layout
            .get(&space)
            .or_else(|| self.pointer_layout.get(&0))
            .context("missing default pointer layout")?;
        allocated(size, alignment, Vec::new())
    }

    fn calculate(
        &self,
        ty: &IrType,
        state: &mut LayoutState<'_>,
        depth: usize,
    ) -> Result<TypeLayout> {
        ensure!(depth < MAX_DEPTH, "type layout nesting limit exceeded");
        match ty {
            IrType::Integer(bits) => {
                let (_, alignment) = self
                    .integer_align
                    .range(*bits..)
                    .next()
                    .or_else(|| self.integer_align.last_key_value())
                    .context("missing integer alignment")?;
                allocated(bits.div_ceil(8), *alignment, Vec::new())
            }
            IrType::Float(bits) => {
                let alignment = self
                    .float_align
                    .get(bits)
                    .with_context(|| format!("no explicit/default f{bits} ABI alignment"))?;
                allocated(bits.div_ceil(8), *alignment, Vec::new())
            }
            IrType::Pointer(space) => self.pointer_layout(*space),
            IrType::Array(count, element) => {
                let element = self.calculate(element, state, depth + 1)?;
                let size = element.size.checked_mul(*count).context("array overflow")?;
                allocated(size, element.alignment, Vec::new())
            }
            IrType::Struct(fields, packed) => self.struct_layout(fields, *packed, state, depth),
            IrType::Named(name) => {
                if let Some(layout) = state.cache.get(name) {
                    return Ok(layout.clone());
                }
                ensure!(
                    state.active.insert(name.clone()),
                    "by-value cycle at %{name}"
                );
                let def = state.defs.get(name).context("missing named struct")?;
                let fields = def
                    .fields
                    .iter()
                    .map(|f| TypeParser::parse(f, state.defs))
                    .collect::<Result<Vec<_>>>()?;
                let result = self.struct_layout(&fields, def.is_packed, state, depth + 1);
                state.active.remove(name);
                let layout = result.with_context(|| format!("layout of %{name}"))?;
                state.cache.insert(name.clone(), layout.clone());
                Ok(layout)
            }
        }
    }

    fn struct_layout(
        &self,
        fields: &[IrType],
        packed: bool,
        state: &mut LayoutState<'_>,
        depth: usize,
    ) -> Result<TypeLayout> {
        let mut size = 0;
        let mut alignment = if packed { 1 } else { self.aggregate_align };
        let mut offsets = Vec::new();
        for field in fields {
            let field = self.calculate(field, state, depth + 1)?;
            let field_align = if packed { 1 } else { field.alignment };
            size = align_up(size, field_align)?;
            offsets.push(size);
            // LLVM uses each field's allocation size, including in packed structs.
            size = size
                .checked_add(field.size)
                .context("struct size overflow")?;
            alignment = alignment.max(field_align);
        }
        allocated(size, alignment, offsets)
    }
}

fn number(text: &str) -> Result<usize> {
    ensure!(
        !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()),
        "expected unsigned decimal integer: '{text}'"
    );
    text.parse().context("integer size overflow")
}

fn address_space(text: &str) -> Result<u32> {
    let space = number(text)?;
    ensure!(space < (1 << 24), "address space out of range");
    Ok(space as u32)
}

fn byte_alignment(text: &str, zero: bool) -> Result<usize> {
    let bits = number(text)?;
    if zero && bits == 0 {
        return Ok(0);
    }
    ensure!(
        bits <= u32::MAX as usize && bits % 8 == 0 && (bits / 8).is_power_of_two(),
        "alignment must be a nonzero power-of-two byte multiple"
    );
    Ok(bits / 8)
}

fn abi_alignment(parts: &[&str], zero: bool) -> Result<usize> {
    ensure!(
        (1..=2).contains(&parts.len()),
        "expected abi[:pref] alignment"
    );
    let abi = byte_alignment(parts[0], zero)?;
    let pref = if parts.len() == 2 {
        byte_alignment(parts[1], zero)?
    } else {
        abi
    };
    ensure!(
        pref >= abi,
        "preferred alignment is less than ABI alignment"
    );
    Ok(abi)
}

fn align_up(size: usize, alignment: usize) -> Result<usize> {
    ensure!(alignment.is_power_of_two(), "invalid byte alignment");
    let padding = (alignment - size % alignment) % alignment;
    size.checked_add(padding)
        .context("alignment rounding overflow")
}

fn allocated(size: usize, alignment: usize, offsets: Vec<usize>) -> Result<TypeLayout> {
    let size = align_up(size, alignment)?;
    ensure!(
        size.checked_mul(8).is_some(),
        "LLVM layout bit size overflow"
    );
    Ok(TypeLayout {
        size,
        alignment,
        offsets,
    })
}

struct LayoutState<'a> {
    defs: &'a HashMap<String, IrStructDef>,
    active: HashSet<String>,
    cache: HashMap<String, TypeLayout>,
}

enum IrType {
    Integer(usize),
    Float(usize),
    Pointer(u32),
    Array(usize, Box<IrType>),
    Struct(Vec<IrType>, bool),
    Named(String),
}

struct TypeParser<'a> {
    rest: &'a str,
    defs: &'a HashMap<String, IrStructDef>,
}

impl<'a> TypeParser<'a> {
    fn parse(text: &'a str, defs: &'a HashMap<String, IrStructDef>) -> Result<IrType> {
        let mut parser = Self { rest: text, defs };
        let ty = parser.ty(0)?;
        ensure!(
            parser.rest.trim().is_empty(),
            "trailing type syntax: '{}'",
            parser.rest
        );
        Ok(ty)
    }

    fn take(&mut self, token: &str) -> bool {
        self.rest = self.rest.trim_start();
        if let Some(rest) = self.rest.strip_prefix(token) {
            self.rest = rest;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, token: &str) -> Result<()> {
        ensure!(self.take(token), "expected '{token}' near '{}'", self.rest);
        Ok(())
    }

    fn word(&mut self) -> Result<&'a str> {
        self.rest = self.rest.trim_start();
        let end = self
            .rest
            .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
            .unwrap_or(self.rest.len());
        ensure!(end != 0, "expected type token near '{}'", self.rest);
        let (word, rest) = self.rest.split_at(end);
        self.rest = rest;
        Ok(word)
    }

    fn space(&mut self) -> Result<Option<u32>> {
        if !self.take("addrspace") {
            return Ok(None);
        }
        self.expect("(")?;
        let space = address_space(self.word()?)?;
        self.expect(")")?;
        Ok(Some(space))
    }

    fn ty(&mut self, depth: usize) -> Result<IrType> {
        ensure!(depth < MAX_DEPTH, "type syntax nesting limit exceeded");
        let mut ty = if self.take("[") {
            let count = number(self.word()?)?;
            ensure!(self.word()? == "x", "expected array 'x'");
            let element = self.ty(depth + 1)?;
            self.expect("]")?;
            IrType::Array(count, Box::new(element))
        } else if self.take("{") {
            IrType::Struct(self.fields(depth)?, false)
        } else if self.take("<") {
            ensure!(
                self.take("{"),
                "fixed/scalable vector layouts are unsupported"
            );
            let fields = self.fields(depth)?;
            self.expect(">")?;
            IrType::Struct(fields, true)
        } else if self.take("%") {
            let name = self.name()?;
            ensure!(self.defs.contains_key(&name), "missing struct %{name}");
            IrType::Named(name)
        } else {
            let word = self.word()?;
            match word {
                "ptr" => return Ok(IrType::Pointer(self.space()?.unwrap_or(0))),
                "half" | "bfloat" => IrType::Float(16),
                "float" => IrType::Float(32),
                "double" => IrType::Float(64),
                "x86_fp80" => IrType::Float(80),
                "fp128" | "ppc_fp128" => IrType::Float(128),
                _ if word.starts_with('i') => {
                    let bits = number(&word[1..])?;
                    ensure!(
                        (1..=MAX_INTEGER_BITS).contains(&bits),
                        "invalid integer width"
                    );
                    IrType::Integer(bits)
                }
                _ => bail!("unsupported or unsized LLVM type '{word}'"),
            }
        };
        // Validate typed pointee syntax without walking its layout (e.g. %Node*).
        let mut pointers = 0;
        loop {
            let space = self.space()?;
            if !self.take("*") {
                ensure!(space.is_none(), "typed addrspace requires '*'");
                break;
            }
            pointers += 1;
            ensure!(pointers < MAX_DEPTH, "pointer nesting limit exceeded");
            ty = IrType::Pointer(space.unwrap_or(0));
        }
        Ok(ty)
    }

    fn fields(&mut self, depth: usize) -> Result<Vec<IrType>> {
        let mut fields = Vec::new();
        if self.take("}") {
            return Ok(fields);
        }
        loop {
            fields.push(self.ty(depth + 1)?);
            if self.take("}") {
                return Ok(fields);
            }
            self.expect(",")?;
        }
    }

    fn name(&mut self) -> Result<String> {
        if let Some(rest) = self.rest.strip_prefix('"') {
            let bytes = rest.as_bytes();
            let mut end = 0;
            while end < bytes.len() {
                match bytes[end] {
                    b'"' => {
                        ensure!(end != 0, "empty struct name");
                        // Keep escape spelling: struct_defs strips quotes, not escapes.
                        let name = rest[..end].to_string();
                        self.rest = &rest[end + 1..];
                        return Ok(name);
                    }
                    b'\\' => {
                        ensure!(
                            bytes
                                .get(end + 1..end + 3)
                                .is_some_and(|s| s.iter().all(u8::is_ascii_hexdigit)),
                            "invalid LLVM name escape"
                        );
                        end += 3;
                    }
                    0 | b'\n' | b'\r' => bail!("invalid quoted struct name"),
                    _ => end += 1,
                }
            }
            bail!("unterminated quoted struct name");
        }
        let end = self
            .rest
            .find(|c: char| !c.is_ascii_alphanumeric() && !"-.$_".contains(c))
            .unwrap_or(self.rest.len());
        ensure!(end != 0, "missing struct name");
        let name = &self.rest[..end];
        ensure!(
            !name.as_bytes()[0].is_ascii_digit() || name.bytes().all(|b| b.is_ascii_digit()),
            "invalid numeric struct name"
        );
        self.rest = &self.rest[end..];
        Ok(name.to_string())
    }
}

#[cfg(test)]
#[path = "data_layout_tests.rs"]
mod tests;
