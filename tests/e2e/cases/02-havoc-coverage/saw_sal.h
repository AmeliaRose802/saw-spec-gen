// SAW-friendly SAL annotation shim.
//
// Under MSVC, `<sal.h>` expands `_In_`, `_Out_`, `_Inout_`, etc. to nothing
// (or to non-portable __declspec attributes) so clang's JSON AST dump does
// NOT carry the annotations into `AnnotateAttr` nodes. saw-spec-gen reads
// those `AnnotateAttr` nodes to derive havoc behaviour for pointer
// parameters; without them every pointer is conservatively havoced, and the
// generator cannot distinguish an `_Out_` buffer from an arbitrary mutable
// pointer.
//
// This header redefines the common SAL macros to `__attribute__((annotate(...)))`
// so clang emits an `AnnotateAttr` carrying the macro name. Include this
// instead of (or after undefining) `<sal.h>` whenever you build a demo for
// SAW verification.
//
// Sized variants (`_In_reads_(N)`, `_Out_writes_(N)`) carry their integer
// argument through the preprocessor via the `#` stringization operator:
//
//     _In_reads_(8)
//   → __attribute__((annotate("_In_reads_(" "8" ")")))
//   → __attribute__((annotate("_In_reads_(8)")))   (after string concat)
//
// which is exactly the form `src/clang_ast/sal.rs::classify` matches.
#pragma once
#ifndef SAW_SAL_H_INCLUDED
#define SAW_SAL_H_INCLUDED

// Undefine any prior MSVC-style definitions so our redefinitions stick.
#ifdef _In_
#undef _In_
#endif
#ifdef _In_opt_
#undef _In_opt_
#endif
#ifdef _Out_
#undef _Out_
#endif
#ifdef _Out_opt_
#undef _Out_opt_
#endif
#ifdef _Inout_
#undef _Inout_
#endif
#ifdef _Inout_opt_
#undef _Inout_opt_
#endif
#ifdef _In_reads_
#undef _In_reads_
#endif
#ifdef _In_reads_bytes_
#undef _In_reads_bytes_
#endif
#ifdef _Out_writes_
#undef _Out_writes_
#endif
#ifdef _Out_writes_bytes_
#undef _Out_writes_bytes_
#endif

#define _In_         __attribute__((annotate("_In_")))
#define _In_opt_     __attribute__((annotate("_In_opt_")))
#define _Out_        __attribute__((annotate("_Out_")))
#define _Out_opt_    __attribute__((annotate("_Out_opt_")))
#define _Inout_      __attribute__((annotate("_Inout_")))
#define _Inout_opt_  __attribute__((annotate("_Inout_opt_")))

// Sized variants: the `#n` stringization turns the integer literal
// into a string fragment that's concatenated with the surrounding
// pieces at translation time.
#define _In_reads_(n)         __attribute__((annotate("_In_reads_(" #n ")")))
#define _In_reads_bytes_(n)   __attribute__((annotate("_In_reads_bytes_(" #n ")")))
#define _Out_writes_(n)       __attribute__((annotate("_Out_writes_(" #n ")")))
#define _Out_writes_bytes_(n) __attribute__((annotate("_Out_writes_bytes_(" #n ")")))

#endif // SAW_SAL_H_INCLUDED
