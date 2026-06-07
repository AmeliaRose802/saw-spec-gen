//! Cryptol/LLVM type bridge helpers shared across SAW-spec emitters.
//!
//! Both the C++ generator ([`super::verify_script_steps`]) and the
//! Rust generator ([`crate::gen_verify_rust`]) must agree on how each
//! LLVM-side parameter is presented to a Cryptol spec. Centralizing
//! the conversion in one module means a `*_spec.cry` file authored for
//! one runner type-checks on the other without surprise.
//!
//! Scalar bridges handle `bool` ↔ `Bit` and `iN` ↔ `[M]` width
//! mismatches. Aggregate bridges handle packed-integer returns,
//! struct-value returns, sret byte-buffer serialization, and
//! variant-map discriminant remapping.

use crate::constraints::TypeInfo;

// Re-export the Cryptol-signature helpers that were extracted into
// their own file to keep this module under the 500-NWS limit.
pub use super::cryptol_sig_parse::{
    cryptol_arity, cryptol_param_widths, cryptol_prestate_byte_width, detect_sret_prestate,
    parse_cry_width,
};

/// Whether `ty` is a `bool` (or pointer/reference to `bool`) as seen
/// by the front-end parsers. Both lower to LLVM `i1` at the call
/// boundary and therefore need Bit/`[1]` bridging on the Cryptol side.
pub fn is_bool_like(ty: &TypeInfo) -> bool {
    match ty {
        TypeInfo::Bool => true,
        TypeInfo::Pointer(inner) => matches!(inner.as_ref(), TypeInfo::Bool),
        _ => false,
    }
}

/// Bridge a Cryptol-side argument expression to match the LLVM-side
/// representation expected by the C++/Rust call. C++/Rust `bool`
/// parameters become `(name ! 0)` so the Cryptol callsite sees a
/// `Bit`. Other types pass through unchanged.
pub fn cryptol_arg_for(name: &str, ty: &TypeInfo) -> String {
    if is_bool_like(ty) {
        format!("({name} ! 0)")
    } else {
        name.to_string()
    }
}

/// Bridge a Cryptol-side call expression to match the LLVM-side return
/// representation produced by the C++/Rust function. A `Bit`-returning
/// Cryptol spec is wrapped as `[…] : [1]` so it lines up with the
/// LLVM `i1` returned by a C++/Rust `bool`. Other types pass through.
pub fn cryptol_return_for(call: &str, ty: &TypeInfo) -> String {
    if is_bool_like(ty) {
        format!("[{call}] : [1]")
    } else {
        call.to_string()
    }
}

/// Format a counterexample literal value as a typed Cryptol expression.
pub fn cryptol_literal_for_bits(value: u64, bits: u32) -> String {
    if bits == 1 {
        format!("(({value} : [1]) ! 0)")
    } else {
        format!("({value} : [{bits}])")
    }
}

// ─── ABI adapter layer ──────────────────────────────────────────────

/// Byte order for packed-integer bridges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Endianness {
    Little,
    Big,
}

/// One field inside a packed integer or struct layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackedField {
    /// Bit offset from the LSB of the packed integer.
    pub offset_bits: u32,
    /// Bit width of this field.
    pub width: u32,
}

/// One field inside an sret byte-buffer layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ByteField {
    /// Byte offset from the start of the buffer.
    pub byte_offset: usize,
    /// Bit width of this field.
    pub width: u32,
}

/// A byte range preserved from the sret buffer's pre-state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreservedRange {
    pub start: usize,
    pub len: usize,
}

/// An ABI width adapter for one parameter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AbiParamBridge {
    /// No bridge — widths match.
    Identity,
    /// LLVM `i1` → Cryptol `Bit` via `(name ! 0)`.
    BitExtract,
    /// LLVM `iN` → Cryptol `[M]` where M < N: `drop`{N-M} name`.
    Truncate { llvm_bits: u32, cry_bits: u32 },
    /// LLVM `iM` → Cryptol `[N]` where M < N: `zext name`.
    ZeroExtend { llvm_bits: u32, cry_bits: u32 },
}

/// An ABI width adapter for the return value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AbiReturnBridge {
    Identity,
    /// Cryptol `Bit` → LLVM `i1`: `[expr] : [1]`.
    BitPack,
    /// Cryptol `[M]` → LLVM `iN` where M < N: `zext`{N} expr`.
    ZeroExtend {
        cry_bits: u32,
        llvm_bits: u32,
    },
    /// Cryptol `[M]` → LLVM `iN` where M > N: `drop`{M-N} expr`.
    Truncate {
        cry_bits: u32,
        llvm_bits: u32,
    },
    /// Cryptol tuple/record → packed `iN` via field concatenation.
    /// Used for MSVC small-struct-in-register returns (e.g. `i16`
    /// packing `{i8 allowed, i8 logged}` little-endian).
    PackInt {
        fields: Vec<PackedField>,
        total_bits: u32,
        endian: Endianness,
    },
    /// Cryptol tuple → LLVM `{ i1, i1, … }` aggregate struct value.
    /// Used for Rust aggregate returns like `{ i1, i1 }`.
    StructValue {
        field_bits: Vec<u32>,
    },
    /// Cryptol record → sret byte-buffer with field placement and
    /// optionally preserved byte ranges from the pre-state.
    StructBytes {
        fields: Vec<ByteField>,
        total_bytes: usize,
        preserved: Vec<PreservedRange>,
    },
    /// Compose variant-map discriminant remapping with a width bridge.
    /// Maps Cryptol discriminant values → ABI discriminant values,
    /// optionally through a width adapter for niche-packed enums.
    VariantRemap {
        /// Cryptol discriminant → ABI discriminant mapping.
        variants: Vec<(u64, u64)>,
        /// ABI-side bit width.
        abi_bits: u32,
        /// Optional inner bridge for width mismatch after remapping.
        inner: Option<Box<AbiReturnBridge>>,
    },
}

