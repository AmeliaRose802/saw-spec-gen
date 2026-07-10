/*
DEMO: sret pre-state slice — struct return via hidden sret pointer.

On x86_64 System V ABI (Linux) aggregates > 16 bytes are returned
via a hidden `sret` output pointer.  The struct must exceed 16 bytes
to force sret on Linux; 20 bytes (4 header + 16 body) clears the
threshold on all supported ABIs.

The Cryptol model takes a trailing [16][8] pre-state parameter
representing the `body` field (at offset 4), while the full struct
is 20 bytes.  saw-spec-gen must:

  1. Allocate preBytes at the FULL buffer size (20 bytes).
  2. Pass `(take`{16} (drop`{4} preBytes))` — not the raw preBytes —
     as the trailing Cryptol argument.

Expected verdict: VERIFIED  (the spec ignores the pre-state param
since the C++ writes every byte deterministically).
*/

#include <cstdint>

struct TwoFields {
    unsigned char header[4];
    unsigned char body[16];
};

TwoFields stamp_header(unsigned char v) {
    TwoFields r;
    r.header[0] = v;
    r.header[1] = v;
    r.header[2] = v;
    r.header[3] = v;
    r.body[0]  = 0; r.body[1]  = 0; r.body[2]  = 0;
    r.body[3]  = 0; r.body[4]  = 0; r.body[5]  = 0;
    r.body[6]  = 0; r.body[7]  = 0; r.body[8]  = 0;
    r.body[9]  = 0; r.body[10] = 0; r.body[11] = 0;
    r.body[12] = 0; r.body[13] = 0; r.body[14] = 0;
    r.body[15] = 0;
    return r;
}
