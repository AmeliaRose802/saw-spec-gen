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
        # SAW executes OkLog::log() inline — concrete body is safe -> VERIFIED.
        # SusLog::log() clobbers a global -> DISPROVED.
        @{ Tag = 'cpp_havoc'; Runner = 'cpp'; Dir = 'tests/e2e/cases/02-havoc-coverage/concrete_type_safe';         File = 'add_one_verified.cpp';                   Expected = 'VERIFIED' }
        @{ Tag = 'cpp_havoc'; Runner = 'cpp'; Dir = 'tests/e2e/cases/02-havoc-coverage/concrete_type_safe';         File = 'add_one_disproved.cpp';                  Expected = 'DISPROVED' }
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

        # ── Bitcode-driven extern override tests ────────────────────────────
        @{ Tag = 'cpp_overrides'; Runner = 'cpp'; Dir = 'tests/e2e/cases/08-overrides/bump';                    File = 'bump_verified.cpp';          Cry = 'bump_spec.cry';          CryptolFn = 'bump_spec';          Function = 'bump';          Expected = 'VERIFIED'  }
        @{ Tag = 'cpp_overrides'; Runner = 'cpp'; Dir = 'tests/e2e/cases/08-overrides/use_helper';              File = 'use_helper_verified.cpp';    Cry = 'use_helper_spec.cry';    CryptolFn = 'use_helper_spec';    Function = 'use_helper';    Expected = 'VERIFIED'  }
        @{ Tag = 'cpp_overrides'; Runner = 'cpp'; Dir = 'tests/e2e/cases/08-overrides/user_variadic';           File = 'bump_with_log_verified.cpp'; Cry = 'bump_with_log_spec.cry'; CryptolFn = 'bump_with_log_spec'; Function = 'bump_with_log'; Expected = 'VERIFIED'  }
        @{ Tag = 'cpp_overrides'; Runner = 'cpp'; Dir = 'tests/e2e/cases/08-overrides/variadic_clobber';        File = 'add_one_disproved.cpp';      Cry = 'add_one_spec.cry';       CryptolFn = 'add_one_spec';       Function = 'add_one_disproved';       Expected = 'DISPROVED' }
        @{ Tag = 'cpp_overrides'; Runner = 'cpp'; Dir = 'tests/e2e/cases/08-overrides/variadic_global_clobber'; File = 'add_one_disproved.cpp';      Cry = 'add_one_spec.cry';       CryptolFn = 'add_one_spec';       Function = 'add_one_disproved';       Expected = 'DISPROVED' }

        # ── C++/Rust equivalence tests (verify-equiv.ps1) ───────────────────
        @{ Tag = 'rust_equiv'; Runner = 'equiv'; Dir = 'tests/e2e/cases/04-cpp-rust-equivalence/compute_fee_reordered';         Cpp = 'compute_fee.cpp'; Rust = 'compute_fee_verified.rs';  Cry = 'compute_fee_spec.cry'; CryptolFn = 'compute_fee_spec'; Function = 'compute_fee'; Expected = 'EQUIVALENT'     }
        @{ Tag = 'rust_equiv'; Runner = 'equiv'; Dir = 'tests/e2e/cases/04-cpp-rust-equivalence/compute_fee_reordered';         Cpp = 'compute_fee.cpp'; Rust = 'compute_fee_disproved.rs'; Cry = 'compute_fee_spec.cry'; CryptolFn = 'compute_fee_spec'; Function = 'compute_fee'; Expected = 'NOT EQUIVALENT' }
        @{ Tag = 'rust_equiv'; Runner = 'equiv'; Dir = 'tests/e2e/cases/04-cpp-rust-equivalence/sat_add_optimized';   Cpp = 'sat_add.cpp';     Rust = 'sat_add_verified.rs';  Cry = 'sat_add_spec.cry';     CryptolFn = 'sat_add_spec';     Function = 'sat_add';     Expected = 'EQUIVALENT'     }
        @{ Tag = 'rust_equiv'; Runner = 'equiv'; Dir = 'tests/e2e/cases/04-cpp-rust-equivalence/sat_add_optimized';   Cpp = 'sat_add.cpp';     Rust = 'sat_add_disproved.rs'; Cry = 'sat_add_spec.cry';     CryptolFn = 'sat_add_spec';     Function = 'sat_add';     Expected = 'NOT EQUIVALENT' }
        @{ Tag = 'rust_equiv'; Runner = 'equiv'; Dir = 'tests/e2e/cases/04-cpp-rust-equivalence/not_operator_trap'; Cpp = 'negate.cpp';     Rust = 'negate.rs';           Cry = 'negate_spec.cry';      CryptolFn = 'negate_spec';      Function = 'negate';      Expected = 'NOT EQUIVALENT' }

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
        @{ Tag = 'rust_adversarial'; Runner = 'rust'; Dir = 'tests/e2e/cases/99-research/rust_adversarial/drop_noinline';           File = 'add_one_verified.rs';  Expected = 'VERIFIED'  }
        @{ Tag = 'rust_adversarial'; Runner = 'rust'; Dir = 'tests/e2e/cases/99-research/rust_adversarial/drop_side_effect';        File = 'add_one_verified.rs';  Expected = 'VERIFIED'  }
        @{ Tag = 'rust_adversarial'; Runner = 'rust'; Dir = 'tests/e2e/cases/99-research/rust_adversarial/raw_pointer_aliasing';    File = 'add_one_disproved.rs'; Expected = 'DISPROVED' }
        @{ Tag = 'rust_adversarial'; Runner = 'rust'; Dir = 'tests/e2e/cases/99-research/rust_adversarial/symbol_collision';        File = 'add_one_disproved.rs'; Expected = 'DISPROVED' }
        @{ Tag = 'rust_adversarial'; Runner = 'rust'; Dir = 'tests/e2e/cases/99-research/rust_adversarial/unreachable_unchecked';   File = 'add_one_disproved.rs'; Expected = 'DISPROVED' }
        # box_allocator currently produces UNKNOWN under the default pipeline
        # (Box::new path the front-end can't model). Tracked separately; not
        # run by default to keep the suite green. To enable, add tag 'box_allocator'.
        @{ Tag = 'box_allocator'; Runner = 'rust'; Dir = 'tests/e2e/cases/99-research/box_allocator';        File = 'add_one.rs'; Expected = 'UNKNOWN' }
    )
}
