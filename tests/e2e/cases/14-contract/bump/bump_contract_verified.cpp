// VERIFIED case for the single-contract multi-effect model (docs/27).
//
// `bump` has TWO observable effects: it mutates `*out` and returns the
// old value. Instead of two separate Cryptol functions (one for the
// return, one for the out-buffer post-state), a SINGLE record-returning
// contract `bump : [32] -> { ret, outPost }` supplies both clauses:
//   - return    <- (bump out_pre).ret
//   - *out post <- (bump out_pre).outPost
//
// The model name matches the C++ symbol (`bump` ↔ `bump`), so linking
// and discovery need no out-of-band name map.

extern "C" int bump(int *out) {
    int old = *out;
    *out = old + 1;
    return old;
}
