# saw-spec-gen

Auto-generate [SAW](https://saw.galois.com/) verification specs from C++ and Rust type information.

Reads compiler-provided type info and generates SAW override specs that model external functions as **adversarial**: they can do anything the type system allows. Mutable memory reachable from parameters is havoced (solver picks any value). Const memory is preserved. The SAW verifier then proves callers are correct for **all possible implementations** of the overridden functions.

## The Problem

When verifying C++ or Rust code with SAW, you need override specs for every function you don't want to (or can't) verify directly: interface methods, OS calls, crypto libraries, allocators, callbacks.

Writing these by hand is error-prone:

- **Too loose**: fresh unconstrained return, but forget to havoc mutable memory — SAW misses bugs where the callee corrupts state
- **Too tight**: hardcoded return values — SAW misses bugs that only appear with certain return values
- **Wrong shape**: forget to account for `const`, miscount struct fields, wrong LLVM type — spec silently doesn't match

This tool extracts constraints from the type system, SAL annotations, and compiler attributes, and generates the **maximally adversarial** spec consistent with those constraints.

Note: STD functions are assumed not to be evil and safe assume specs are auto generated for them. Maybe evil std mode is a future feature.

## What It Generates

### Regular specs (`from-clang-ast` / `from-mir-json`)

For each function, a `.saw` file with:
- `llvm_alloc_readonly` for const pointers, `llvm_alloc` for mutable
- `llvm_fresh_var` for scalar parameters
- Return type with appropriate SAW type
- Postconditions: readonly params unchanged, mangled name in the usage comment

### Adversarial interface stubs (`--emit-stubs`)

For virtual methods in C++ classes:

| File | Purpose |
|------|---------|
| `vtable_stubs.ll` | LLVM IR with stub functions + vtable globals. SAW resolves `this->vptr->vtable[slot]->stub` |
| `*_havoc_spec.saw` | Per-method adversarial spec: mutable memory havoced, const memory preserved |
| `interface_overrides.saw` | `llvm_unsafe_assume_spec` calls + `alloc_this` helper for vtable wiring |

The adversarial model:

| Constraint source | Preserved or havoced? |
|---|---|
| `const T*` / `const T&` | **Preserved** — `llvm_alloc_readonly`, postcondition asserts unchanged |
| `_In_` / `_In_reads_(n)` | **Preserved** — SAL says input-only |
| Non-const `T*` / `T&` | **Havoced** — every reachable byte gets a fresh symbolic after the call |
| `_Out_` / `_Out_writes_(n)` | **Havoced** — SAL says output |
| `_Inout_` | **Havoced** — SAL says read-write |
| Non-const `this` | **Havoced** — method can modify any field |
| `const` method `this` | **Preserved** |
| Return value | **Unconstrained** — solver picks any value of the right type |
| **Conflict** (e.g. `const` + `_Inout_`) | **Strictest wins** — preserved |

Struct fields with known types are decomposed: each scalar field gets its own fresh var, each pointer field gets a separate allocation that is individually havoced or preserved.

## Usage

### Basic: generate specs from C++ AST

```bash
# Dump the AST
clang -Xclang -ast-dump=json -fsyntax-only MyFile.cpp > ast.json

# Generate SAW specs
saw-spec-gen from-clang-ast --input ast.json --output specs/

# Also generate Cryptol type predicates
saw-spec-gen from-clang-ast --input ast.json --output specs/ --cryptol
```

### Interface stubs: adversarial overrides for virtual methods

```bash
# Generate everything: specs + vtable stubs + havoc overrides
saw-spec-gen from-clang-ast --input ast.json --output specs/ --emit-stubs
```

This produces:
```
specs/
├── auto_specs.saw              # Index: includes all regular specs
├── validate_auto_spec.saw      # Regular spec for each function
├── vtable_stubs.ll             # LLVM IR: stub functions + vtable globals
├── IValidator_validate_havoc_spec.saw   # Adversarial havoc spec
├── IValidator_error_code_havoc_spec.saw
└── interface_overrides.saw     # Override setup + alloc_this helper
```

In your SAW script:
```saw
enable_experimental;
m_main  <- llvm_load_module "your_code.bc";
m_stubs <- llvm_load_module "specs/vtable_stubs.ll";
m       <- llvm_combine_modules m_main [m_stubs];
include "specs/interface_overrides.saw";

// Now verify — overrides model ANY implementation of the interface
llvm_verify m "?your_function@@..." [ov_ivalidator_validate_stub] false your_spec z3;
```

