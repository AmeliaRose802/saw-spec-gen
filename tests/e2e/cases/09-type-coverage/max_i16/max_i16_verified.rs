// DEMO: max of two i16 values (Rust mirror of max_i16_verified.cpp).
// Exercises i16 parameter + return + signed-compare, absent from earlier suites.

fn max_i16(a: i16, b: i16) -> i16 {
    if a > b { a } else { b }
}
