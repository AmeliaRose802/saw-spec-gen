// VERIFIED case for compositional contract overrides (assume-guarantee).
//
// `double_it` is defined in a *different* translation unit, so it appears
// here only as a `declare` — the classic cross-TU call. With the
// `[functions.double_plus_one_spec].compose` config entry, its already-proven
// Cryptol contract (`double_it_spec x = x * 2`) is installed via
// `llvm_unsafe_assume_spec` instead of the default fresh-return/havoc extern
// model. The obligation then reduces to `x*2 + 1 == x*2 + 1`, so
// `double_plus_one` verifies.
//
// See docs/25-compositional-contract-overrides.md.

extern "C" int double_it(int x); // declared here, defined in another TU

extern "C" int double_plus_one(int x) {
    return double_it(x) + 1; // cross-TU call
}
