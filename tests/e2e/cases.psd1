# End-to-end test manifest.
#
# Every entry is one case the harness runs end-to-end. The runner
# (Run-E2ETests.ps1) dispatches on `Runner`, executes the underlying
# verify*.ps1 (or custom script), captures stdout, and matches against
# `Expected`.
#
# Common keys
#   Tag       Group label (cpp_havoc, rust_havoc, bounded_loop, ...).
#   Dir       Test-case directory, relative to repo root.
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
        # ── C++ havoc tests (verify.ps1) ─────────────────────────────────────
        @{ Tag = 'cpp_havoc'; Runner = 'cpp'; Dir = 'tests/e2e/cases/02-havoc-coverage/nothing_sketchy';            File = 'add_one_verified.cpp';                   Expected = 'VERIFIED' }
        @{ Tag = 'cpp_havoc'; Runner = 'cpp'; Dir = 'tests/e2e/cases/02-havoc-coverage/nothing_sketchy';            File = 'add_one_disproved.cpp';                  Expected = 'DISPROVED' }
        # OkLog::log has a concrete body with no opaque extern calls —
        # BFS traces all stores, none touch super_important → VERIFIED.
        @{ Tag = 'cpp_havoc'; Runner = 'cpp'; Dir = 'tests/e2e/cases/02-havoc-coverage/concrete_type_safe';         File = 'add_one_verified.cpp';                   Expected = 'VERIFIED' }
        @{ Tag = 'cpp_havoc'; Runner = 'cpp'; Dir = 'tests/e2e/cases/02-havoc-coverage/concrete_type_safe';         File = 'add_one_disproved.cpp';                  Expected = 'DISPROVED' }
        # Non-virtual target in a TU that *incidentally* declares a
        # polymorphic hierarchy (issue #57). add_one never dispatches
        # through the vtable, so the reachability gate skips interface
        # emission and the script uses the single-module load path.
        @{ Tag = 'cpp_havoc'; Runner = 'cpp'; Dir = 'tests/e2e/cases/02-havoc-coverage/incidental_polymorphic_type'; File = 'add_one_verified.cpp';                   Expected = 'VERIFIED' }
        @{ Tag = 'cpp_havoc'; Runner = 'cpp'; Dir = 'tests/e2e/cases/02-havoc-coverage/class_member_clobbered';     File = 'add_one_verified.cpp';                   Expected = 'VERIFIED' }
        @{ Tag = 'cpp_havoc'; Runner = 'cpp'; Dir = 'tests/e2e/cases/02-havoc-coverage/class_member_clobbered';     File = 'add_one_disproved.cpp';                  Expected = 'DISPROVED' }
        @{ Tag = 'cpp_havoc'; Runner = 'cpp'; Dir = 'tests/e2e/cases/02-havoc-coverage/global_memory_clobbered';    File = 'add_one_verified.cpp';                   Expected = 'VERIFIED' }
        @{ Tag = 'cpp_havoc'; Runner = 'cpp'; Dir = 'tests/e2e/cases/02-havoc-coverage/global_memory_clobbered';    File = 'add_one_disproved.cpp';                  Expected = 'DISPROVED' }
        @{ Tag = 'cpp_havoc'; Runner = 'cpp'; Dir = 'tests/e2e/cases/02-havoc-coverage/input_param_modified';       File = 'add_one_verified.cpp';                   Expected = 'VERIFIED' }
        @{ Tag = 'cpp_havoc'; Runner = 'cpp'; Dir = 'tests/e2e/cases/02-havoc-coverage/input_param_modified';       File = 'add_one_disproved.cpp';                  Expected = 'DISPROVED' }
        @{ Tag = 'cpp_havoc'; Runner = 'cpp'; Dir = 'tests/e2e/cases/02-havoc-coverage/output_param_uninitialized'; File = 'add_one_verified.cpp';                   Expected = 'VERIFIED' }
        @{ Tag = 'cpp_havoc'; Runner = 'cpp'; Dir = 'tests/e2e/cases/02-havoc-coverage/output_param_uninitialized'; File = 'add_one_disproved.cpp';                  Expected = 'DISPROVED' }
        @{ Tag = 'cpp_havoc'; Runner = 'cpp'; Dir = 'tests/e2e/cases/02-havoc-coverage/pointer_aliasing';           File = 'add_one_verified.cpp';                   Expected = 'VERIFIED' }
        @{ Tag = 'cpp_havoc'; Runner = 'cpp'; Dir = 'tests/e2e/cases/02-havoc-coverage/pointer_aliasing';           File = 'add_one_disproved.cpp';                  Expected = 'DISPROVED' }
        # `ctor_stub_false_verdicts` exhibits two patterns that historically
        # produced *false* verdicts (one falsely DISPROVED, one falsely
        # VERIFIED) before SAW fixes landed. Filenames now follow the
        # convention; the README captures the historical bug context.
        @{ Tag = 'cpp_havoc'; Runner = 'cpp'; Dir = 'tests/e2e/cases/02-havoc-coverage/ctor_stub_false_verdicts';          File = 'add_one_verified.cpp';                   Expected = 'VERIFIED' }
        @{ Tag = 'cpp_havoc'; Runner = 'cpp'; Dir = 'tests/e2e/cases/02-havoc-coverage/ctor_stub_false_verdicts';          File = 'add_one_disproved.cpp';                  Expected = 'DISPROVED' }
        @{ Tag = 'cpp_havoc'; Runner = 'cpp'; Dir = 'tests/e2e/cases/02-havoc-coverage/multi_method_ordering';      File = 'call_validate_verified.cpp';             Expected = 'VERIFIED' }
        @{ Tag = 'cpp_havoc'; Runner = 'cpp'; Dir = 'tests/e2e/cases/02-havoc-coverage/multi_method_ordering';      File = 'call_audit_verified.cpp';                Expected = 'VERIFIED' }
        @{ Tag = 'cpp_havoc'; Runner = 'cpp'; Dir = 'tests/e2e/cases/02-havoc-coverage/multi_method_ordering';      File = 'call_prepare_disproved.cpp';             Expected = 'DISPROVED' }
        @{ Tag = 'cpp_havoc'; Runner = 'cpp'; Dir = 'tests/e2e/cases/02-havoc-coverage/multi_method_ordering';      File = 'call_report_verified.cpp';               Expected = 'VERIFIED' }
        @{ Tag = 'cpp_havoc'; Runner = 'cpp'; Dir = 'tests/e2e/cases/02-havoc-coverage/multi_method_ordering';      File = 'call_extra_disproved.cpp';               Expected = 'DISPROVED' }
        # C++ exception lowering: total Cryptol spec, partial impl.
        @{ Tag = 'cpp_throws'; Runner = 'cpp'; Dir = 'tests/e2e/cases/06-throws-exception';           File = 'add_one_verified.cpp';                   Expected = 'VERIFIED' }
        @{ Tag = 'cpp_throws'; Runner = 'cpp'; Dir = 'tests/e2e/cases/06-throws-exception';           File = 'add_one_disproved.cpp';                  Expected = 'DISPROVED' }
        @{ Tag = 'cpp_throws'; Runner = 'cpp'; Dir = 'tests/e2e/cases/06-throws-exception';           File = 'add_one_throws_caught_verified.cpp';     Expected = 'VERIFIED' }
        @{ Tag = 'cpp_throws'; Runner = 'cpp'; Dir = 'tests/e2e/cases/06-throws-exception';           File = 'add_one_multi_catch_disproved.cpp';      Expected = 'DISPROVED' }
        @{ Tag = 'cpp_throws'; Runner = 'cpp'; Dir = 'tests/e2e/cases/06-throws-exception';           File = 'add_one_rethrow_disproved.cpp';          Expected = 'DISPROVED' }
        @{ Tag = 'cpp_throws'; Runner = 'cpp'; Dir = 'tests/e2e/cases/06-throws-exception';           File = 'add_one_nested_catch_verified.cpp';      Expected = 'VERIFIED' }

        # ── Rust havoc tests (verify-rust.ps1) ──────────────────────────────
        # Note: Rust has no havoc model in this verifier path — every
        # function body SAW can see gets inlined. The 02-havoc-coverage
        # Rust ports are therefore pedagogical mirrors of the C++ tests;
        # only the cases exercising a *distinct* mutation pattern earn
        # a slot in the regression suite. The remaining .rs files in
        # those dirs are kept on disk as side-by-side teaching material
        # but are not registered here. See README.md for the rationale.
        #
        # Smoke: pipeline end-to-end on the simplest possible case.
        @{ Tag = 'rust_havoc'; Runner = 'rust'; Dir = 'tests/e2e/cases/02-havoc-coverage/nothing_sketchy';         File = 'add_one_verified.rs';  Expected = 'VERIFIED'  }
        @{ Tag = 'rust_havoc'; Runner = 'rust'; Dir = 'tests/e2e/cases/02-havoc-coverage/nothing_sketchy';         File = 'add_one_disproved.rs'; Expected = 'DISPROVED' }
        # `static mut` global write tracked across a concrete method call.
        @{ Tag = 'rust_havoc'; Runner = 'rust'; Dir = 'tests/e2e/cases/02-havoc-coverage/concrete_type_safe';      File = 'add_one_disproved.rs'; Expected = 'DISPROVED' }
        # `&mut self` struct-field write tracked across a method call.
        @{ Tag = 'rust_havoc'; Runner = 'rust'; Dir = 'tests/e2e/cases/02-havoc-coverage/class_member_clobbered';  File = 'add_one_disproved.rs'; Expected = 'DISPROVED' }
        # `&mut u32` argument mutation tracked across a free-function call.
        @{ Tag = 'rust_havoc'; Runner = 'rust'; Dir = 'tests/e2e/cases/02-havoc-coverage/input_param_modified';    File = 'add_one_disproved.rs'; Expected = 'DISPROVED' }
        @{ Tag = 'rust_havoc'; Runner = 'rust'; Dir = 'tests/e2e/cases/03-rust-trait-dispatch/static_dispatch';   File = 'add_one_verified.rs';  Expected = 'VERIFIED'  }
        @{ Tag = 'rust_havoc'; Runner = 'rust'; Dir = 'tests/e2e/cases/03-rust-trait-dispatch/static_dispatch';   File = 'add_one_disproved.rs'; Expected = 'DISPROVED' }
        @{ Tag = 'rust_havoc'; Runner = 'rust'; Dir = 'tests/e2e/cases/03-rust-trait-dispatch/dynamic_dispatch';  File = 'add_one_verified.rs';  Expected = 'VERIFIED'  }
        @{ Tag = 'rust_havoc'; Runner = 'rust'; Dir = 'tests/e2e/cases/03-rust-trait-dispatch/dynamic_dispatch';  File = 'add_one_disproved.rs'; Expected = 'DISPROVED' }
        @{ Tag = 'rust_havoc'; Runner = 'rust'; Dir = 'tests/e2e/cases/03-rust-trait-dispatch/external_crate';    File = 'add_one_verified.rs';  Expected = 'VERIFIED'  }
        @{ Tag = 'rust_havoc'; Runner = 'rust'; Dir = 'tests/e2e/cases/03-rust-trait-dispatch/external_crate';    File = 'add_one_disproved.rs'; Expected = 'DISPROVED' }

        # ── Bounded-loop tests (positive) ───────────────────────────────────
        # `add_one_verified.{cpp,rs}` in this dir duplicate the
        # `nothing_sketchy` smoke pair (both are `return x + 1`); they
        # remain on disk as a teaching introduction to the pipeline but
        # are not registered here. Only `sum_first_n` actually exercises
        # bounded-loop unrolling.
        @{ Tag = 'bounded_loop'; Runner = 'cpp';  Dir = 'tests/e2e/cases/01-tutorial/bounded_loop'; File = 'sum_first_n_verified.cpp';  Expected = 'VERIFIED'; Cry = 'sum_first_n_spec.cry'; CryptolFn = 'sum_first_n_spec'; Function = 'sum_first_n' }
        @{ Tag = 'bounded_loop'; Runner = 'rust'; Dir = 'tests/e2e/cases/01-tutorial/bounded_loop'; File = 'sum_first_n_verified.rs';   Expected = 'VERIFIED'; Cry = 'sum_first_n_spec.cry'; CryptolFn = 'sum_first_n_spec'; Function = 'sum_first_n' }

        # ── CSEP590B 26sp Coding Assignment 4 — end-to-end tests ─────────────────────────
        # Five problems × {verified, disproved} (Part A) or {verified}
        # (Part B), each ported to both C++ and Rust. See
        # tests/e2e/cases/01-tutorial/csep590b_c04/README.md for the assignment
        # context and per-problem bounds.
        # Part A — counterexample finding (verified + disproved variants).
        @{ Tag = 'csep590b_c04'; Runner = 'cpp';  Dir = 'tests/e2e/cases/01-tutorial/csep590b_c04/p1_clamp_sub';    File = 'clamp_sub_verified.cpp';     Expected = 'VERIFIED';  Cry = 'clamp_sub_spec.cry';    CryptolFn = 'clamp_sub_spec';    Function = 'clamp_sub'    }
        @{ Tag = 'csep590b_c04'; Runner = 'cpp';  Dir = 'tests/e2e/cases/01-tutorial/csep590b_c04/p1_clamp_sub';    File = 'clamp_sub_disproved.cpp';    Expected = 'DISPROVED'; Cry = 'clamp_sub_spec.cry';    CryptolFn = 'clamp_sub_spec';    Function = 'clamp_sub'    }
        @{ Tag = 'csep590b_c04'; Runner = 'rust'; Dir = 'tests/e2e/cases/01-tutorial/csep590b_c04/p1_clamp_sub';    File = 'clamp_sub_verified.rs';      Expected = 'VERIFIED';  Cry = 'clamp_sub_spec.cry';    CryptolFn = 'clamp_sub_spec';    Function = 'clamp_sub'    }
        @{ Tag = 'csep590b_c04'; Runner = 'rust'; Dir = 'tests/e2e/cases/01-tutorial/csep590b_c04/p1_clamp_sub';    File = 'clamp_sub_disproved.rs';     Expected = 'DISPROVED'; Cry = 'clamp_sub_spec.cry';    CryptolFn = 'clamp_sub_spec';    Function = 'clamp_sub'    }
        @{ Tag = 'csep590b_c04'; Runner = 'cpp';  Dir = 'tests/e2e/cases/01-tutorial/csep590b_c04/p2_safe_mul';     File = 'safe_mul_verified.cpp';      Expected = 'VERIFIED';  Cry = 'safe_mul_spec.cry';     CryptolFn = 'safe_mul_spec';     Function = 'safe_mul'     }
        @{ Tag = 'csep590b_c04'; Runner = 'cpp';  Dir = 'tests/e2e/cases/01-tutorial/csep590b_c04/p2_safe_mul';     File = 'safe_mul_disproved.cpp';     Expected = 'DISPROVED'; Cry = 'safe_mul_spec.cry';     CryptolFn = 'safe_mul_spec';     Function = 'safe_mul'     }
        @{ Tag = 'csep590b_c04'; Runner = 'rust'; Dir = 'tests/e2e/cases/01-tutorial/csep590b_c04/p2_safe_mul';     File = 'safe_mul_verified.rs';       Expected = 'VERIFIED';  Cry = 'safe_mul_spec.cry';     CryptolFn = 'safe_mul_spec';     Function = 'safe_mul'     }
        @{ Tag = 'csep590b_c04'; Runner = 'rust'; Dir = 'tests/e2e/cases/01-tutorial/csep590b_c04/p2_safe_mul';     File = 'safe_mul_disproved.rs';      Expected = 'DISPROVED'; Cry = 'safe_mul_spec.cry';     CryptolFn = 'safe_mul_spec';     Function = 'safe_mul'     }
        @{ Tag = 'csep590b_c04'; Runner = 'cpp';  Dir = 'tests/e2e/cases/01-tutorial/csep590b_c04/p3_count_groups'; File = 'count_groups_verified.cpp';  Expected = 'VERIFIED';  Cry = 'count_groups_spec.cry'; CryptolFn = 'count_groups_spec'; Function = 'count_groups' }
        @{ Tag = 'csep590b_c04'; Runner = 'cpp';  Dir = 'tests/e2e/cases/01-tutorial/csep590b_c04/p3_count_groups'; File = 'count_groups_disproved.cpp'; Expected = 'DISPROVED'; Cry = 'count_groups_spec.cry'; CryptolFn = 'count_groups_spec'; Function = 'count_groups' }
        @{ Tag = 'csep590b_c04'; Runner = 'rust'; Dir = 'tests/e2e/cases/01-tutorial/csep590b_c04/p3_count_groups'; File = 'count_groups_verified.rs';   Expected = 'VERIFIED';  Cry = 'count_groups_spec.cry'; CryptolFn = 'count_groups_spec'; Function = 'count_groups' }
        @{ Tag = 'csep590b_c04'; Runner = 'rust'; Dir = 'tests/e2e/cases/01-tutorial/csep590b_c04/p3_count_groups'; File = 'count_groups_disproved.rs';  Expected = 'DISPROVED'; Cry = 'count_groups_spec.cry'; CryptolFn = 'count_groups_spec'; Function = 'count_groups' }
        # Part B — invariant finding (bounded reference impl only).
        @{ Tag = 'csep590b_c04'; Runner = 'cpp';  Dir = 'tests/e2e/cases/01-tutorial/csep590b_c04/p4_make_change';  File = 'make_change_verified.cpp';   Expected = 'VERIFIED';  Cry = 'make_change_spec.cry';  CryptolFn = 'make_change_spec';  Function = 'make_change'  }
        @{ Tag = 'csep590b_c04'; Runner = 'rust'; Dir = 'tests/e2e/cases/01-tutorial/csep590b_c04/p4_make_change';  File = 'make_change_verified.rs';    Expected = 'VERIFIED';  Cry = 'make_change_spec.cry';  CryptolFn = 'make_change_spec';  Function = 'make_change'  }
        @{ Tag = 'csep590b_c04'; Runner = 'cpp';  Dir = 'tests/e2e/cases/01-tutorial/csep590b_c04/p5_isqrt';        File = 'isqrt_verified.cpp';         Expected = 'VERIFIED';  Cry = 'isqrt_spec.cry';        CryptolFn = 'isqrt_spec';        Function = 'isqrt'        }
        @{ Tag = 'csep590b_c04'; Runner = 'rust'; Dir = 'tests/e2e/cases/01-tutorial/csep590b_c04/p5_isqrt';        File = 'isqrt_verified.rs';          Expected = 'VERIFIED';  Cry = 'isqrt_spec.cry';        CryptolFn = 'isqrt_spec';        Function = 'isqrt'        }

        # ── Integer-operation coverage tests (06-int-ops). ──────────────────
        # Each topic fills a specific gap left by the earlier suites:
        #   min3_i32         multi-argument + signed comparison
        #   is_power_of_two  predicate-style 0/1 return + bit trick
        #   byte_swap_u32    logical shifts + bitwise OR with masks
        #   popcount_u8      u8 input width + small static-bounded loop
        @{ Tag = 'int_ops'; Runner = 'cpp';  Dir = 'tests/e2e/cases/06-int-ops/min3_i32';        File = 'min3_i32_verified.cpp';        Expected = 'VERIFIED';  Cry = 'min3_i32_spec.cry';        CryptolFn = 'min3_i32_spec';        Function = 'min3_i32'        }
        @{ Tag = 'int_ops'; Runner = 'cpp';  Dir = 'tests/e2e/cases/06-int-ops/min3_i32';        File = 'min3_i32_disproved.cpp';       Expected = 'DISPROVED'; Cry = 'min3_i32_spec.cry';        CryptolFn = 'min3_i32_spec';        Function = 'min3_i32'        }
        @{ Tag = 'int_ops'; Runner = 'rust'; Dir = 'tests/e2e/cases/06-int-ops/min3_i32';        File = 'min3_i32_verified.rs';         Expected = 'VERIFIED';  Cry = 'min3_i32_spec.cry';        CryptolFn = 'min3_i32_spec';        Function = 'min3_i32'        }
        @{ Tag = 'int_ops'; Runner = 'rust'; Dir = 'tests/e2e/cases/06-int-ops/min3_i32';        File = 'min3_i32_disproved.rs';        Expected = 'DISPROVED'; Cry = 'min3_i32_spec.cry';        CryptolFn = 'min3_i32_spec';        Function = 'min3_i32'        }
        @{ Tag = 'int_ops'; Runner = 'cpp';  Dir = 'tests/e2e/cases/06-int-ops/is_power_of_two'; File = 'is_power_of_two_verified.cpp'; Expected = 'VERIFIED';  Cry = 'is_power_of_two_spec.cry'; CryptolFn = 'is_power_of_two_spec'; Function = 'is_power_of_two' }
        @{ Tag = 'int_ops'; Runner = 'cpp';  Dir = 'tests/e2e/cases/06-int-ops/is_power_of_two'; File = 'is_power_of_two_disproved.cpp';Expected = 'DISPROVED'; Cry = 'is_power_of_two_spec.cry'; CryptolFn = 'is_power_of_two_spec'; Function = 'is_power_of_two' }
        @{ Tag = 'int_ops'; Runner = 'rust'; Dir = 'tests/e2e/cases/06-int-ops/is_power_of_two'; File = 'is_power_of_two_verified.rs';  Expected = 'VERIFIED';  Cry = 'is_power_of_two_spec.cry'; CryptolFn = 'is_power_of_two_spec'; Function = 'is_power_of_two' }
        @{ Tag = 'int_ops'; Runner = 'rust'; Dir = 'tests/e2e/cases/06-int-ops/is_power_of_two'; File = 'is_power_of_two_disproved.rs'; Expected = 'DISPROVED'; Cry = 'is_power_of_two_spec.cry'; CryptolFn = 'is_power_of_two_spec'; Function = 'is_power_of_two' }
        @{ Tag = 'int_ops'; Runner = 'cpp';  Dir = 'tests/e2e/cases/06-int-ops/byte_swap_u32';   File = 'byte_swap_u32_verified.cpp';   Expected = 'VERIFIED';  Cry = 'byte_swap_u32_spec.cry';   CryptolFn = 'byte_swap_u32_spec';   Function = 'byte_swap_u32'   }
        @{ Tag = 'int_ops'; Runner = 'cpp';  Dir = 'tests/e2e/cases/06-int-ops/byte_swap_u32';   File = 'byte_swap_u32_disproved.cpp';  Expected = 'DISPROVED'; Cry = 'byte_swap_u32_spec.cry';   CryptolFn = 'byte_swap_u32_spec';   Function = 'byte_swap_u32'   }
        @{ Tag = 'int_ops'; Runner = 'rust'; Dir = 'tests/e2e/cases/06-int-ops/byte_swap_u32';   File = 'byte_swap_u32_verified.rs';    Expected = 'VERIFIED';  Cry = 'byte_swap_u32_spec.cry';   CryptolFn = 'byte_swap_u32_spec';   Function = 'byte_swap_u32'   }
        @{ Tag = 'int_ops'; Runner = 'rust'; Dir = 'tests/e2e/cases/06-int-ops/byte_swap_u32';   File = 'byte_swap_u32_disproved.rs';   Expected = 'DISPROVED'; Cry = 'byte_swap_u32_spec.cry';   CryptolFn = 'byte_swap_u32_spec';   Function = 'byte_swap_u32'   }
        @{ Tag = 'int_ops'; Runner = 'cpp';  Dir = 'tests/e2e/cases/06-int-ops/popcount_u8';     File = 'popcount_u8_verified.cpp';     Expected = 'VERIFIED';  Cry = 'popcount_u8_spec.cry';     CryptolFn = 'popcount_u8_spec';     Function = 'popcount_u8'     }
        @{ Tag = 'int_ops'; Runner = 'cpp';  Dir = 'tests/e2e/cases/06-int-ops/popcount_u8';     File = 'popcount_u8_disproved.cpp';    Expected = 'DISPROVED'; Cry = 'popcount_u8_spec.cry';     CryptolFn = 'popcount_u8_spec';     Function = 'popcount_u8'     }
        @{ Tag = 'int_ops'; Runner = 'rust'; Dir = 'tests/e2e/cases/06-int-ops/popcount_u8';     File = 'popcount_u8_verified.rs';      Expected = 'VERIFIED';  Cry = 'popcount_u8_spec.cry';     CryptolFn = 'popcount_u8_spec';     Function = 'popcount_u8'     }
        @{ Tag = 'int_ops'; Runner = 'rust'; Dir = 'tests/e2e/cases/06-int-ops/popcount_u8';     File = 'popcount_u8_disproved.rs';     Expected = 'DISPROVED'; Cry = 'popcount_u8_spec.cry';     CryptolFn = 'popcount_u8_spec';     Function = 'popcount_u8'     }

        # ── String operations (SWAR null-byte detection: real libc strlen
        #    bit-trick over a 64-bit word treated as 8 packed bytes). ───────
        @{ Tag = 'string_ops'; Runner = 'cpp'; Dir = 'tests/e2e/cases/05-string-ops/has_null_byte'; File = 'has_null_byte_verified.cpp';  Cry = 'has_null_byte_spec.cry'; CryptolFn = 'has_null_byte_spec'; Function = 'has_null_byte'; Expected = 'VERIFIED'  }
        @{ Tag = 'string_ops'; Runner = 'cpp'; Dir = 'tests/e2e/cases/05-string-ops/has_null_byte'; File = 'has_null_byte_disproved.cpp'; Cry = 'has_null_byte_spec.cry'; CryptolFn = 'has_null_byte_spec'; Function = 'has_null_byte'; Expected = 'DISPROVED' }

        # ── Real C-string test: count_digits over `_In_reads_(8) const char*`.
        #    Exercises the SAL count annotation -> 8-byte buffer allocation
        #    path plus the auto-emitted llvm.var.annotation override. ───────
        @{ Tag = 'strings'; Runner = 'cpp'; Dir = 'tests/e2e/cases/05-string-ops/count_digits'; File = 'count_digits_cstr_verified.cpp';  Cry = 'count_digits_spec.cry'; CryptolFn = 'count_digits_spec'; Function = 'count_digits'; Expected = 'VERIFIED'  }
        @{ Tag = 'strings'; Runner = 'cpp'; Dir = 'tests/e2e/cases/05-string-ops/count_digits'; File = 'count_digits_cstr_disproved.cpp'; Cry = 'count_digits_spec.cry'; CryptolFn = 'count_digits_spec'; Function = 'count_digits'; Expected = 'DISPROVED' }

        # ── ArrayView rule 1 (saw_spec_gen-4po): bind Cryptol type
        #    variable to C++ pointer length. The Cryptol spec carries
        #    `{n}(fin n, n <= 8) => [n][8] -> [32]`.
        #    saw-spec-gen reads the upper bound automatically (no annotation
        #    or CLI flag needed) and allocates an 8-byte buffer. ─────────────
        @{ Tag = 'cryptol_len_bind'; Runner = 'cpp'; Dir = 'tests/e2e/cases/05-string-ops/cryptol_len_bind'; File = 'count_bytes_bind_verified.cpp'; Cry = 'count_bytes_spec.cry'; CryptolFn = 'count_bytes_spec'; Function = 'count_bytes'; Expected = 'VERIFIED' }
        # Deliberate value bug (widens digit range to include '@'). The
        # config still allocates 8 bytes, so reads succeed; the proof fails
        # because the symbolic solver finds a witness — DISPROVED for the
        # *right* reason (real arithmetic bug, not allocation size).
        @{ Tag = 'cryptol_len_bind'; Runner = 'cpp'; Dir = 'tests/e2e/cases/05-string-ops/cryptol_len_bind'; File = 'count_bytes_bind_disproved.cpp'; Cry = 'count_bytes_spec.cry'; CryptolFn = 'count_bytes_spec'; Function = 'count_bytes'; Expected = 'DISPROVED' }

        # ── ArrayView rule 2 (saw_spec_gen-5mt): `_In_z_(N)` SAL macro
        #    for null-terminated input strings. The macro allocates an
        #    N-byte buffer (same shape as `_In_reads_(N)`) and emits a
        #    NUL-content precondition (`any (\b -> b == 0) s`) so SAW
        #    only explores buffers that contain a terminator within the
        #    declared bound. ─────────────────────────────────────────────
        @{ Tag = 'in_z_macro'; Runner = 'cpp'; Dir = 'tests/e2e/cases/05-string-ops/in_z_macro'; File = 'count_digits_z_verified.cpp'; Cry = 'count_digits_z_spec.cry'; CryptolFn = 'count_digits_z_spec'; Function = 'count_digits_z'; Expected = 'VERIFIED' }
        # Deliberate value bug (counts non-digits). The _In_z_(8) macro
        # still allocates 8 bytes, so reads succeed; the proof fails on
        # the value, not the allocation — DISPROVED for the *right*
        # reason (real arithmetic bug).
        @{ Tag = 'in_z_macro'; Runner = 'cpp'; Dir = 'tests/e2e/cases/05-string-ops/in_z_macro'; File = 'count_digits_z_disproved.cpp'; Cry = 'count_digits_z_spec.cry'; CryptolFn = 'count_digits_z_spec'; Function = 'count_digits_z'; Expected = 'DISPROVED' }

        # ── ArrayView rule 4 (saw_spec_gen-26d): struct-shape recognizer
        #    auto-pairs `(T* buf, size_t len)` parameters. With the
        #    recognizer (default): buf is sized to its length sibling
        #    → VERIFIED. With `--no-struct-shape-recognizer`: legacy
        #    1-byte fallback → DISPROVED. ─────────────────────────────────
        @{ Tag = 'struct_shape'; Runner = 'cpp'; Dir = 'tests/e2e/cases/05-string-ops/struct_shape_recognizer'; File = 'sum_first_byte_verified.cpp'; Cry = 'sum_first_byte_spec.cry'; CryptolFn = 'sum_first_byte_spec'; Function = 'sum_first_byte'; Expected = 'VERIFIED' }
        # Same source, recognizer disabled: 1-byte fallback alloc keeps
        # buf[0] in-bounds but the precondition the recognizer would
        # have emitted (`len <= MAX`) is gone, so the proof fails on
        # the unbounded len case → DISPROVED (recognizer regression
        # witness).
        @{ Tag = 'struct_shape'; Runner = 'cpp'; Dir = 'tests/e2e/cases/05-string-ops/struct_shape_recognizer'; File = 'sum_first_byte_verified.cpp'; Cry = 'sum_first_byte_spec.cry'; CryptolFn = 'sum_first_byte_spec'; Function = 'sum_first_byte'; Expected = 'DISPROVED'; ExtraSpecGenArgs = @('--no-struct-shape-recognizer') }
        # Deliberate value bug (returns buf[0] + 1). The recognizer
        # (default) still sizes buf to its length sibling, so reads
        # succeed; the proof fails on the value — DISPROVED for the
        # *right* reason (real arithmetic bug).
        @{ Tag = 'struct_shape'; Runner = 'cpp'; Dir = 'tests/e2e/cases/05-string-ops/struct_shape_recognizer'; File = 'sum_first_byte_disproved.cpp'; Cry = 'sum_first_byte_spec.cry'; CryptolFn = 'sum_first_byte_spec'; Function = 'sum_first_byte'; Expected = 'DISPROVED' }


        # ── String content verification (string_content) ───────────────────
        # Verifies the *actual byte contents* of output buffers, not just
        # length/presence. The C++ source uses _Out_writes_(N) / _In_reads_(N)
        # SAL annotations; saw-spec-gen auto-detects the <param>_post Cryptol
        # convention and emits postconditions with no CLI flags.
        #
        # write_hello: fills a 5-byte _Out_writes_(5) buffer with "hello".
        #   verified  -- writes exactly [0x68,0x65,0x6c,0x6c,0x6f]; postcondition holds.
        #   disproved -- writes "world" instead; postcondition catches the mismatch.
        @{ Tag = 'string_content'; Runner = 'cpp'; Dir = 'tests/e2e/cases/05-string-ops/string_content'; File = 'write_hello_verified.cpp';  Expected = 'VERIFIED';
           Cry = 'string_content_spec.cry'; CryptolFn = 'write_hello_ret'; Function = 'write_hello' }
        @{ Tag = 'string_content'; Runner = 'cpp'; Dir = 'tests/e2e/cases/05-string-ops/string_content'; File = 'write_hello_disproved.cpp'; Expected = 'DISPROVED';
           Cry = 'string_content_spec.cry'; CryptolFn = 'write_hello_ret'; Function = 'write_hello' }
        # to_lower_ascii: _In_reads_(6) src -> _Out_writes_(6) dst, A-Z -> a-z.
        #   verified  -- correct lowercasing (A-Z -> a-z, others pass through).
        #   disproved -- applies the shift to the wrong range; SAW finds
        #               a counterexample at 'A' (expected 'a', got 'A').
        @{ Tag = 'string_content'; Runner = 'cpp'; Dir = 'tests/e2e/cases/05-string-ops/string_content'; File = 'to_lower_ascii_verified.cpp';  Expected = 'VERIFIED';
           Cry = 'string_content_spec.cry'; CryptolFn = 'to_lower_ascii_ret'; Function = 'to_lower_ascii' }
        @{ Tag = 'string_content'; Runner = 'cpp'; Dir = 'tests/e2e/cases/05-string-ops/string_content'; File = 'to_lower_ascii_disproved.cpp'; Expected = 'DISPROVED';
           Cry = 'string_content_spec.cry'; CryptolFn = 'to_lower_ascii_ret'; Function = 'to_lower_ascii' }

        # ── Bitcode-driven extern override tests ────────────────────────────
        @{ Tag = 'cpp_overrides'; Runner = 'cpp'; Dir = 'tests/e2e/cases/08-overrides/bump';                    File = 'bump_verified.cpp';          Cry = 'bump_spec.cry';          CryptolFn = 'bump_spec';          Function = 'bump';          Expected = 'VERIFIED'  }
        @{ Tag = 'cpp_overrides'; Runner = 'cpp'; Dir = 'tests/e2e/cases/08-overrides/use_helper';              File = 'use_helper_verified.cpp';    Cry = 'use_helper_spec.cry';    CryptolFn = 'use_helper_spec';    Function = 'use_helper';    Expected = 'VERIFIED'  }
        @{ Tag = 'cpp_overrides'; Runner = 'cpp'; Dir = 'tests/e2e/cases/08-overrides/user_variadic';           File = 'bump_with_log_verified.cpp'; Cry = 'bump_with_log_spec.cry'; CryptolFn = 'bump_with_log_spec'; Function = 'bump_with_log'; Expected = 'VERIFIED'  }
        @{ Tag = 'cpp_overrides'; Runner = 'cpp'; Dir = 'tests/e2e/cases/08-overrides/variadic_clobber';        File = 'add_one_disproved.cpp';      Cry = 'add_one_spec.cry';       CryptolFn = 'add_one_spec';       Function = 'add_one_disproved';       Expected = 'DISPROVED' }
        @{ Tag = 'cpp_overrides'; Runner = 'cpp'; Dir = 'tests/e2e/cases/08-overrides/variadic_global_clobber'; File = 'add_one_disproved.cpp';      Cry = 'add_one_spec.cry';       CryptolFn = 'add_one_spec';       Function = 'add_one_disproved';       Expected = 'DISPROVED' }
        # Mutex success-sentinel: `_Mtx_lock`/`_Mtx_unlock` are declare-only
        # `_Thrd_result` primitives whose override pins the `_Thrd_success` (0)
        # sentinel instead of a fresh return. The verified case only proves
        # because the spurious lock-failure branch is dead; the disproved case
        # shows the pin still lets a genuine off-by-one bug surface.
        @{ Tag = 'cpp_overrides'; Runner = 'cpp'; Dir = 'tests/e2e/cases/08-overrides/mutex_sentinel';          File = 'guarded_add_verified.cpp';   Cry = 'guarded_add_spec.cry';   CryptolFn = 'guarded_add_spec';   Function = 'guarded_add';             Expected = 'VERIFIED'  }
        @{ Tag = 'cpp_overrides'; Runner = 'cpp'; Dir = 'tests/e2e/cases/08-overrides/mutex_sentinel';          File = 'guarded_add_disproved.cpp';  Cry = 'guarded_add_spec.cry';   CryptolFn = 'guarded_add_spec';   Function = 'guarded_add_disproved';   Expected = 'DISPROVED' }
        # MSVC _Mutex_base::_Verify_ownership_levels no-op override (bd issue #65).
        # This defined-in-module helper performs typed field reads on the mutex
        # struct. Before the MsvcMutexHelper fix, extern_override_scan skipped
        # it and SAW failed with "Error during memory load" when inlining its
        # body against a symbolically-allocated struct. With the fix it is
        # detected via the _Verify_ownership_levels substring pattern (fires on
        # both MSVC and GCC/Clang mangled names) and overridden as a no-op
        # returning {{ 1 : [1] }} (ownership always valid in sequential proofs).
        @{ Tag = 'cpp_overrides'; Runner = 'cpp'; Dir = 'tests/e2e/cases/08-overrides/msvc_mutex_helper'; File = 'ownership_check_verified.cpp';  Cry = 'ownership_check_spec.cry'; CryptolFn = 'ownership_check_spec'; Function = 'ownership_check';           Expected = 'VERIFIED'  }
        @{ Tag = 'cpp_overrides'; Runner = 'cpp'; Dir = 'tests/e2e/cases/08-overrides/msvc_mutex_helper'; File = 'ownership_check_disproved.cpp'; Cry = 'ownership_check_spec.cry'; CryptolFn = 'ownership_check_spec'; Function = 'ownership_check_disproved'; Expected = 'DISPROVED' }

        # ── C++/Rust equivalence tests (verify-equiv.ps1) ───────────────────
        @{ Tag = 'rust_equiv'; Runner = 'equiv'; Dir = 'tests/e2e/cases/04-cpp-rust-equivalence/compute_fee_reordered';         Cpp = 'compute_fee.cpp'; Rust = 'compute_fee_verified.rs';  Cry = 'compute_fee_spec.cry'; CryptolFn = 'compute_fee_spec'; Function = 'compute_fee'; Expected = 'EQUIVALENT'     }
        @{ Tag = 'rust_equiv'; Runner = 'equiv'; Dir = 'tests/e2e/cases/04-cpp-rust-equivalence/compute_fee_reordered';         Cpp = 'compute_fee.cpp'; Rust = 'compute_fee_disproved.rs'; Cry = 'compute_fee_spec.cry'; CryptolFn = 'compute_fee_spec'; Function = 'compute_fee'; Expected = 'NOT EQUIVALENT' }
        @{ Tag = 'rust_equiv'; Runner = 'equiv'; Dir = 'tests/e2e/cases/04-cpp-rust-equivalence/sat_add_optimized';   Cpp = 'sat_add.cpp';     Rust = 'sat_add_verified.rs';  Cry = 'sat_add_spec.cry';     CryptolFn = 'sat_add_spec';     Function = 'sat_add';     Expected = 'EQUIVALENT'     }
        @{ Tag = 'rust_equiv'; Runner = 'equiv'; Dir = 'tests/e2e/cases/04-cpp-rust-equivalence/sat_add_optimized';   Cpp = 'sat_add.cpp';     Rust = 'sat_add_disproved.rs'; Cry = 'sat_add_spec.cry';     CryptolFn = 'sat_add_spec';     Function = 'sat_add';     Expected = 'NOT EQUIVALENT' }
        @{ Tag = 'rust_equiv'; Runner = 'equiv'; Dir = 'tests/e2e/cases/04-cpp-rust-equivalence/not_operator_trap'; Cpp = 'negate.cpp';     Rust = 'negate.rs';           Cry = 'negate_spec.cry';      CryptolFn = 'negate_spec';      Function = 'negate';      Expected = 'NOT EQUIVALENT' }

        # ── Enum type-constraint tests (saw-spec-gen-xip) ───────────────────
        # Without the auto-emitted `llvm_precond {{ r <= (variants-1 : [N]) }}`,
        # SAW explores out-of-range discriminants and DISPROVES the spec. With
        # the precondition the generated SAW script verifies that the C++
        # implementation matches its Cryptol spec on every legal variant.
        @{ Tag = 'enum_constraints'; Runner = 'cpp';  Dir = 'tests/e2e/cases/07-enum-constraints/auth_enum'; File = 'auth_enum_verified.cpp'; Expected = 'VERIFIED'; Cry = 'auth_enum_spec.cry'; CryptolFn = 'classify_spec'; Function = 'classify' }
        # Rust verifies even without a precondition because rustc lowers
        # the exhaustive `match` so out-of-range tags hit LLVM
        # `unreachable` (UB → vacuously valid). The case is registered
        # as a regression: if the Rust pipeline ever stops emitting that
        # unreachable, we want to notice and emit the precondition there
        # too (verify-rust.ps1 doesn't currently use saw-spec-gen).
        @{ Tag = 'enum_constraints'; Runner = 'rust'; Dir = 'tests/e2e/cases/07-enum-constraints/auth_enum'; File = 'auth_enum_verified.rs';  Expected = 'VERIFIED'; Cry = 'auth_enum_spec.cry'; CryptolFn = 'classify_spec'; Function = 'classify' }
        # Gapped (non-contiguous) enum: `Status : uint8_t { Ok = 0,
        # NotFound = 2, Denied = 100 }`. Regression for bd issue
        # `saw_spec_gen-iyh` — saw-spec-gen used to emit a contiguous
        # range bound `r <= (variants-1 : [N])` that both admitted
        # undefined gap values (e.g. r = 1) and excluded valid tags
        # (e.g. r = 100). The fix emits a membership disjunction; this
        # case fails to verify without it.
        @{ Tag = 'enum_constraints'; Runner = 'cpp';  Dir = 'tests/e2e/cases/07-enum-constraints/gapped_enum'; File = 'gapped_enum_verified.cpp'; Expected = 'VERIFIED'; Cry = 'gapped_enum_spec.cry'; CryptolFn = 'classify_spec'; Function = 'classify' }

        # ── Unknown concrete-impl dyn-trait case — removed pending real
        #    tool support. See bd issue saw-spec-gen-uki: restore once
        #    verify-rust.ps1 can derive a TraitSchema + fat-pointer
        #    driver directly from the Rust source (no hand-rolled SAW
        #    script, no duplicated trait_schema.json).

        # ── Async-Rust coroutine test — removed pending real tool support.
        #    See bd issue saw-spec-gen-tfg: restore once verify-rust.ps1
        #    learns to detect coroutine lowering and resolve the resume
        #    symbol natively (no per-test bespoke .ps1 driver).

        # ── Rust adversarial research cases: each demonstrates a specific
        #    verifier blind spot or successful adversarial coverage.
        #    Expected verdicts captured from baseline runs.
        @{ Tag = 'rust_adversarial'; Runner = 'rust'; Dir = 'tests/e2e/cases/99-research/rust_adversarial/cell_interior_mutation';  File = 'add_one_disproved.rs'; Expected = 'DISPROVED' }
        @{ Tag = 'rust_adversarial'; Runner = 'rust'; Dir = 'tests/e2e/cases/99-research/rust_adversarial/cross_fn_global';         File = 'add_one_verified.rs';  Expected = 'VERIFIED'  }
        @{ Tag = 'rust_adversarial'; Runner = 'rust'; Dir = 'tests/e2e/cases/99-research/rust_adversarial/raw_pointer_aliasing';    File = 'add_one_disproved.rs'; Expected = 'DISPROVED' }
        @{ Tag = 'rust_adversarial'; Runner = 'rust'; Dir = 'tests/e2e/cases/99-research/rust_adversarial/symbol_collision';        File = 'add_one_disproved.rs'; Expected = 'DISPROVED' }
        @{ Tag = 'rust_adversarial'; Runner = 'rust'; Dir = 'tests/e2e/cases/99-research/rust_adversarial/unreachable_unchecked';   File = 'add_one_disproved.rs'; Expected = 'DISPROVED' }

        # ── STL coverage (verify.ps1) ───────────────────────────────────────
        # "Typical C++" cases that exercise the standard library: <algorithm>,
        # <numeric>, <utility>, <tuple>, <memory>, <vector>. Every entry in
        # this section currently RESOLVES TO DISPROVED — not because the
        # code is wrong, but because the tool's compositional havoc model
        # treats every function in a system header as an adversarial extern.
        # `std::max(y, y)` is a one-line `return a < b ? b : a;` in the
        # bitcode, but the auto-spec lets the solver return ANYTHING from
        # it, so equivalence with `add_one_spec` is refuted.
        #
        # This block documents the gap so regressions stand out (any case
        # tests/e2e/cases/10-stl-coverage/README.md for the matrix +
        # rationale. The `algorithm_clamp` case requires C++17.
        @{ Tag = 'stl_coverage'; Runner = 'cpp'; Dir = 'tests/e2e/cases/10-stl-coverage/algorithm_max';          File = 'add_one_verified.cpp';  Expected = 'VERIFIED' }
        @{ Tag = 'stl_coverage'; Runner = 'cpp'; Dir = 'tests/e2e/cases/10-stl-coverage/algorithm_max';          File = 'add_one_disproved.cpp'; Expected = 'DISPROVED' }
        @{ Tag = 'stl_coverage'; Runner = 'cpp'; Dir = 'tests/e2e/cases/10-stl-coverage/algorithm_min';          File = 'add_one_verified.cpp';  Expected = 'VERIFIED' }
        @{ Tag = 'stl_coverage'; Runner = 'cpp'; Dir = 'tests/e2e/cases/10-stl-coverage/algorithm_min';          File = 'add_one_disproved.cpp'; Expected = 'DISPROVED' }
        @{ Tag = 'stl_coverage'; Runner = 'cpp'; Dir = 'tests/e2e/cases/10-stl-coverage/algorithm_clamp';        File = 'add_one_verified.cpp';  Expected = 'VERIFIED'; CxxStandard = 'c++17' }
        @{ Tag = 'stl_coverage'; Runner = 'cpp'; Dir = 'tests/e2e/cases/10-stl-coverage/algorithm_clamp';        File = 'add_one_disproved.cpp'; Expected = 'DISPROVED'; CxxStandard = 'c++17' }
        @{ Tag = 'stl_coverage'; Runner = 'cpp'; Dir = 'tests/e2e/cases/10-stl-coverage/numeric_accumulate';     File = 'add_one_verified.cpp';  Expected = 'VERIFIED' }
        @{ Tag = 'stl_coverage'; Runner = 'cpp'; Dir = 'tests/e2e/cases/10-stl-coverage/numeric_accumulate';     File = 'add_one_disproved.cpp'; Expected = 'DISPROVED' }
        @{ Tag = 'stl_coverage'; Runner = 'cpp'; Dir = 'tests/e2e/cases/10-stl-coverage/pair_first';             File = 'add_one_verified.cpp';  Expected = 'VERIFIED' }
        @{ Tag = 'stl_coverage'; Runner = 'cpp'; Dir = 'tests/e2e/cases/10-stl-coverage/pair_first';             File = 'add_one_disproved.cpp'; Expected = 'DISPROVED' }
        @{ Tag = 'stl_coverage'; Runner = 'cpp'; Dir = 'tests/e2e/cases/10-stl-coverage/tuple_get';              File = 'add_one_verified.cpp';  Expected = 'VERIFIED' }
        @{ Tag = 'stl_coverage'; Runner = 'cpp'; Dir = 'tests/e2e/cases/10-stl-coverage/tuple_get';              File = 'add_one_disproved.cpp'; Expected = 'DISPROVED' }
        @{ Tag = 'stl_coverage'; Runner = 'cpp'; Dir = 'tests/e2e/cases/10-stl-coverage/vector_back_havoc';      File = 'add_one_verified.cpp';  Expected = 'VERIFIED' }
        @{ Tag = 'stl_coverage'; Runner = 'cpp'; Dir = 'tests/e2e/cases/10-stl-coverage/vector_back_havoc';      File = 'add_one_disproved.cpp'; Expected = 'DISPROVED' }
        # std::string mirror of vector_back_havoc: exercises the
        # `St12basic_string` / `St7__cxx1112basic_string` substring
        # patterns in the jpp override registry. Both variants are
        # DISPROVED: the gap case can't couple resize(N) with size(),
        # the wrong case writes the wrong length on purpose.
        @{ Tag = 'stl_coverage'; Runner = 'cpp'; Dir = 'tests/e2e/cases/10-stl-coverage/string_size_havoc';      File = 'add_one_verified.cpp';  Expected = 'VERIFIED' }
        @{ Tag = 'stl_coverage'; Runner = 'cpp'; Dir = 'tests/e2e/cases/10-stl-coverage/string_size_havoc';      File = 'add_one_disproved.cpp'; Expected = 'DISPROVED' }
        # std::string SSO regression (saw_spec_gen-xzg): exercises
        # `basic_string::data()` on a symbolic-length string so both
        # the SSO branch (len <= 15) and the heap branch (len > 15)
        # flow through the override. The model-agnostic option (a)
        # spec verifies under both branches.
      @{ Tag = 'stl_coverage'; Runner = 'cpp'; Dir = 'tests/e2e/cases/10-stl-coverage/string_data_sso';        File = 'add_one_verified.cpp';  Expected = 'VERIFIED' }
      @{ Tag = 'stl_coverage'; Runner = 'cpp'; Dir = 'tests/e2e/cases/10-stl-coverage/string_cstr';            File = 'add_one_verified.cpp';  Expected = 'VERIFIED' }
         @{ Tag = 'stl_coverage'; Runner = 'cpp'; Dir = 'tests/e2e/cases/10-stl-coverage/string_cstr';            File = 'add_one_disproved.cpp'; Expected = 'DISPROVED' }
      @{ Tag = 'stl_coverage'; Runner = 'cpp'; Dir = 'tests/e2e/cases/10-stl-coverage/string_index';           File = 'add_one_verified.cpp';  Expected = 'VERIFIED' }
         @{ Tag = 'stl_coverage'; Runner = 'cpp'; Dir = 'tests/e2e/cases/10-stl-coverage/string_index';           File = 'add_one_disproved.cpp'; Expected = 'DISPROVED' }
      @{ Tag = 'stl_coverage'; Runner = 'cpp'; Dir = 'tests/e2e/cases/10-stl-coverage/string_copy_assign';     File = 'add_one_verified.cpp';  Expected = 'VERIFIED' }
         @{ Tag = 'stl_coverage'; Runner = 'cpp'; Dir = 'tests/e2e/cases/10-stl-coverage/string_copy_assign';     File = 'add_one_disproved.cpp'; Expected = 'DISPROVED' }
      @{ Tag = 'stl_coverage'; Runner = 'cpp'; Dir = 'tests/e2e/cases/10-stl-coverage/string_empty';           File = 'add_one_verified.cpp';  Expected = 'VERIFIED' }
         @{ Tag = 'stl_coverage'; Runner = 'cpp'; Dir = 'tests/e2e/cases/10-stl-coverage/string_empty';           File = 'add_one_disproved.cpp'; Expected = 'DISPROVED' }
      @{ Tag = 'stl_coverage'; Runner = 'cpp'; Dir = 'tests/e2e/cases/10-stl-coverage/string_reserve';         File = 'add_one_verified.cpp';  Expected = 'VERIFIED' }
         @{ Tag = 'stl_coverage'; Runner = 'cpp'; Dir = 'tests/e2e/cases/10-stl-coverage/string_reserve';         File = 'add_one_disproved.cpp'; Expected = 'DISPROVED' }
        @{ Tag = 'stl_coverage'; Runner = 'cpp'; Dir = 'tests/e2e/cases/10-stl-coverage/unique_ptr_deref_havoc'; File = 'add_one_gap_disproved.cpp';  Expected = 'DISPROVED' }
        @{ Tag = 'stl_coverage'; Runner = 'cpp'; Dir = 'tests/e2e/cases/10-stl-coverage/unique_ptr_deref_havoc'; File = 'add_one_disproved.cpp'; Expected = 'DISPROVED' }

        # ── sret pre-state slice (09-type-coverage) ─────────────────────────────
        # Returns a 16-byte struct via sret. Cryptol model has a trailing
        # [12][8] pre-state param (body field at offset 4) — saw-spec-gen must
        # emit take`{12}/drop`{4} slice, not the full buffer.
        @{ Tag = 'sret_slice'; Runner = 'cpp'; Dir = 'tests/e2e/cases/09-type-coverage/sret_slice'; File = 'stamp_header_verified.cpp'; Expected = 'VERIFIED'; Cry = 'stamp_header_spec.cry'; CryptolFn = 'stamp_header_spec'; Function = 'stamp_header' }

        # ── Buffer overrides (08-overrides) ─────────────────────────────────
        # Exercises the --out-buffer-param / --in-buffer-size /
        # --cryptol-fn-out / --max-len-precond CLI surface plus the
        # `emit_param_preconditions_filtered` cosmetic-TODO suppressor
        # in src/emit/saw_emit/verify_script_steps.rs. The override
        # branch is the same one demo_protocol's canonicalize_lp relies
        # on for its real-world proof — without this case, that pipeline
        # is our only end-to-end witness.
        #
        # verified  : honours the postcondition (out[nb..] left untouched).
        # disproved : zero-fills out[nb..]; postcondition rejects it,
        #             proving the override-branch postcondition is not
        #             vacuous.
        @{ Tag = 'cpp_overrides'; Runner = 'cpp'; Dir = 'tests/e2e/cases/08-overrides/bounded_copy'; File = 'bounded_copy_verified.cpp';  Expected = 'VERIFIED';
           Cry = 'bounded_copy_spec.cry'; CryptolFn = 'bounded_copy_ret'; Function = 'bounded_copy';
           ExtraSpecGenArgs = @(
               '--in-buffer-size',    'src=4',
               '--out-buffer-param',  'out=4',
               '--cryptol-fn-out',    'out=bounded_copy_post',
               '--max-len-precond',   'nb=4'
           ) }
        @{ Tag = 'cpp_overrides'; Runner = 'cpp'; Dir = 'tests/e2e/cases/08-overrides/bounded_copy'; File = 'bounded_copy_disproved.cpp'; Expected = 'DISPROVED';
           Cry = 'bounded_copy_spec.cry'; CryptolFn = 'bounded_copy_ret'; Function = 'bounded_copy';
           ExtraSpecGenArgs = @(
               '--in-buffer-size',    'src=4',
               '--out-buffer-param',  'out=4',
               '--cryptol-fn-out',    'out=bounded_copy_post',
               '--max-len-precond',   'nb=4'
           ) }

        # ── Uninterpreted primitives (08-overrides/uninterpreted) ───────────
        # Exercises the `@uninterpreted` Cryptol annotation surface: an
        # opaque primitive `prim` is bound to its Cryptol model via
        # `llvm_unsafe_assume_spec` instead of being symbolically
        # executed. No CLI flag — the annotation in use_prim_spec.cry is
        # the only declaration needed. See
        # src/uninterpreted.rs and docs/uninterpreted-primitives-proposal.md.
        #
        # verified  : caller forwards the primitive result unchanged.
        # disproved : caller perturbs the result (+1); the assumed
        #             contract rejects it, proving it is not vacuous.
        @{ Tag = 'cpp_overrides'; Runner = 'cpp'; Dir = 'tests/e2e/cases/08-overrides/uninterpreted'; File = 'use_prim_verified.cpp';  Expected = 'VERIFIED';
           Cry = 'use_prim_spec.cry'; CryptolFn = 'use_prim_spec'; Function = 'use_prim' }
        @{ Tag = 'cpp_overrides'; Runner = 'cpp'; Dir = 'tests/e2e/cases/08-overrides/uninterpreted'; File = 'use_prim_disproved.cpp'; Expected = 'DISPROVED';
           Cry = 'use_prim_spec.cry'; CryptolFn = 'use_prim_spec'; Function = 'use_prim' }

        # ── Stateful methods: whole-object post-state via the ordinary
        #    --out-buffer-param / --cryptol-fn-out flags (no dedicated
        #    flag needed; see docs/03-stateful-method-specs.md).
        #
        # A stateful method's headline safety property is relational over
        # the pre/post heap, which the default functional-equivalence spec
        # (`f(x) == model(x)`) can't express. Modelling the object as a
        # writable byte buffer and pinning its post-state with
        # `--cryptol-fn-out` closes the gap — the same machinery used for
        # ordinary output buffers.
        #
        # key_store : single-byte latch (isActive 0/1 → 1).
        # block     : wide but byte-granular — a uint8[4] buffer XOR-ed
        #             byte-by-byte (post == map (^0xAB) pre).
        # session   : multi-field with kept (unchanged) members — set
        #             isOpen, preserve the tag bytes (`[1] # drop pre`).
        # counter   : a *wide typed field* — a bare uint32 incremented as
        #             a single i32 store. The byte-array object model
        #             can't satisfy that access, so the object is typed
        #             explicitly via the `i32` out-buffer shape
        #             (`--out-buffer-param c=i32`).
        #
        # key_store/block/session model as plain byte buffers; counter
        # exercises the typed `iW` out-buffer shape that lifts the
        # byte-granular restriction for wide scalar fields.
        #
        # Each *_disproved variant proves the post-condition is not
        # vacuous: the buggy body diverges from the model and SAW returns
        # the discriminating counterexample.
        @{ Tag = 'cpp_stateful'; Runner = 'cpp'; Dir = 'tests/e2e/cases/09-stateful/key_store'; File = 'key_store_verified.cpp';  Expected = 'VERIFIED';
           Cry = 'key_store_spec.cry'; CryptolFn = 'key_store_activate_ret'; Function = 'key_store_activate';
           ExtraSpecGenArgs = @(
               '--out-buffer-param', 'ks=1',
               '--cryptol-fn-out',   'ks=key_store_activate_post'
           ) }
        @{ Tag = 'cpp_stateful'; Runner = 'cpp'; Dir = 'tests/e2e/cases/09-stateful/key_store'; File = 'key_store_disproved.cpp'; Expected = 'DISPROVED';
           Cry = 'key_store_spec.cry'; CryptolFn = 'key_store_activate_ret'; Function = 'key_store_activate';
           ExtraSpecGenArgs = @(
               '--out-buffer-param', 'ks=1',
               '--cryptol-fn-out',   'ks=key_store_activate_post'
           ) }
        @{ Tag = 'cpp_stateful'; Runner = 'cpp'; Dir = 'tests/e2e/cases/09-stateful/block'; File = 'block_verified.cpp';  Expected = 'VERIFIED';
           Cry = 'block_spec.cry'; CryptolFn = 'block_mask_ret'; Function = 'block_mask';
           ExtraSpecGenArgs = @(
               '--out-buffer-param', 'b=4',
               '--cryptol-fn-out',   'b=block_mask_post'
           ) }
        @{ Tag = 'cpp_stateful'; Runner = 'cpp'; Dir = 'tests/e2e/cases/09-stateful/block'; File = 'block_disproved.cpp'; Expected = 'DISPROVED';
           Cry = 'block_spec.cry'; CryptolFn = 'block_mask_ret'; Function = 'block_mask';
           ExtraSpecGenArgs = @(
               '--out-buffer-param', 'b=4',
               '--cryptol-fn-out',   'b=block_mask_post'
           ) }
        @{ Tag = 'cpp_stateful'; Runner = 'cpp'; Dir = 'tests/e2e/cases/09-stateful/session'; File = 'session_verified.cpp';  Expected = 'VERIFIED';
           Cry = 'session_spec.cry'; CryptolFn = 'session_open_ret'; Function = 'session_open';
           ExtraSpecGenArgs = @(
               '--out-buffer-param', 's=4',
               '--cryptol-fn-out',   's=session_open_post'
           ) }
        @{ Tag = 'cpp_stateful'; Runner = 'cpp'; Dir = 'tests/e2e/cases/09-stateful/session'; File = 'session_disproved.cpp'; Expected = 'DISPROVED';
           Cry = 'session_spec.cry'; CryptolFn = 'session_open_ret'; Function = 'session_open';
           ExtraSpecGenArgs = @(
               '--out-buffer-param', 's=4',
               '--cryptol-fn-out',   's=session_open_post'
           ) }
        @{ Tag = 'cpp_stateful'; Runner = 'cpp'; Dir = 'tests/e2e/cases/09-stateful/counter'; File = 'counter_verified.cpp';  Expected = 'VERIFIED';
           Cry = 'counter_spec.cry'; CryptolFn = 'counter_inc_ret'; Function = 'counter_inc';
           ExtraSpecGenArgs = @(
               '--out-buffer-param', 'c=i32',
               '--cryptol-fn-out',   'c=counter_inc_post'
           ) }
        @{ Tag = 'cpp_stateful'; Runner = 'cpp'; Dir = 'tests/e2e/cases/09-stateful/counter'; File = 'counter_disproved.cpp'; Expected = 'DISPROVED';
           Cry = 'counter_spec.cry'; CryptolFn = 'counter_inc_ret'; Function = 'counter_inc';
           ExtraSpecGenArgs = @(
               '--out-buffer-param', 'c=i32',
               '--cryptol-fn-out',   'c=counter_inc_post'
           ) }
        # enrollment : a named heterogeneous struct with padding — byte
        #             `engaged` at offset 0, then 7 pad bytes, then an
        #             aligned i64 `createdAt` at offset 8. The case uses
        #             `--out-buffer-param k=struct:EnrollmentKey`, which
        #             emits `llvm_alloc (llvm_struct "struct.EnrollmentKey")`
        #             so SAW keeps typed i8/i64 cells and leaves padding
        #             bytes unconstrained.
        @{ Tag = 'cpp_stateful'; Runner = 'cpp'; Dir = 'tests/e2e/cases/09-stateful/enrollment'; File = 'enrollment_verified.cpp';  Expected = 'VERIFIED';
           Cry = 'enrollment_spec.cry'; CryptolFn = 'enroll_key_ret'; Function = 'enroll_key';
           ExtraSpecGenArgs = @(
               '--out-buffer-param', 'k=struct:EnrollmentKey',
               '--cryptol-fn-out',   'k=enroll_key_post'
           ) }
        @{ Tag = 'cpp_stateful'; Runner = 'cpp'; Dir = 'tests/e2e/cases/09-stateful/enrollment'; File = 'enrollment_disproved.cpp'; Expected = 'DISPROVED';
           Cry = 'enrollment_spec.cry'; CryptolFn = 'enroll_key_ret'; Function = 'enroll_key';
           ExtraSpecGenArgs = @(
               '--out-buffer-param', 'k=struct:EnrollmentKey',
               '--cryptol-fn-out',   'k=enroll_key_post'
           ) }

        # ── Box allocator: currently UNKNOWN due to MIR allocator model gap
        # box_allocator currently produces UNKNOWN under the default pipeline
        # (Box::new path the front-end can't model). Tracked separately; not
        # run by default to keep the suite green. To enable, add tag 'box_allocator'.
        @{ Tag = 'box_allocator'; Runner = 'rust'; Dir = 'tests/e2e/cases/99-research/box_allocator';        File = 'add_one.rs'; Expected = 'UNKNOWN' }

        # ── Proof markers (saw-spec-gen-dtb) ────────────────────────────────
        # Verifies the BEGIN_PROOF / PROVED log contract end-to-end:
        # constructs a synthetic SAW log, runs Parse-PropertyLog.ps1,
        # and asserts the per-property result.json files match the
        # schema-1 shape.  Toolchain-free — no SAW / clang / rustc.
        @{ Tag = 'proof_markers'; Runner = 'custom'; Expected = 'VERIFIED';
           Script = 'tests/e2e/cases/10-proof-markers/Check-ProofMarkers.ps1';
           ScriptArgs = @{} }

        # ── Rust parity tests (saw-spec-gen-7fd / saw-spec-gen-5yl / saw-spec-gen-15t)
        # Toolchain-light: need rustc + llvm-dis but NOT SAW (script
        # generation only). These verify the gen-verify-rust CLI surface.

        # ── Rust cleanup funclet tests: exercise the exception-lower pass
        #    on Rust bitcode compiled with -C panic=unwind. Each has a Drop
        #    impl that produces cleanuppad/cleanupret funclets which must
        #    be lowered before SAW can parse the module.
        @{ Tag = 'rust_cleanup'; Runner = 'rust'; Dir = 'tests/e2e/cases/11-rust-parity/drop_noinline';    File = 'add_one_verified.rs'; Expected = 'VERIFIED' }
        @{ Tag = 'rust_cleanup'; Runner = 'rust'; Dir = 'tests/e2e/cases/11-rust-parity/drop_side_effect'; File = 'add_one_verified.rs'; Expected = 'VERIFIED' }
        @{ Tag = 'rust_cleanup'; Runner = 'rust'; Dir = 'tests/e2e/cases/11-rust-parity/cleanup_unwind';   File = 'add_one_verified.rs'; Expected = 'VERIFIED' }

        # spec-only-on-missing: gen-verify-rust soft-exits when the target
        # function has no matching symbol in the LLVM IR.
        @{ Tag = 'rust_parity'; Runner = 'custom'; Expected = 'VERIFIED';
           Script = 'tests/e2e/cases/11-rust-parity/spec_only_on_missing/Check-SpecOnlyOnMissing.ps1';
           ScriptArgs = @{} }

        # variant-map: membership precondition emitted in generated SAW script.
        @{ Tag = 'rust_parity'; Runner = 'custom'; Expected = 'VERIFIED';
           Script = 'tests/e2e/cases/11-rust-parity/variant_map/Check-VariantMap.ps1';
           ScriptArgs = @{} }

        # return narrowing: --variant-map return=... emits if/then/else adapter.
        @{ Tag = 'rust_parity'; Runner = 'custom'; Expected = 'VERIFIED';
           Script = 'tests/e2e/cases/11-rust-parity/return_narrowing/Check-ReturnNarrowing.ps1';
           ScriptArgs = @{} }

        # unified gen-verify: gen-verify --lang rust produces identical output
        # to gen-verify-rust.
        @{ Tag = 'rust_parity'; Runner = 'custom'; Expected = 'VERIFIED';
           Script = 'tests/e2e/cases/11-rust-parity/unified_gen_verify/Check-UnifiedGenVerify.ps1';
           ScriptArgs = @{} }

        # ── 12-aggregate-bridge ──────────────────────────────────────────────
        # Aggregate/struct ABI bridge tests. Each validates that the
        # generated SAW script contains the correct bridge construct
        # for non-scalar returns.

        # packed tuple return: StructValue bridge for { i1, i1 } aggregate
        @{ Tag = 'aggregate_bridge'; Runner = 'custom'; Expected = 'VERIFIED';
           Script = 'tests/e2e/cases/12-aggregate-bridge/packed_tuple_return/Check-PackedTupleReturn.ps1';
           ScriptArgs = @{} }

        # sret struct: sret byte-buffer allocation + llvm_points_to
        @{ Tag = 'aggregate_bridge'; Runner = 'custom'; Expected = 'VERIFIED';
           Script = 'tests/e2e/cases/12-aggregate-bridge/sret_preserved/Check-SretPreserved.ps1';
           ScriptArgs = @{} }

        # niche-packed enum: VariantRemap bridge + variant-map composition
        @{ Tag = 'aggregate_bridge'; Runner = 'custom'; Expected = 'VERIFIED';
           Script = 'tests/e2e/cases/12-aggregate-bridge/niche_enum_remap/Check-NicheEnumRemap.ps1';
           ScriptArgs = @{} }
    )
}