### From Rust (mir-json)

```bash
# Build with saw-rustc
cargo saw-build

# Generate SAW specs (LLVM mode)
saw-spec-gen from-mir-json --input target/crate.linked-mir.json --output specs/

# Or MIR verify mode
saw-spec-gen from-mir-json --input crate.mir.json --output specs/ --mir-verify
```

### From LLVM IR (any language)

```bash
# Disassemble bitcode
llvm-dis module.bc -o module.ll

# Generate specs — resolves struct types, detects sret
saw-spec-gen from-llvm-ir --input module.ll --output specs/
```

### Filtering

```bash
saw-spec-gen from-clang-ast --input ast.json --output specs/ --filter "Validate"
```

## Constraint Sources

| Source | What it tells us | How it's used |
|--------|-----------------|---------------|
| C++ `const T*` / Rust `&T` | Parameter is read-only | `llvm_alloc_readonly` + preservation postcondition |
| C++ `T*` / Rust `&mut T` | Parameter is mutable | `llvm_alloc` + havoced postcondition |
| SAL `_In_` / `_In_reads_(n)` | Input-only | Preserved; buffer sized to `n` |
| SAL `_Out_` / `_Out_writes_(n)` | Output-only | Havoced; buffer sized to `n` |
| SAL `_Inout_` | Read-write | Havoced (unless const overrides) |
| `noexcept` / `nounwind` | Cannot throw | No exception path warning |
| `nonnull` / Rust `&T` | Pointer never null | Non-null precondition |
| Rust `enum` | Finite variants | Discriminant range constraint |
| Rust `Option<T>` / `Result<T,E>` | Two-variant enum | Discriminant ∈ {0, 1} |
| LLVM `sret` | Return via pointer | Modeled as output parameter |
| Struct fields | Known layout | Per-field decomposition in havoc specs |
| Struct size | Byte-level layout | `llvm_array N (llvm_int 8)` |
| STL types | Known MSVC x64 sizes | `std::string`=32, `std::vector`=24, etc. |

## How the Adversarial Model Works

When `--emit-stubs` encounters a virtual method like:
```cpp
class IValidator {
    virtual bool validate(const SessionToken &token) const noexcept = 0;
};
```

It generates a spec where:
1. `this` is `llvm_alloc_readonly` (const method → preserved)
2. `token` is `llvm_alloc_readonly` (const ref → preserved)
3. Return is `llvm_fresh_var "ret" (llvm_int 1)` (unconstrained bool)
4. Postconditions assert `this` and `token` are unchanged

For a non-const method like `int process(Config *cfg)`:
1. `this` is `llvm_alloc` (mutable → havoced)
2. `cfg` fields are decomposed: `flags` gets `cfg_flags_after`, pointer fields get new allocations
3. Every mutable field gets an independent fresh symbolic in the postcondition
4. The solver can set any of them to any value

This means: if a caller reads `cfg->counter` after calling `process()`, and the proof depends on `counter` being unchanged, **the proof fails** — because the adversarial override models `process()` as potentially trashing it.

## Architecture

```
Input                   Constraint Engine         Output
─────                   ─────────────────         ──────

clang_ast.rs            constraints.rs            saw_emit.rs
  Parse AST               FunctionInfo              .saw specs
  Extract virtuals         ParamInfo                 vtable_stubs.ll
  Resolve struct IDs       TypeInfo                  havoc specs
  Parse SAL annotations    Mutability                interface_overrides.saw
                           derive_constraints()
mir_json.rs                                       cryptol_emit.rs
  Parse MIR JSON                                    .cry predicates
  Resolve ADTs

llvm_ir.rs
  Parse .ll text
  Resolve struct types
  Detect sret
```

The constraint engine (`constraints.rs`) is language-independent. All three parsers produce the same `FunctionInfo` structs. The SAW emitter doesn't know which language the input came from.

## Building

```bash
cargo build --release
```

## Examples

The `demo/` directory contains working examples:

- `checksum.cpp` / `verify.saw` — Basic verification of C++ functions with z3
- `interface_example.cpp` / `prove_cpp.saw` — Virtual dispatch through IValidator, proved equivalent to Cryptol spec
- `adversarial.h` / `prove_adversarial.saw` — Adversarial havoc model: safe vs unsafe patterns after unknown function calls

## License

MIT