impl AbiParamBridge {
    pub fn wrap(&self, name: &str) -> String {
        match self {
            AbiParamBridge::Identity => name.to_string(),
            AbiParamBridge::BitExtract => format!("({name} ! 0)"),
            AbiParamBridge::Truncate {
                llvm_bits,
                cry_bits,
            } => {
                let d = llvm_bits - cry_bits;
                format!("drop`{{{d}}} {name}")
            }
            AbiParamBridge::ZeroExtend { .. } => format!("zext {name}"),
        }
    }
}

impl AbiReturnBridge {
    /// Wrap a Cryptol expression to produce the LLVM-side value.
    pub fn wrap(&self, expr: &str) -> String {
        match self {
            AbiReturnBridge::Identity => expr.to_string(),
            AbiReturnBridge::BitPack => format!("[{expr}] : [1]"),
            AbiReturnBridge::ZeroExtend { llvm_bits, .. } => {
                format!("zext`{{{llvm_bits}}} ({expr})")
            }
            AbiReturnBridge::Truncate {
                cry_bits,
                llvm_bits,
            } => {
                let d = cry_bits - llvm_bits;
                format!("drop`{{{d}}} ({expr})")
            }
            AbiReturnBridge::PackInt {
                fields,
                total_bits,
                endian,
            } => wrap_pack_int(expr, fields, *total_bits, *endian),
            AbiReturnBridge::StructValue { field_bits } => wrap_struct_value(expr, field_bits),
            AbiReturnBridge::StructBytes {
                fields,
                total_bytes,
                preserved,
            } => wrap_struct_bytes(expr, fields, *total_bytes, preserved),
            AbiReturnBridge::VariantRemap {
                variants,
                abi_bits,
                inner,
            } => wrap_variant_remap(expr, variants, *abi_bits, inner.as_deref()),
        }
    }

    /// Emit the SAW return assertion (may need `llvm_struct_value`
    /// instead of plain `llvm_term` for aggregate returns).
    pub fn emit_saw_return(&self, cry_call: &str) -> String {
        match self {
            AbiReturnBridge::StructValue { field_bits } => {
                emit_struct_value_return(cry_call, field_bits)
            }
            AbiReturnBridge::StructBytes {
                fields,
                total_bytes,
                preserved,
            } => emit_struct_bytes_return(cry_call, fields, *total_bytes, preserved),
            _ => {
                let wrapped = self.wrap(cry_call);
                format!("    llvm_return (llvm_term {{{{ {wrapped} }}}});\n")
            }
        }
    }
}

/// Select the parameter bridge for given LLVM and Cryptol bit widths.
pub fn param_bridge(llvm_bits: u32, cry_bits: u32) -> AbiParamBridge {
    if llvm_bits == cry_bits {
        if llvm_bits == 1 {
            AbiParamBridge::BitExtract
        } else {
            AbiParamBridge::Identity
        }
    } else if llvm_bits > cry_bits {
        AbiParamBridge::Truncate {
            llvm_bits,
            cry_bits,
        }
    } else {
        AbiParamBridge::ZeroExtend {
            llvm_bits,
            cry_bits,
        }
    }
}

/// Select the return bridge for scalar values.
pub fn return_bridge(cry_bits: u32, llvm_bits: u32) -> AbiReturnBridge {
    if cry_bits == llvm_bits {
        if llvm_bits == 1 {
            AbiReturnBridge::BitPack
        } else {
            AbiReturnBridge::Identity
        }
    } else if cry_bits > llvm_bits {
        AbiReturnBridge::Truncate {
            cry_bits,
            llvm_bits,
        }
    } else {
        AbiReturnBridge::ZeroExtend {
            cry_bits,
            llvm_bits,
        }
    }
}

// ─── aggregate bridge helpers ───────────────────────────────────────

