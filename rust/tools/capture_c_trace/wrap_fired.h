/* Fire counts for the --wrap interposers (plan 2.3). Each __wrap_* function
 * starts with ZEN_WRAP_FIRED(sym). With SVT_WRAP_REPORT set, the driver prints
 * one line per interposer at exit, zeros included:
 *   WRAP_FIRED\t<symbol>\t<count>
 * to stderr (SVT_WRAP_REPORT=1) or appended to the named file. A gate that
 * relies on an interposer can then assert it fired, instead of silently
 * measuring nothing when an upstream refactor routes around the symbol. */
#ifndef ZEN_WRAP_FIRED_H
#define ZEN_WRAP_FIRED_H
#include <stdatomic.h>

enum {
#define ZEN_WRAP(sym) ZEN_WRAP_ID_##sym,
#include "wrap_list.h"
#undef ZEN_WRAP
    ZEN_WRAP_COUNT
};

extern atomic_ulong zen_wrap_fired[ZEN_WRAP_COUNT];

#define ZEN_WRAP_FIRED(sym) \
    atomic_fetch_add_explicit(&zen_wrap_fired[ZEN_WRAP_ID_##sym], 1, memory_order_relaxed)

#endif
