# saw-spec-gen

Auto-generate [SAW](https://saw.galois.com/) verification specs from C++ and Rust type information.

Reads compiler-provided type info and generates the **tightest correct SAW override specs** derivable from the type system, annotations, and compiler attributes. The generated specs model external functions as **unspecified** — any behavior consistent with the type system — so the SAW verifier proves callers are correct for ALL possible implementations.

## Why?

When verifying code with SAW, external functions (crypto libraries, OS calls, allocators) need override specs. Writing these manually is error-prone:

- **Too loose**: `llvm_fresh_var` for everything — SAW explores impossible states
- **Too tight**: Hardcoded return values — SAW misses real bugs
- **Just right**: Constraints from the type system — the tightest spec that's guaranteed correct

This tool automatically extracts those constraints.

## Sources of Constraints

| Source | What it tells us | Example |
|--------|-----------------|--------|
| C++ `const&` / Rust `&T` | Parameter is readonly | `llvm_alloc_readonly` |
| LLVM `nounwind` / C++ `noexcept` | Function cannot throw | No exception path |
| LLVM `nonnull` / Rust references | Pointer is never null | Non-null precondition |
| LLVM `dereferenceable(N)` | N bytes are readable | Allocation size |
| Rust `Option<T>` | Return is `None` or `Some(T)` | Discriminant in {0, 1} |
| Rust `Result<T, E>` | Return is `Ok(T)` or `Err(E)` | Discriminant in {0, 1} |
| Rust enum | Finite set of variants | Discriminant range |
| C++ SAL `_In_` | Input-only, not modified | Readonly postcondition |
| C++ SAL `_Out_` | Must be written before return | Write-only |
| Unsigned types | Value >= 0 | No negative values |

## Usage

### From C++ (clang AST)

```bash
# Step 1: Generate AST dump
clang -Xclang -ast-dump=json -fsyntax-only MyFile.cpp > ast.json

# Step 2: Generate SAW specs
saw-spec-gen from-clang-ast --input ast.json --output generated_specs/

# Step 3: Use in SAW
# include "generated_specs/auto_specs.saw";
```

### From Rust (mir-json)

```bash
# Step 1: Build with saw-rustc
cargo saw-build

# Step 2: Generate SAW specs  
saw-spec-gen from-mir-json --input target/my_crate.linked-mir.json --output generated_specs/
```

### Filtering

```bash
saw-spec-gen from-clang-ast --input ast.json --output specs/ --filter "IsValid"
```

## Building

```bash
cargo build --release
```

## Architecture

```
Input (JSON)        ->  Constraint Derivation  ->  SAW Emission
                    
clang_ast.rs           constraints.rs            saw_emit.rs
  parse AST              FunctionInfo              .saw files
  extract params         ParamInfo                 llvm_alloc_readonly
  detect const           TypeInfo                  llvm_fresh_var
  parse SAL              Mutability                llvm_precond
                         derive_constraints()      llvm_postcond
mir_json.rs                                     
  parse MIR JSON   
  extract types    
  detect &/&mut    
```

The key design: `constraints.rs` is **language-independent**. Both `clang_ast.rs` and `mir_json.rs` produce the same `FunctionInfo` structs. The constraint derivation and SAW emission don't know which language the input came from.

## License

MIT
