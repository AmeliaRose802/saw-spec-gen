/*
DEMO: String content verification — write_hello (disproved).

Same function signature as write_hello_verified.cpp, but this version
writes "world" instead of "hello". The Cryptol postcondition checks for
exactly "hello" ([0x68, 0x65, 0x6c, 0x6c, 0x6f]), so SAW will find a
counterexample: the pre-state of buf doesn't matter, and for every
input the output diverges from the expected string.

The `_Out_writes_(5)` SAL annotation triggers auto-postcondition from
`buf_post` in the Cryptol spec — no CLI flags needed.

Expected RESULT: DISPROVED.
*/

#include <cstdint>
#include <cstddef>
#include "../../02-havoc-coverage/saw_sal.h"

std::uint32_t write_hello(_Out_writes_(5) std::uint8_t* buf) noexcept {
    buf[0] = 0x77; // 'w'  ← wrong: should be 'h'
    buf[1] = 0x6f; // 'o'  ← wrong: should be 'e'
    buf[2] = 0x72; // 'r'  ← wrong: should be 'l'
    buf[3] = 0x6c; // 'l'
    buf[4] = 0x64; // 'd'  ← wrong: should be 'o'
    return 5;
}
