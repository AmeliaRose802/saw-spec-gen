//! Generate SAW vtable stubs and havoc specs for Rust trait methods.
//!
//! Mirrors the C++ side's `vtable_stubs.bc` + `interface_overrides.saw`
//! flow. Used to verify Rust functions that take `&dyn Trait` from an
//! opaque caller — i.e. when the concrete impl is not known to SAW.
//!
//! Inputs:  a small typed schema describing the traits and methods we
//!          care about (see [`TraitSchema`]).
//! Outputs: `trait_stubs.ll`     — synthetic vtable + stub functions
//!          `interface_overrides.saw` — assumed havoc specs for each
//!                                     trait method, plus a list named
//!                                     `trait_overrides` for splicing
//!                                     into `llvm_verify`.
//!
//! Design:
//!   * The schema is parsed via `serde` derives (typed, not Value
//!     spelunking).
//!   * Code generation goes through a [`TraitVisitor`] trait — two
//!     emitters (LLVM IR and SAWScript) walk the same schema and each
//!     accumulate into their own output buffer.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::fmt::Write as _;
use std::path::Path;

// ─── Schema ──────────────────────────────────────────────────────────

/// Top-level schema, parsed from JSON.
#[derive(Debug, Deserialize)]
pub struct TraitSchema {
    pub traits: Vec<Trait>,
}

#[derive(Debug, Deserialize)]
pub struct Trait {
    pub name: String,
    pub methods: Vec<Method>,
}

#[derive(Debug, Deserialize)]
pub struct Method {
    pub name: String,
    pub args: Vec<RustType>,
    pub ret: RustType,
}

/// Subset of Rust types we know how to model as a SAW havoc spec.
///
/// `Ref` and `Ptr` are both represented as raw `ptr` in LLVM; the
/// distinction only matters when the spec wants to assert points-to
/// information, which the havoc form deliberately avoids (we *don't*
/// know what an opaque trait impl does).
#[derive(Debug, Deserialize)]
#[serde(tag = "kind")]
pub enum RustType {
    Bool,
    U8,
    U16,
    U32,
    U64,
    I8,
    I16,
    I32,
    I64,
    Usize,
    Isize,
    /// `&T` / `&mut T` / `*const T` / `*mut T` — all become `ptr` in LLVM.
    Ref,
    Ptr,
    /// `()` — `void` return.
    Unit,
}

impl RustType {
    /// LLVM IR textual type (`i32`, `ptr`, …). `Unit` is rendered as
    /// `void`; nullary return arguments are filtered separately.
    fn llvm_ty(&self) -> &'static str {
        match self {
            RustType::Bool => "i1",
            RustType::U8 | RustType::I8 => "i8",
            RustType::U16 | RustType::I16 => "i16",
            RustType::U32 | RustType::I32 => "i32",
            RustType::U64 | RustType::I64 | RustType::Usize | RustType::Isize => "i64",
            RustType::Ref | RustType::Ptr => "ptr",
            RustType::Unit => "void",
        }
    }

    /// Bit width of an integer return — used for SAW `llvm_int N` and
    /// for the Cryptol type `[N]`.  Returns `None` for non-integer
    /// returns (pointers, unit, bool).
    fn int_bits(&self) -> Option<u32> {
        match self {
            RustType::U8 | RustType::I8 => Some(8),
            RustType::U16 | RustType::I16 => Some(16),
            RustType::U32 | RustType::I32 => Some(32),
            RustType::U64 | RustType::I64 | RustType::Usize | RustType::Isize => Some(64),
            _ => None,
        }
    }
}

// ─── Visitor pattern ─────────────────────────────────────────────────

/// Generic visitor over a [`TraitSchema`]. Each emitter implements
/// just the methods it cares about.
pub trait TraitVisitor {
    fn visit_trait_start(&mut self, _t: &Trait) {}
    fn visit_trait_end(&mut self, _t: &Trait) {}
    fn visit_method(&mut self, _t: &Trait, _m: &Method, _slot: usize) {}
    fn visit_schema_start(&mut self, _s: &TraitSchema) {}
    fn visit_schema_end(&mut self, _s: &TraitSchema) {}
}