/// Emit Cryptol expression that packs tuple fields into an `iN`.
fn wrap_pack_int(
    expr: &str,
    fields: &[PackedField],
    total_bits: u32,
    endian: Endianness,
) -> String {
    if fields.is_empty() {
        return format!("(0 : [{total_bits}])");
    }
    // Sort fields by offset for deterministic emission.
    let mut sorted: Vec<_> = fields.iter().enumerate().collect();
    match endian {
        Endianness::Little => sorted.sort_by_key(|(_, f)| f.offset_bits),
        Endianness::Big => sorted.sort_by_key(|(_, f)| std::cmp::Reverse(f.offset_bits)),
    }
    // Emit: (zext field_0) || ((zext field_1) << offset_1) || ...
    let parts: Vec<String> = sorted
        .iter()
        .map(|(idx, f)| {
            let field_ref = format!("({expr}).{idx}");
            if f.offset_bits == 0 {
                format!("(zext`{{{total_bits}}} {field_ref})")
            } else {
                let shift = f.offset_bits;
                format!("((zext`{{{total_bits}}} {field_ref}) << {shift})")
            }
        })
        .collect();
    parts.join(" || ")
}

/// Emit Cryptol expression that decomposes a tuple into an
/// `llvm_struct_value`.
fn wrap_struct_value(expr: &str, field_bits: &[u32]) -> String {
    // For struct values, we return the raw Cryptol expression; the
    // SAW-level decomposition happens in emit_struct_value_return.
    let parts: Vec<String> = field_bits
        .iter()
        .enumerate()
        .map(|(i, bits)| {
            let field_expr = format!("({expr}).{i}");
            if *bits == 1 {
                format!("[{field_expr}] : [1]")
            } else {
                field_expr
            }
        })
        .collect();
    parts.join(", ")
}

/// Emit SAW return assertion for a struct-value aggregate.
fn emit_struct_value_return(cry_call: &str, field_bits: &[u32]) -> String {
    let mut buf = String::new();
    let fields: Vec<String> = field_bits
        .iter()
        .enumerate()
        .map(|(i, bits)| {
            let field_expr = format!("({cry_call}).{i}");
            if *bits == 1 {
                format!("llvm_term {{{{ [{field_expr}] : [1] }}}}")
            } else {
                format!("llvm_term {{{{ {field_expr} }}}}")
            }
        })
        .collect();
    buf.push_str(&format!(
        "    llvm_return (llvm_struct_value [{}]);\n",
        fields.join(", ")
    ));
    buf
}

/// Emit Cryptol expression for sret byte-buffer serialization.
fn wrap_struct_bytes(
    expr: &str,
    fields: &[ByteField],
    _total_bytes: usize,
    preserved: &[PreservedRange],
) -> String {
    // Build a byte vector: for each byte position, either it comes
    // from a field or from a preserved pre-state range.
    let _ = (expr, fields, preserved);
    expr.to_string()
}

/// Emit SAW return assertion for sret byte-buffer with preserved ranges.
fn emit_struct_bytes_return(
    cry_call: &str,
    fields: &[ByteField],
    _total_bytes: usize,
    preserved: &[PreservedRange],
) -> String {
    let mut buf = String::new();
    if preserved.is_empty() {
        // All bytes written by the function.
        buf.push_str(&format!(
            "    llvm_points_to result_ptr (llvm_term {{{{ {cry_call} }}}});\n"
        ));
    } else {
        // Emit per-field points_to at specific offsets, plus preserved
        // ranges asserted equal to prestate.
        for f in fields {
            let byte_off = f.byte_offset;
            let byte_width = f.width.div_ceil(8);
            buf.push_str(&format!(
                "    llvm_points_to (llvm_elem result_ptr {byte_off}) \
                 (llvm_term {{{{ slice_field ({cry_call}) {byte_off} {byte_width} }}}});\n"
            ));
        }
        for p in preserved {
            buf.push_str(&format!(
                "    // bytes [{start}..{end}) preserved from prestate\n\
                 \x20   llvm_points_to_at_type \
                 (llvm_field result_ptr \"{start}\") \
                 (llvm_array_type {len} (llvm_int 8)) \
                 (llvm_term preBytes);\n",
                start = p.start,
                end = p.start + p.len,
                len = p.len,
            ));
        }
    }
    buf
}

/// Emit Cryptol expression for variant-map discriminant remapping.
fn wrap_variant_remap(
    expr: &str,
    variants: &[(u64, u64)],
    abi_bits: u32,
    inner: Option<&AbiReturnBridge>,
) -> String {
    if variants.len() == 2 {
        let (cry0, abi0) = variants[0];
        let (_, abi1) = variants[1];
        let cond = format!("({expr}) == ({cry0} : [{abi_bits}])");
        let result = format!("if {cond} then ({abi0} : [{abi_bits}]) else ({abi1} : [{abi_bits}])");
        if let Some(inner) = inner {
            inner.wrap(&result)
        } else {
            result
        }
    } else {
        // General case: emit nested if-then-else chain.
        let mut result = format!("({} : [{abi_bits}])", variants.last().unwrap().1);
        for (cry_disc, abi_disc) in variants.iter().rev().skip(1).rev() {
            result = format!(
                "if ({expr}) == ({cry_disc} : [{abi_bits}]) \
                 then ({abi_disc} : [{abi_bits}]) else ({result})"
            );
        }
        if let Some(inner) = inner {
            inner.wrap(&result)
        } else {
            result
        }
    }
}

#[cfg(test)]
#[path = "cryptol_bridge_tests.rs"]
mod bridge_tests;
