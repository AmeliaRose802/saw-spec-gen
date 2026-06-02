// Two's complement negation for 32-bit unsigned ints.
//
// Classic identity:  -x  ==  (~x) + 1
//
// This function is supposed to compute that.
// (Same formula and same source text as the C++ file next door.)

fn negate(x: u32) -> u32 {
    !x + 1
}
