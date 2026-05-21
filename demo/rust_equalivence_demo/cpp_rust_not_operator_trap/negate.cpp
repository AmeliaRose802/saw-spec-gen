// Two's complement negation for 32-bit unsigned ints.
//
// Classic identity:  -x  ==  (~x) + 1
//
// This function is supposed to compute that.
// (Same formula and same source text as the Rust file next door.)

unsigned int negate(unsigned int x) {
    return !x + 1;
}
