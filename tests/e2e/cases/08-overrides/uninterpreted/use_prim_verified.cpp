// VERIFIED case for the `@uninterpreted` primitive feature.
//
// `prim` is an opaque external primitive — there is no body in this
// translation unit, so SAW sees only a declaration. The verify script
// binds it to the Cryptol model `prim` via `llvm_unsafe_assume_spec`
// (emitted from the `@uninterpreted` annotation in use_prim_spec.cry).
//
// `use_prim` simply forwards to the primitive, so it is extensionally
// equal to `use_prim_spec x = prim x`.

extern "C" unsigned char prim(unsigned char x);

extern "C" unsigned char use_prim(unsigned char x) {
    return prim(x);
}