/// Drive a visitor over the whole schema.
pub fn walk(schema: &TraitSchema, v: &mut dyn TraitVisitor) {
    v.visit_schema_start(schema);
    for t in &schema.traits {
        v.visit_trait_start(t);
        // First non-method slots in the Rust trait-object vtable:
        //   [0] drop_in_place, [1] size, [2] align, [3..] methods
        for (i, m) in t.methods.iter().enumerate() {
            v.visit_method(t, m, i);
        }
        v.visit_trait_end(t);
    }
    v.visit_schema_end(schema);
}

// ─── Emitter: trait_stubs.ll ─────────────────────────────────────────

/// Emits a synthetic LLVM module containing one stub function per
/// trait method plus a `@__stubvtable_<Trait>` constant in Rust
/// trait-object layout.
pub struct LlvmStubEmitter {
    out: String,
}

impl Default for LlvmStubEmitter {
    fn default() -> Self {
        Self::new()
    }
}

impl LlvmStubEmitter {
    pub fn new() -> Self {
        let mut s = LlvmStubEmitter { out: String::new() };
        s.preamble();
        s
    }

    pub fn finish(self) -> String {
        self.out
    }

    fn preamble(&mut self) {
        let _ = writeln!(
            self.out,
            "; Auto-generated by saw-spec-gen: rust trait vtable stubs.\n\
             ; Layout matches what rustc emits for `&dyn Trait`:\n\
             ;   offset  0: drop_in_place (ptr)\n\
             ;   offset  8: size           (i64)\n\
             ;   offset 16: align          (i64)\n\
             ;   offset 24: first method   (ptr)\n\n\
             target datalayout = \"e-m:w-p270:32:32-p271:32:32-p272:64:64-i64:64-f80:128-n8:16:32:64-S128\"\n\
             target triple = \"x86_64-pc-windows-msvc\"\n",
        );
    }

    fn stub_fn_name(t: &Trait, m: &Method) -> String {
        format!("__stub_{}_{}", t.name, m.name)
    }

    fn vtable_name(t: &Trait) -> String {
        format!("__stubvtable_{}", t.name)
    }
}

impl TraitVisitor for LlvmStubEmitter {
    fn visit_method(&mut self, t: &Trait, m: &Method, _slot: usize) {
        // Stub function: trivially returns 0 / null. SAW will replace
        // the body via llvm_unsafe_assume_spec.
        let ret = m.ret.llvm_ty();
        let args: Vec<String> = m
            .args
            .iter()
            .enumerate()
            .map(|(i, a)| format!("{} %a{}", a.llvm_ty(), i))
            .collect();
        let name = Self::stub_fn_name(t, m);
        let body = match m.ret {
            RustType::Unit => "  ret void".to_string(),
            RustType::Ref | RustType::Ptr => "  ret ptr null".to_string(),
            RustType::Bool => "  ret i1 0".to_string(),
            _ => format!("  ret {} 0", ret),
        };
        let _ = writeln!(
            self.out,
            "define {} @{}({}) {{\n{}\n}}\n",
            ret,
            name,
            args.join(", "),
            body,
        );
    }

    fn visit_trait_end(&mut self, t: &Trait) {
        // Synthesise a vtable: { dtor, size, align, method0, method1, ... }.
        // We use { ptr, i64, i64, ptr, ptr, ... } as the struct type.
        let mut tys = vec!["ptr".to_string(), "i64".to_string(), "i64".to_string()];
        let mut vals = vec![
            "ptr null".to_string(),
            "i64 0".to_string(),
            "i64 1".to_string(),
        ];
        for m in &t.methods {
            tys.push("ptr".to_string());
            vals.push(format!("ptr @{}", Self::stub_fn_name(t, m)));
        }
        let _ = writeln!(
            self.out,
            "@{} = constant {{ {} }} {{\n  {}\n}}\n",
            Self::vtable_name(t),
            tys.join(", "),
            vals.join(",\n  "),
        );
    }
}

