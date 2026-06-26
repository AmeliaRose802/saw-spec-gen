/*
DEMO: String content verification — write_hello (verified).

`write_hello` writes the literal ASCII bytes of "hello" into the
caller-supplied 5-byte output buffer and returns 5.

The key point is that this test verifies *actual string content*:
the postcondition asserts the output buffer equals exactly
[0x68, 0x65, 0x6c, 0x6c, 0x6f] ("hello"), not just some symbolic
arbitrary value. SAW will reject any implementation that writes
different bytes.

The `_Out_writes_(5)` SAL annotation tells saw-spec-gen to:
  - allocate a 5-byte writable buffer for `buf`
  - look for a `buf_post` function in the Cryptol spec
  - auto-emit the output postcondition without any CLI flags

Expected RESULT: VERIFIED.
*/

#include <cstdint>
#include <cstddef>
#include "../../02-havoc-coverage/saw_sal.h"

std::uint32_t write_hello(_Out_writes_(5) std::uint8_t* buf) noexcept {
    buf[0] = 0x68; // 'h'
    buf[1] = 0x65; // 'e'
    buf[2] = 0x6c; // 'l'
    buf[3] = 0x6c; // 'l'
    buf[4] = 0x6f; // 'o'
    return 5;
}
