/*
EXPERIMENT: is the bare-`void*` → `llvm_int 64` lowering still sound
when one side of the comparison is a typed pointer cast to `void*`?

Setup: caller allocates a `Widget`, passes its address both as a typed
`const Widget*` and (separately) as a `const void*`. Function returns
1 iff the void* operand is the same address as the typed-pointer
operand.

What the harness does with each parameter:
    w  : `const Widget*` → SAW-allocates `sizeof(Widget) = 8` bytes,
         passes the *symbolic typed pointer* (NOT its numeric address)
         as the first argument.
    q  : `const void*`    → fresh `[64]` symbolic value (bug #15 path),
         passed via `llvm_term q` (a raw integer coerced to a pointer
         at the LLVM ABI).

At the C++ level the body computes `(void*)w == q`, which lowers to
`ptrtoint w` compared against the i64 in `q`. SAW's pointer model
does not expose the numeric address of `w` to the solver in any form
the Cryptol spec can constrain, so:

  - A "constant 0" spec is DISPROVED: the solver picks a `q` that
    happens to equal the symbolic address of `w`.
  - A "constant 1" spec is DISPROVED: q can also be any *other* value.

No total Cryptol spec exists for this function under the current
lowering. The test below pins that observation — Expected = DISPROVED
no matter what trivial spec we plug in.
*/

#include <cstdint>

struct Widget {
    uint32_t a;
    uint32_t b;
};

uint32_t widget_addr_eq(const Widget* w, const void* q) {
    return static_cast<const void*>(w) == q ? 1u : 0u;
}
