// VERIFIED case for AUTOMATIC compositional contract overrides.
//
// `double_it` is defined in a *different* translation unit, so it appears
// here only as a `declare` — the classic cross-TU call. saw-spec-gen
// discovers automatically (no config) that the Cryptol spec defines a
// contract `double_it_spec` for the callee symbol `double_it`, and
// installs it via `llvm_unsafe_assume_spec` instead of the default
// fresh-return/havoc extern model. The obligation then reduces to
// `x*2 + 1 == x*2 + 1`, so `double_plus_one` verifies.
//
// This case only proves BECAUSE the contract is auto-composed: with the
// default havoc model `double_it(x)` returns an arbitrary value and the
// proof would fail. See docs/25-compositional-contract-overrides.md.

extern "C" int double_it(int x); // declared here, defined in another TU

extern "C" int double_plus_one(int x) {
    return double_it(x) + 1; // cross-TU call
}
