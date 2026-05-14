#include "deep_havoc.h"

// Force the compiler to emit the class definitions
void force_emit(const IProcessor *p, Config *cfg) {
    p->check(cfg);
}
