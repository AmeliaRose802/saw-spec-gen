// Smoke test #2: target calls a system-library function (printf).
// MUST verify — the std-library call should be auto-stubbed so SAW
// never has to symbolically execute MSVC's <cstdio> inline machinery.
//
// This is the "better story for external std things" case: even though
// printf in MSVC <cstdio> is technically an inline function with a body
// (it calls _vfprintf_l etc.), the auto-stubber recognises it as a
// system function and emits an adversarial override that returns a
// fresh symbolic int, so the side effect on stdout is irrelevant.
#include <cstdint>
#include <cstdio>

uint32_t bump(uint32_t x) {
    printf("bumping %u\n", x);  // side effect we don't care about
    return x + 1;
}