// ─── Emitter: interface_overrides.saw ────────────────────────────────

/// Emits SAWScript that introduces a fresh symbolic value for each
/// trait method's return, plus a havoc spec per method and a single
/// `trait_overrides` list ready to splice into `llvm_verify`.
pub struct SawSpecEmitter {
    out: String,
    override_idents: Vec<String>,
}

impl Default for SawSpecEmitter {
    fn default() -> Self {
        Self::new()
    }
}

impl SawSpecEmitter {
    pub fn new() -> Self {
        SawSpecEmitter {
            out: String::new(),
            override_idents: Vec::new(),
        }
    }

    pub fn finish(mut self) -> String {
        // Trailing list literal so the verifier can `concat trait_overrides
        // its_own_overrides` and pass them all to llvm_verify.
        let _ = writeln!(
            self.out,
            "\nlet trait_overrides = [{}];",
            self.override_idents.join(", "),
        );
        self.out
    }
}

impl TraitVisitor for SawSpecEmitter {
    fn visit_schema_start(&mut self, _s: &TraitSchema) {
        let _ = writeln!(
            self.out,
            "// Auto-generated by saw-spec-gen. INCLUDE AFTER `m <- llvm_load_module ...`.\n\
             //\n\
             // For each trait method we expose:\n\
             //   <Trait>_<method>_ret : a `fresh_symbolic` SAW Term reusable in\n\
             //                          your outer verify-spec's postcondition.\n\
             //   <Trait>_<method>_havoc_spec : a SAW LLVMSetup spec asserting the\n\
             //                                 stub returns that symbol.\n\
             // After all bindings we expose `trait_overrides : [LLVMOverride]`."
        );
    }

    fn visit_method(&mut self, t: &Trait, m: &Method, _slot: usize) {
        let ret_ident = format!("{}_{}_ret", t.name, m.name);
        let spec_ident = format!("{}_{}_havoc_spec", t.name, m.name);
        let ov_ident = format!("ov_{}_{}", t.name, m.name);
        let stub_sym = format!("__stub_{}_{}", t.name, m.name);

        // Fresh symbolic return — shared across all callers of this
        // method in the proof. (Pointer / unit returns get a fresh
        // pointer alloc inside the spec instead of fresh_symbolic.)
        match m.ret.int_bits() {
            Some(bits) => {
                let _ = writeln!(
                    self.out,
                    "\n{ret_ident} <- fresh_symbolic \"{ret_ident}\" {{| [{bits}] |}};",
                );
            }
            None => {
                // Non-integer returns: no shared symbolic, just leave
                // the slot for the spec to handle.
            }
        }

        // The setup spec.
        let _ = writeln!(self.out, "let {spec_ident} = do {{");
        for (i, a) in m.args.iter().enumerate() {
            match a {
                RustType::Ref | RustType::Ptr => {
                    let _ = writeln!(self.out, "    a{i} <- llvm_alloc (llvm_int 8);  // ptr",);
                }
                RustType::Unit => { /* unrepresentable as arg */ }
                _ => {
                    let bits = a.int_bits().unwrap_or(32);
                    let _ = writeln!(
                        self.out,
                        "    a{i} <- llvm_fresh_var \"a{i}\" (llvm_int {bits});",
                    );
                }
            }
        }
        let exec_args: Vec<String> = m
            .args
            .iter()
            .enumerate()
            .filter_map(|(i, a)| match a {
                RustType::Unit => None,
                RustType::Ref | RustType::Ptr => Some(format!("a{i}")),
                _ => Some(format!("llvm_term a{i}")),
            })
            .collect();
        let _ = writeln!(
            self.out,
            "    llvm_execute_func [{}];",
            exec_args.join(", "),
        );

        match m.ret {
            RustType::Unit => { /* no return */ }
            RustType::Ref | RustType::Ptr => {
                let _ = writeln!(
                    self.out,
                    "    ret_ptr <- llvm_alloc (llvm_int 8);\n    llvm_return ret_ptr;",
                );
            }
            _ => {
                let _ = writeln!(self.out, "    llvm_return (llvm_term {ret_ident});");
            }
        }
        let _ = writeln!(self.out, "}};");

        // Register the override.
        let _ = writeln!(
            self.out,
            "{ov_ident} <- llvm_unsafe_assume_spec m \"{stub_sym}\" {spec_ident};",
        );
        self.override_idents.push(ov_ident);
    }
}

