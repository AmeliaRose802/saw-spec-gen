// DISPROVED case for compositional contract overrides.
//
// Identical source to the verified case, but *no* `compose` config is
// supplied. `double_it` is therefore modeled as an unspecified extern
// (fresh symbolic return + havoc frame), so `double_plus_one(x)` returns
// `v + 1` for an arbitrary `v`. The spec `double_plus_one_spec x = x*2 + 1`
// cannot be met for all `v` (counterexample `v != x*2`), so the proof is
// disproved. This pins that B's correctness is recoverable *only* by
// composing A's proven contract; the havoc model is provably insufficient.
//
// See docs/25-compositional-contract-overrides.md.

extern "C" int double_it(int x); // declared here, defined in another TU

extern "C" int double_plus_one(int x) {
    return double_it(x) + 1; // cross-TU call
}
