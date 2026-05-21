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
// Only the macros saw-spec-gen currently understands are listed here. Sized
// variants (`_In_reads_(n)`, `_Out_writes_(n)`) are *not* covered because
// the size argument can't pass through the annotation string portably; use
// the unsized forms in demos.
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

#define _In_         __attribute__((annotate("_In_")))
#define _In_opt_     __attribute__((annotate("_In_opt_")))
#define _Out_        __attribute__((annotate("_Out_")))
#define _Out_opt_    __attribute__((annotate("_Out_opt_")))
#define _Inout_      __attribute__((annotate("_Inout_")))
#define _Inout_opt_  __attribute__((annotate("_Inout_opt_")))

#endif // SAW_SAL_H_INCLUDED
