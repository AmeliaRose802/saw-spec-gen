// DISPROVED case for AUTOMATIC compositional contract overrides.
//
// `double_it`'s contract `double_it_spec` is auto-composed (x*2), so the
// callee is modeled faithfully — but this caller is wrong: it adds 2
// instead of 1. The obligation `x*2 + 2 == x*2 + 1` is unsatisfiable, so
// the proof is disproved with a counterexample. This confirms the
// auto-composed contract is applied (the callee is NOT havoc'd away) yet
// the check is non-vacuous. See docs/25-compositional-contract-overrides.md.

extern "C" int double_it(int x); // declared here, defined in another TU

extern "C" int double_plus_one(int x) {
    return double_it(x) + 2; // BUG: spec says +1
}