// ─── Public entry point ──────────────────────────────────────────────

/// Parse a `TraitSchema` JSON file and emit `trait_stubs.ll` and
/// `interface_overrides.saw` into `out_dir`.
pub fn emit_trait_stubs(schema_path: &Path, out_dir: &Path) -> Result<()> {
    let raw = std::fs::read_to_string(schema_path)
        .with_context(|| format!("failed to read {}", schema_path.display()))?;
    let schema: TraitSchema = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse {} as TraitSchema", schema_path.display()))?;

    std::fs::create_dir_all(out_dir)
        .with_context(|| format!("failed to mkdir {}", out_dir.display()))?;

    let mut llvm = LlvmStubEmitter::new();
    walk(&schema, &mut llvm);
    let llvm_path = out_dir.join("trait_stubs.ll");
    std::fs::write(&llvm_path, llvm.finish())
        .with_context(|| format!("failed to write {}", llvm_path.display()))?;

    let mut saw = SawSpecEmitter::new();
    walk(&schema, &mut saw);
    let saw_path = out_dir.join("interface_overrides.saw");
    std::fs::write(&saw_path, saw.finish())
        .with_context(|| format!("failed to write {}", saw_path.display()))?;

    Ok(())
}

// ─── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn stepper_schema() -> TraitSchema {
        serde_json::from_str(
            r#"{
                "traits": [
                    {
                        "name": "Stepper",
                        "methods": [
                            {
                                "name": "step",
                                "args": [{"kind": "Ref"}],
                                "ret":  {"kind": "U32"}
                            }
                        ]
                    }
                ]
            }"#,
        )
        .unwrap()
    }

    #[test]
    fn schema_parses_with_serde_derive() {
        let s = stepper_schema();
        assert_eq!(s.traits.len(), 1);
        assert_eq!(s.traits[0].name, "Stepper");
        assert_eq!(s.traits[0].methods[0].args.len(), 1);
        assert!(matches!(s.traits[0].methods[0].ret, RustType::U32));
    }

    #[test]
    fn llvm_emitter_produces_stub_and_vtable() {
        let s = stepper_schema();
        let mut e = LlvmStubEmitter::new();
        walk(&s, &mut e);
        let out = e.finish();
        assert!(out.contains("define i32 @__stub_Stepper_step(ptr %a0)"));
        assert!(out.contains("@__stubvtable_Stepper = constant"));
        // Vtable layout: 3 metadata slots + 1 method slot.
        assert!(out.contains("ptr, i64, i64, ptr"));
    }

    #[test]
    fn saw_emitter_produces_shared_fresh_symbolic() {
        let s = stepper_schema();
        let mut e = SawSpecEmitter::new();
        walk(&s, &mut e);
        let out = e.finish();
        assert!(out.contains("Stepper_step_ret <- fresh_symbolic"));
        assert!(out.contains("let Stepper_step_havoc_spec = do {"));
        assert!(out.contains("llvm_return (llvm_term Stepper_step_ret)"));
        assert!(out.contains("ov_Stepper_step <- llvm_unsafe_assume_spec"));
        assert!(out.contains("let trait_overrides = [ov_Stepper_step]"));
    }

    #[test]
    fn rust_type_int_bits_match_signed_unsigned() {
        assert_eq!(RustType::U32.int_bits(), Some(32));
        assert_eq!(RustType::I64.int_bits(), Some(64));
        assert_eq!(RustType::Bool.int_bits(), None);
        assert_eq!(RustType::Ref.int_bits(), None);
    }
}
