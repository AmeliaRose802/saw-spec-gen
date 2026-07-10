; byte_array_return.ll — regression test: an extern returning [16 x i8] must
; emit a  llvm_fresh_var "rv" (llvm_array 16 (llvm_int 8))  override, not a
; missing llvm_return that caused a SAW type-mismatch:
;   "Incompatible types for return value: Expected: i64 but given [16 x i8]"
;
; Two fixes in saw-spec-gen are exercised:
;   1. split_bracket_tokens — [16 x i8] is kept as a single token instead of
;      being split into three fragments by split_whitespace.
;   2. ReturnSetup::ByteArray — ir_return_setup matches [N x i8] patterns and
;      emits llvm_array N (llvm_int 8) fresh vars in the override.
;
; Assembled with LLVM 20 (opaque-pointer mode).
target datalayout = "e-m:w-p270:32:32-p271:32:32-p272:64:64-i64:64-f80:128-n8:16:32:64-S128"
target triple = "x86_64-pc-windows-msvc19.29.30140"

; Opaque extern returning a 16-byte aggregate — MSVC ABI for small structs.
declare [16 x i8] @"?get_token@TokenProvider@@QEAA?AUToken@@XZ"()

; Function under test: calls the [16 x i8] extern (result discarded),
; then computes x+1.  gen-verify must handle the [16 x i8] return type
; when generating the override for the extern call.
define i32 @add_one(i32 %x) {
entry:
  %tok = call [16 x i8] @"?get_token@TokenProvider@@QEAA?AUToken@@XZ"()
  %r = add i32 %x, 1
  ret i32 %r
}
