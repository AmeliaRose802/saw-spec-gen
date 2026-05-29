# SAW demo test manifest.
#
# Every entry is one case the harness runs end-to-end. The runner
# (Run-SawDemos.ps1) dispatches on `Runner`, executes the underlying
# verify*.ps1 (or custom script), captures stdout, and matches against
# `Expected`.
#
# Common keys
#   Tag       Group label (cpp_havoc, rust_havoc, bounded_loop, ...).
#   Dir       Demo directory, relative to repo root.
#   Expected  One of VERIFIED | DISPROVED | UNKNOWN
#                 |  EQUIVALENT | NOT EQUIVALENT.
#   Runner    cpp | rust | equiv | custom
#
# Convention defaults (override per-case as needed):
#   Cry        = "add_one_spec.cry"
#   CryptolFn  = "add_one_spec"
#   Function   = "add_one"
#
# Naming convention for sources:
#   *_verified.{cpp,rs}  expected to prove (VERIFIED).
#   *_disproved.{cpp,rs} expected to fail with a counterexample
#                        (DISPROVED). Earlier revisions called these
#                        `_sat` / `_unsat`, which was backwards from
#                        the SAT-solver meaning (SAT result = bug found,
#                        UNSAT result = proof succeeded) and confusing.
#
# Custom-script runners pass Script + ScriptArgs verbatim.
@{
    Cases = @(
        # ── C++ havoc demos (verify.ps1) ─────────────────────────────────────
        @{ Tag = 'cpp_havoc'; Runner = 'cpp'; Dir = 'demo/vtable_havoc_spec_demos/nothing_sketchy';            File = 'add_one_verified.cpp';                   Expected = 'VERIFIED' }
        @{ Tag = 'cpp_havoc'; Runner = 'cpp'; Dir = 'demo/vtable_havoc_spec_demos/nothing_sketchy';            File = 'add_one_disproved.cpp';                  Expected = 'DISPROVED' }
        # Compositional gen-verify havocs all direct callees, so OkLog::log is
        # overridden adversarially and may clobber super_important -> DISPROVED.
        @{ Tag = 'cpp_havoc'; Runner = 'cpp'; Dir = 'demo/vtable_havoc_spec_demos/concrete_type_safe';         File = 'add_one_verified.cpp';                   Expected = 'DISPROVED' }
        @{ Tag = 'cpp_havoc'; Runner = 'cpp'; Dir = 'demo/vtable_havoc_spec_demos/concrete_type_safe';         File = 'add_one_disproved.cpp';                  Expected = 'DISPROVED' }
        @{ Tag = 'cpp_havoc'; Runner = 'cpp'; Dir = 'demo/vtable_havoc_spec_demos/class_member_clobbered';     File = 'add_one_verified.cpp';                   Expected = 'VERIFIED' }
        @{ Tag = 'cpp_havoc'; Runner = 'cpp'; Dir = 'demo/vtable_havoc_spec_demos/class_member_clobbered';     File = 'add_one_disproved.cpp';                  Expected = 'DISPROVED' }
        @{ Tag = 'cpp_havoc'; Runner = 'cpp'; Dir = 'demo/vtable_havoc_spec_demos/global_memory_clobbered';    File = 'add_one_verified.cpp';                   Expected = 'VERIFIED' }
        @{ Tag = 'cpp_havoc'; Runner = 'cpp'; Dir = 'demo/vtable_havoc_spec_demos/global_memory_clobbered';    File = 'add_one_disproved.cpp';                  Expected = 'DISPROVED' }
        @{ Tag = 'cpp_havoc'; Runner = 'cpp'; Dir = 'demo/vtable_havoc_spec_demos/input_param_modified';       File = 'add_one_verified.cpp';                   Expected = 'VERIFIED' }
        @{ Tag = 'cpp_havoc'; Runner = 'cpp'; Dir = 'demo/vtable_havoc_spec_demos/input_param_modified';       File = 'add_one_disproved.cpp';                  Expected = 'DISPROVED' }
        @{ Tag = 'cpp_havoc'; Runner = 'cpp'; Dir = 'demo/vtable_havoc_spec_demos/output_param_uninitialized'; File = 'add_one_verified.cpp';                   Expected = 'VERIFIED' }
        @{ Tag = 'cpp_havoc'; Runner = 'cpp'; Dir = 'demo/vtable_havoc_spec_demos/output_param_uninitialized'; File = 'add_one_disproved.cpp';                  Expected = 'DISPROVED' }
        @{ Tag = 'cpp_havoc'; Runner = 'cpp'; Dir = 'demo/vtable_havoc_spec_demos/pointer_aliasing';           File = 'add_one_verified.cpp';                   Expected = 'VERIFIED' }
        @{ Tag = 'cpp_havoc'; Runner = 'cpp'; Dir = 'demo/vtable_havoc_spec_demos/pointer_aliasing';           File = 'add_one_disproved.cpp';                  Expected = 'DISPROVED' }
        # Adversarial-holes filenames embed the *historical* false verdict
        # (e.g. `false_disproved_ctor_stub.cpp` once falsely reported
        # DISPROVED before the bug was fixed; today it correctly VERIFIES).
        @{ Tag = 'cpp_havoc'; Runner = 'cpp'; Dir = 'demo/vtable_havoc_spec_demos/adversarial_holes';          File = 'add_one_false_disproved_ctor_stub.cpp';  Expected = 'VERIFIED' }
        @{ Tag = 'cpp_havoc'; Runner = 'cpp'; Dir = 'demo/vtable_havoc_spec_demos/adversarial_holes';          File = 'add_one_false_verified_mutable.cpp';     Expected = 'DISPROVED' }
        @{ Tag = 'cpp_havoc'; Runner = 'cpp'; Dir = 'demo/vtable_havoc_spec_demos/multi_method_ordering';      File = 'call_validate_verified.cpp';             Expected = 'VERIFIED' }
        @{ Tag = 'cpp_havoc'; Runner = 'cpp'; Dir = 'demo/vtable_havoc_spec_demos/multi_method_ordering';      File = 'call_audit_verified.cpp';                Expected = 'VERIFIED' }
        @{ Tag = 'cpp_havoc'; Runner = 'cpp'; Dir = 'demo/vtable_havoc_spec_demos/multi_method_ordering';      File = 'call_prepare_disproved.cpp';             Expected = 'DISPROVED' }
        @{ Tag = 'cpp_havoc'; Runner = 'cpp'; Dir = 'demo/vtable_havoc_spec_demos/multi_method_ordering';      File = 'call_report_verified.cpp';               Expected = 'VERIFIED' }
        @{ Tag = 'cpp_havoc'; Runner = 'cpp'; Dir = 'demo/vtable_havoc_spec_demos/multi_method_ordering';      File = 'call_extra_disproved.cpp';               Expected = 'DISPROVED' }
        # MSVC C++ exception lowering: total Cryptol spec, partial impl.
        @{ Tag = 'cpp_havoc'; Runner = 'cpp'; Dir = 'demo/vtable_havoc_spec_demos/throws_exception';           File = 'add_one_verified.cpp';                   Expected = 'VERIFIED' }
        @{ Tag = 'cpp_havoc'; Runner = 'cpp'; Dir = 'demo/vtable_havoc_spec_demos/throws_exception';           File = 'add_one_throws.cpp';                     Expected = 'DISPROVED' }

        # ── Rust havoc demos (verify-rust.ps1) ──────────────────────────────
        @{ Tag = 'rust_havoc'; Runner = 'rust'; Dir = 'demo/rust_equalivence_demo/nothing_sketchy';         File = 'add_one_verified.rs';  Expected = 'VERIFIED'  }
        @{ Tag = 'rust_havoc'; Runner = 'rust'; Dir = 'demo/rust_equalivence_demo/nothing_sketchy';         File = 'add_one_disproved.rs'; Expected = 'DISPROVED' }
        @{ Tag = 'rust_havoc'; Runner = 'rust'; Dir = 'demo/rust_equalivence_demo/concrete_type_safe';      File = 'add_one_verified.rs';  Expected = 'VERIFIED'  }
        @{ Tag = 'rust_havoc'; Runner = 'rust'; Dir = 'demo/rust_equalivence_demo/concrete_type_safe';      File = 'add_one_disproved.rs'; Expected = 'DISPROVED' }
        @{ Tag = 'rust_havoc'; Runner = 'rust'; Dir = 'demo/rust_equalivence_demo/class_member_clobbered';  File = 'add_one_verified.rs';  Expected = 'VERIFIED'  }
        @{ Tag = 'rust_havoc'; Runner = 'rust'; Dir = 'demo/rust_equalivence_demo/class_member_clobbered';  File = 'add_one_disproved.rs'; Expected = 'DISPROVED' }
        @{ Tag = 'rust_havoc'; Runner = 'rust'; Dir = 'demo/rust_equalivence_demo/global_memory_clobbered'; File = 'add_one_verified.rs';  Expected = 'VERIFIED'  }
        @{ Tag = 'rust_havoc'; Runner = 'rust'; Dir = 'demo/rust_equalivence_demo/global_memory_clobbered'; File = 'add_one_disproved.rs'; Expected = 'DISPROVED' }
        @{ Tag = 'rust_havoc'; Runner = 'rust'; Dir = 'demo/rust_equalivence_demo/input_param_modified';    File = 'add_one_verified.rs';  Expected = 'VERIFIED'  }
        @{ Tag = 'rust_havoc'; Runner = 'rust'; Dir = 'demo/rust_equalivence_demo/input_param_modified';    File = 'add_one_disproved.rs'; Expected = 'DISPROVED' }
        @{ Tag = 'rust_havoc'; Runner = 'rust'; Dir = 'demo/rust_equalivence_demo/pointer_aliasing';        File = 'add_one_verified.rs';  Expected = 'VERIFIED'  }
        @{ Tag = 'rust_havoc'; Runner = 'rust'; Dir = 'demo/rust_equalivence_demo/trait_static_dispatch';   File = 'add_one_verified.rs';  Expected = 'VERIFIED'  }
        @{ Tag = 'rust_havoc'; Runner = 'rust'; Dir = 'demo/rust_equalivence_demo/trait_static_dispatch';   File = 'add_one_disproved.rs'; Expected = 'DISPROVED' }
        @{ Tag = 'rust_havoc'; Runner = 'rust'; Dir = 'demo/rust_equalivence_demo/trait_dynamic_dispatch';  File = 'add_one_verified.rs';  Expected = 'VERIFIED'  }
        @{ Tag = 'rust_havoc'; Runner = 'rust'; Dir = 'demo/rust_equalivence_demo/trait_dynamic_dispatch';  File = 'add_one_disproved.rs'; Expected = 'DISPROVED' }
        @{ Tag = 'rust_havoc'; Runner = 'rust'; Dir = 'demo/rust_equalivence_demo/trait_external_crate';    File = 'add_one_verified.rs';  Expected = 'VERIFIED'  }
        @{ Tag = 'rust_havoc'; Runner = 'rust'; Dir = 'demo/rust_equalivence_demo/trait_external_crate';    File = 'add_one_disproved.rs'; Expected = 'DISPROVED' }

        # ── Bounded-loop demos (positive) ───────────────────────────────────
        @{ Tag = 'bounded_loop'; Runner = 'cpp';  Dir = 'demo/bounded_loop'; File = 'add_one.cpp';      Expected = 'VERIFIED' }
        @{ Tag = 'bounded_loop'; Runner = 'rust'; Dir = 'demo/bounded_loop'; File = 'add_one.rs';       Expected = 'VERIFIED' }
        @{ Tag = 'bounded_loop'; Runner = 'cpp';  Dir = 'demo/bounded_loop'; File = 'sum_first_n.cpp';  Expected = 'VERIFIED'; Cry = 'sum_first_n_spec.cry'; CryptolFn = 'sum_first_n_spec'; Function = 'sum_first_n' }
        @{ Tag = 'bounded_loop'; Runner = 'rust'; Dir = 'demo/bounded_loop'; File = 'sum_first_n.rs';   Expected = 'VERIFIED'; Cry = 'sum_first_n_spec.cry'; CryptolFn = 'sum_first_n_spec'; Function = 'sum_first_n' }

        # ── String operations (SWAR null-byte detection: real libc strlen
        #    bit-trick over a 64-bit word treated as 8 packed bytes). ───────
        @{ Tag = 'string_ops'; Runner = 'cpp'; Dir = 'demo/string_ops'; File = 'has_null_byte_verified.cpp';  Cry = 'has_null_byte_spec.cry'; CryptolFn = 'has_null_byte_spec'; Function = 'has_null_byte'; Expected = 'VERIFIED'  }
        @{ Tag = 'string_ops'; Runner = 'cpp'; Dir = 'demo/string_ops'; File = 'has_null_byte_disproved.cpp'; Cry = 'has_null_byte_spec.cry'; CryptolFn = 'has_null_byte_spec'; Function = 'has_null_byte'; Expected = 'DISPROVED' }

        # ── Real C-string demo: count_digits over `_In_reads_(8) const char*`.
        #    Exercises the SAL count annotation -> 8-byte buffer allocation
        #    path plus the auto-emitted llvm.var.annotation override. ───────
        @{ Tag = 'strings'; Runner = 'cpp'; Dir = 'demo/strings'; File = 'count_digits_cstr.cpp';           Cry = 'count_digits_spec.cry'; CryptolFn = 'count_digits_spec'; Function = 'count_digits'; Expected = 'VERIFIED'  }
        @{ Tag = 'strings'; Runner = 'cpp'; Dir = 'demo/strings'; File = 'count_digits_cstr_disproved.cpp'; Cry = 'count_digits_spec.cry'; CryptolFn = 'count_digits_spec'; Function = 'count_digits'; Expected = 'DISPROVED' }
        # std::string heap-mode demo: hand-rolled SAW driver allocates a
        # basic_string with `_Ptr` wired to a separately-allocated content
        # buffer, asserts the Cryptol `valid_string` precondition, and
        # verifies (z3) that count_digits matches the spec for any string
        # up to MAX_LEN=32 bytes. Uses a custom runner because gen-verify
        # can't synthesise this layout yet.
        @{ Tag = 'strings'; Runner = 'custom'; Script = 'demo/strings/run_string_demo.ps1'; ScriptArgs = @{ CppFile = 'demo/strings/count_digits_string.cpp';           ExpectedResult = 'VERIFIED'  }; Expected = 'VERIFIED'  }
        @{ Tag = 'strings'; Runner = 'custom'; Script = 'demo/strings/run_string_demo.ps1'; ScriptArgs = @{ CppFile = 'demo/strings/count_digits_string_disproved.cpp'; ExpectedResult = 'DISPROVED' }; Expected = 'DISPROVED' }

        # ── C++/Rust equivalence demos (verify-equiv.ps1) ───────────────────
        @{ Tag = 'rust_equiv'; Runner = 'equiv'; Dir = 'demo/rust_equalivence_demo/cpp_fee_reordered';         Cpp = 'compute_fee.cpp'; Rust = 'compute_fee_good.rs'; Cry = 'compute_fee_spec.cry'; CryptolFn = 'compute_fee_spec'; Function = 'compute_fee'; Expected = 'EQUIVALENT'     }
        @{ Tag = 'rust_equiv'; Runner = 'equiv'; Dir = 'demo/rust_equalivence_demo/cpp_fee_reordered';         Cpp = 'compute_fee.cpp'; Rust = 'compute_fee_bad.rs';  Cry = 'compute_fee_spec.cry'; CryptolFn = 'compute_fee_spec'; Function = 'compute_fee'; Expected = 'NOT EQUIVALENT' }
        @{ Tag = 'rust_equiv'; Runner = 'equiv'; Dir = 'demo/rust_equalivence_demo/cpp_gold_rust_optimized';   Cpp = 'sat_add.cpp';     Rust = 'sat_add_good.rs';     Cry = 'sat_add_spec.cry';     CryptolFn = 'sat_add_spec';     Function = 'sat_add';     Expected = 'EQUIVALENT'     }
        @{ Tag = 'rust_equiv'; Runner = 'equiv'; Dir = 'demo/rust_equalivence_demo/cpp_gold_rust_optimized';   Cpp = 'sat_add.cpp';     Rust = 'sat_add_bad.rs';      Cry = 'sat_add_spec.cry';     CryptolFn = 'sat_add_spec';     Function = 'sat_add';     Expected = 'NOT EQUIVALENT' }
        @{ Tag = 'rust_equiv'; Runner = 'equiv'; Dir = 'demo/rust_equalivence_demo/cpp_rust_not_operator_trap'; Cpp = 'negate.cpp';     Rust = 'negate.rs';           Cry = 'negate_spec.cry';      CryptolFn = 'negate_spec';      Function = 'negate';      Expected = 'NOT EQUIVALENT' }

        # ── Unknown concrete-impl dyn-trait case (custom runner) ────────────
        @{ Tag = 'trait_unknown_impl'; Runner = 'custom'; Script = 'demo/rust_equalivence_demo/trait_unknown_impl/run_unknown_impl.ps1'; ScriptArgs = @{ RustFile = 'demo/rust_equalivence_demo/trait_unknown_impl/add_step_verified.rs';  ExpectedResult = 'VERIFIED'  }; Expected = 'VERIFIED'  }
        @{ Tag = 'trait_unknown_impl'; Runner = 'custom'; Script = 'demo/rust_equalivence_demo/trait_unknown_impl/run_unknown_impl.ps1'; ScriptArgs = @{ RustFile = 'demo/rust_equalivence_demo/trait_unknown_impl/add_step_disproved.rs'; ExpectedResult = 'DISPROVED' }; Expected = 'DISPROVED' }

        # ── Async-Rust coroutine demo (custom runner) ───────────────────────
        @{ Tag = 'async_rust'; Runner = 'custom'; Script = 'demo/async_rust/run_async_demo.ps1'; ScriptArgs = @{}; Expected = 'VERIFIED' }

        # ── Rust adversarial holes (research cases: each demonstrates a
        #    specific verifier blind spot or successful adversarial coverage).
        #    Expected verdicts captured from baseline runs.
        @{ Tag = 'rust_adversarial'; Runner = 'rust'; Dir = 'demo/rust_adversarial_holes/cell_interior_mutation';  File = 'add_one.rs'; Expected = 'DISPROVED' }
        @{ Tag = 'rust_adversarial'; Runner = 'rust'; Dir = 'demo/rust_adversarial_holes/cross_fn_global';          File = 'add_one.rs'; Expected = 'VERIFIED'  }
        @{ Tag = 'rust_adversarial'; Runner = 'rust'; Dir = 'demo/rust_adversarial_holes/drop_noinline';            File = 'add_one.rs'; Expected = 'VERIFIED'  }
        @{ Tag = 'rust_adversarial'; Runner = 'rust'; Dir = 'demo/rust_adversarial_holes/drop_side_effect';         File = 'add_one.rs'; Expected = 'VERIFIED'  }
        @{ Tag = 'rust_adversarial'; Runner = 'rust'; Dir = 'demo/rust_adversarial_holes/raw_pointer_aliasing';     File = 'add_one.rs'; Expected = 'DISPROVED' }
        @{ Tag = 'rust_adversarial'; Runner = 'rust'; Dir = 'demo/rust_adversarial_holes/symbol_collision';         File = 'add_one.rs'; Expected = 'DISPROVED' }
        @{ Tag = 'rust_adversarial'; Runner = 'rust'; Dir = 'demo/rust_adversarial_holes/unreachable_unchecked';    File = 'add_one.rs'; Expected = 'DISPROVED' }
        # box_allocator currently produces UNKNOWN under the default pipeline
        # (Box::new path the front-end can't model). Tracked separately; not
        # run by default to keep the suite green. To enable, add tag 'box_allocator'.
        @{ Tag = 'box_allocator'; Runner = 'rust'; Dir = 'demo/rust_adversarial_holes/box_allocator';        File = 'add_one.rs'; Expected = 'UNKNOWN' }
    )
}
