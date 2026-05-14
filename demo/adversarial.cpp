#include "adversarial.h"

// process() is declared but NOT defined — truly unknown implementation
// SAW will model it as adversarial: can modify anything reachable

// get_counter_after_process: UNSAFE — reads ctx->counter after process() may have trashed it
// A proof that counter is preserved should FAIL

// get_flags_then_process: SAFE — saves flags before the call
// A proof that the return value equals original flags should SUCCEED
