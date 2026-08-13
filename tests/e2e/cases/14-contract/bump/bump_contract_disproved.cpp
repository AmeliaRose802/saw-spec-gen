// DISPROVED case for the single-contract multi-effect model (docs/27).
//
// The return clause still matches (`old`), but the out-buffer post-state
// is wrong: this writes `old + 2` while the contract's `outPost` field
// says `pre + 1`. The `ensures out = outPost` clause is therefore
// checked and fails — confirming the heap postcondition is not defaulted
// away when the single contract carries multiple effects.

extern "C" int bump(int *out) {
    int old = *out;
    *out = old + 2; // BUG: contract outPost says +1
    return old;
}
