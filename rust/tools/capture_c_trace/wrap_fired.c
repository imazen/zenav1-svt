/* See wrap_fired.h. */
#include "wrap_fired.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

atomic_ulong zen_wrap_fired[ZEN_WRAP_COUNT];

static const char* const zen_wrap_names[ZEN_WRAP_COUNT] = {
#define ZEN_WRAP(sym) #sym,
#include "wrap_list.h"
#undef ZEN_WRAP
};

static void zen_wrap_report(void) {
    const char* where = getenv("SVT_WRAP_REPORT");
    if (!where || !*where) return;
    FILE* f = strcmp(where, "1") == 0 ? stderr : fopen(where, "a");
    if (!f) return;
    for (int i = 0; i < ZEN_WRAP_COUNT; i++)
        fprintf(f, "WRAP_FIRED\t%s\t%lu\n", zen_wrap_names[i],
                (unsigned long)atomic_load(&zen_wrap_fired[i]));
    if (f != stderr) fclose(f);
}

__attribute__((constructor)) static void zen_wrap_register(void) { atexit(zen_wrap_report); }
