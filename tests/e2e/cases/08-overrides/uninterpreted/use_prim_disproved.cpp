// DISPROVED case for the `@uninterpreted` primitive feature.
//
// Same setup as use_prim_verified.cpp, but the caller perturbs the
// primitive's result (`+ 1`). The Cryptol spec says `use_prim_spec x =
// prim x`, so the assumed contract for `prim` is non-vacuous: the
// postcondition rejects the off-by-one, proving the uninterpreted
// binding actually constrains the proof.

extern "C" unsigned char prim(unsigned char x);

extern "C" unsigned char use_prim(unsigned char x) {
    return (unsigned char)(prim(x) + 1);
}
