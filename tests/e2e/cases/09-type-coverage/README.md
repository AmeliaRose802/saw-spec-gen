# 09-type-coverage

End-to-end tests that exercise the full verification pipeline on the
integer widths and sign-classes that the earlier suites did *not* hit.
Each subdirectory carries a `*_verified` / `*_disproved` pair in both
C++ and Rust, plus a shared Cryptol spec.

| Subdir              | Width / sign            | Notes                                                                |
|---------------------|-------------------------|----------------------------------------------------------------------|
| `clamp_neg_i8/`     | `int8_t`  / `i8`        | Signed 8-bit (no earlier test exercised an i8 *return* type).        |
| `swap_bytes_u16/`   | `uint16_t`/ `u16`       | Unsigned 16-bit — left/right shift composition.                       |
| `max_i16/`          | `int16_t` / `i16`       | Signed 16-bit + multi-arg signed compare.                            |
| `clamp_neg_i64/`    | `int64_t` / `i64`       | Signed 64-bit ("long" in the user-facing taxonomy).                  |
| `add_u64/`          | `uint64_t`/ `u64`       | Unsigned 64-bit ("unsigned long") wrapping add.                      |
| `bool_or_to_u32/`   | `bool`    / `bool`      | i1 input lowering; widened to i32 return to avoid bool-return ABI churn. |
| `void_noop/`        | `void` return           | Bare void return; no params, no side effects. Smoke-tests the fix in `emit_postcondition_and_close` (skip `llvm_return` for void targets). |
| `void_out_inc/`     | `void` return + `_Out_` | Void return *with* a `_Out_ uint32_t*` parameter. Verified variant proves "no UB"; disproved variant writes past the SAW allocation. |
| `void_ptr_param/`   | `const void*` params    | Two `const void*` parameters compared by raw pointer-bit equality. Pointee is never dereferenced, so the `// void` → `llvm_int 8` rewrite in `pointee_saw_type` is exercised without needing a specific pointee width. |

## Coverage matrix snapshot

After this directory lands, the e2e suite covers:

| Type     | Where verified                                                    |
|----------|-------------------------------------------------------------------|
| `bool`   | `09-type-coverage/bool_or_to_u32`                                  |
| `i8`     | `09-type-coverage/clamp_neg_i8`                                    |
| `u8`     | `06-int-ops/popcount_u8`                                           |
| `i16`    | `09-type-coverage/max_i16`                                         |
| `u16`    | `09-type-coverage/swap_bytes_u16`                                  |
| `i32`    | `06-int-ops/min3_i32`, `06-int-ops/is_power_of_two`                |
| `u32`    | `02-havoc-coverage/nothing_sketchy`, `06-int-ops/byte_swap_u32`    |
| `i64`    | `09-type-coverage/clamp_neg_i64`                                   |
| `u64`    | `09-type-coverage/add_u64`                                         |
| pointer  | `02-havoc-coverage/input_param_modified`                           |
| struct/object | `02-havoc-coverage/class_member_clobbered` (interface ptr)    |
| `void` return | `09-type-coverage/void_noop`, `09-type-coverage/void_out_inc` |
| `void*` param | `09-type-coverage/void_ptr_param`                            |

## Known gaps — *not* covered

| Type             | Status            | Reason                                                              |
|------------------|-------------------|---------------------------------------------------------------------|
| `float` / `f32`  | Not supported     | `constraints::TypeInfo` has no `Float` variant; `verify-rust.ps1` rejects any non-`iN` LLVM type. Would need a new `TypeInfo::Float`, a `type_to_saw` arm emitting `llvm_float`, and an `llvm_fresh_var` lowering for `[FloatN]`. |
| `double` / `f64` | Not supported     | Same as above (`llvm_double` SAW type exists but the front-end never emits it).  |
| `void` return — functional postcondition on `_Out_` params | Not auto-generated | `emit_postcondition_and_close` correctly skips `llvm_return` for void returns (so the spec parses + SAW proves no-UB), but it does **not** yet emit a closing `llvm_points_to <out_ptr> (llvm_term {{ spec ... }})` to encode the functional contract. The `void_out_inc/_verified` case therefore only proves memory-safety; a buggy implementation that writes the *wrong* value through `out` would still pass. The `_disproved` case demonstrates the orthogonal memory-safety check by writing past the allocation. Fix: extend the emitter to add a post-execute `llvm_points_to` for every `_Out_` / mutable-pointer parameter, threading the spec call's result components through. |
| `void` return on Rust path | Not exercised | `verify-rust.ps1` doesn't go through `saw-spec-gen gen-verify`; it hand-rolls the SAW script and `Test-IntegerSignature` rejects non-`iN` return types outright. A separate enhancement to the Rust harness is required before `fn foo() -> ()` can be e2e-tested. |
| `void*` / `*const c_void` on Rust path | Not exercised | Same root cause as the Rust void-return gap. |

These intentionally have no test cases yet — adding one would either
fail with `Unsupported LLVM argument type 'float'` (Rust path) or
silently fall through to `llvm_alias "float"` (C++ path) and crash at
SAW load. When the front-end gains a `Float` arm, drop a
`float_add/` and `double_add/` directory here and a row in the matrix
above.
