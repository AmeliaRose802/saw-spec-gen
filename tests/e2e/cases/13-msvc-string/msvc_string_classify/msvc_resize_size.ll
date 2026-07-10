; msvc_resize_size.ll — regression test: MSVC-mangled basic_string methods
; must be classified by classify_basic_string_msvc and receive functional
; SAW overrides.  Also tests that is_basic_string_alias accepts the MSVC
; full-template struct name which contains "char_traits".
;
; Proves: resize(x+1); size() == x+1
;
; Three fixes in saw-spec-gen are exercised:
;   1. classify_basic_string_msvc — ??0/??1 (ctor/dtor) and ?resize@/?size@
;      are now matched and emit functional specs.
;   2. is_basic_string_alias — struct names with "char_traits" are no longer
;      excluded from layout discovery.
;   3. split_bracket_tokens / ReturnSetup — ensures the i64 return of ?size@
;      is parsed correctly (not confused with any [N x i8] pattern).
;
; Assembled with LLVM 20 (opaque-pointer mode).
target datalayout = "e-m:w-p270:32:32-p271:32:32-p272:64:64-i64:64-f80:128-n8:16:32:64-S128"
target triple = "x86_64-pc-windows-msvc19.29.30140"

; MSVC full-template struct name — previously rejected by is_basic_string_alias
; because it contains "char_traits".  The fix removes the char_traits guard.
; Layout: { data ptr, _Mysize (i64), _Myres (i64) }
%"class.std::basic_string<char,struct std::char_traits<char>,class std::allocator<char>>" = type { ptr, i64, i64 }

; MSVC-mangled basic_string declarations — exercised by classify_basic_string_msvc:
declare void @"??0?$basic_string@DU?$char_traits@D@std@@V?$allocator@D@2@@std@@QEAA@XZ"(ptr)
declare void @"??1?$basic_string@DU?$char_traits@D@std@@V?$allocator@D@2@@std@@QEAA@XZ"(ptr)
declare void @"?resize@?$basic_string@DU?$char_traits@D@std@@V?$allocator@D@2@@std@@QEAAX_K@Z"(ptr, i64)
declare i64 @"?size@?$basic_string@DU?$char_traits@D@std@@V?$allocator@D@2@@std@@QEBA_KXZ"(ptr)

; add_one: creates a string, resizes to x+1, returns size().
; With functional MSVC overrides SAW proves this equals x+1.
; Note: LLVM IR does not distinguish signed/unsigned at the type level;
; `i32` is the standard representation for C `unsigned int (uint32_t)`.
define i32 @add_one(i32 %x) {
entry:
  %str = alloca %"class.std::basic_string<char,struct std::char_traits<char>,class std::allocator<char>>"
  call void @"??0?$basic_string@DU?$char_traits@D@std@@V?$allocator@D@2@@std@@QEAA@XZ"(ptr %str)
  %x64 = zext i32 %x to i64
  %n = add i64 %x64, 1
  call void @"?resize@?$basic_string@DU?$char_traits@D@std@@V?$allocator@D@2@@std@@QEAAX_K@Z"(ptr %str, i64 %n)
  %sz = call i64 @"?size@?$basic_string@DU?$char_traits@D@std@@V?$allocator@D@2@@std@@QEBA_KXZ"(ptr %str)
  call void @"??1?$basic_string@DU?$char_traits@D@std@@V?$allocator@D@2@@std@@QEAA@XZ"(ptr %str)
  %r = trunc i64 %sz to i32
  ret i32 %r
}
