// DEMO: BUGGY max of two i16 values — returns the smaller. SAW disproves.

fn max_i16(a: i16, b: i16) -> i16 {
    if a < b { a } else { b }   // BUG: was a > b.
}
