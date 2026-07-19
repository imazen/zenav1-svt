/*
 * wrap_odec.c — ld --wrap interceptors for the SVT-AV1 arithmetic coder.
 *
 * build.sh links the driver with
 *   -Wl,--wrap=svt_od_ec_encode_cdf_q15
 *   -Wl,--wrap=svt_od_ec_encode_bool_q15
 *   -Wl,--wrap=svt_od_ec_encode_bool_eq_q15
 *   -Wl,--wrap=svt_od_ec_enc_init
 *   -Wl,--wrap=svt_od_ec_enc_reset
 *   -Wl,--wrap=svt_od_ec_enc_done
 * so every cross-TU call from the library's bitstream writer lands here
 * first, gets appended to the $SVT_TRACE_OUT text file, then forwarded to
 * the real implementation. This is the COMPLETE od_ec encode surface of
 * bitstream_unit.h (v4.2): aom_write_bit -> bool_eq, aom_write_symbol
 * nsyms==2 -> bool (f = cdf[0]), nsyms>2 -> cdf. Header bits
 * (AomWriteBitBuffer) never touch od_ec and are compared byte-wise.
 *
 * Record format (one line per op, matching the Rust `symtrace` feature):
 *   W CDF nsyms=<n> s=<s> icdf=[<i0>,<i1>,<i2>] rng=<rng>
 *   W BOOL val=<v> f=<f> rng=<rng>
 *   W BOOLEQ val=<v> rng=<rng>          (C only; == BOOL f=16384 arithmetic)
 *   W INIT ec=<ptr>
 *   W RESET ec=<ptr>
 *   W DONE nbytes=<n> ec=<ptr>
 * `rng` is the coder range BEFORE the op — a state checksum: if two traces
 * agree on every op tuple, rng must agree too unless an op escaped tracing.
 *
 * If $SVT_TRACE_OUT is unset the wrappers are pure pass-through.
 */
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

/* Full struct layout (need enc->rng): same include set as svtav1-cref. */
#include "bitstream_unit.h"

void           __real_svt_od_ec_encode_cdf_q15(OdEcEnc* enc, int32_t s, const uint16_t* icdf, int32_t nsyms);
void           __real_svt_od_ec_encode_bool_q15(OdEcEnc* enc, int32_t val, unsigned f_q15);
void           __real_svt_od_ec_encode_bool_eq_q15(OdEcEnc* enc, int32_t val);
void           __real_svt_od_ec_enc_init(OdEcEnc* enc);
void           __real_svt_od_ec_enc_reset(OdEcEnc* enc);
unsigned char* __real_svt_od_ec_enc_done(OdEcEnc* enc, uint32_t* nbytes);

static FILE*          trace_fp   = NULL;
static pthread_once_t trace_once = PTHREAD_ONCE_INIT;

static void trace_open(void) {
    const char* path = getenv("SVT_TRACE_OUT");
    if (path && *path)
        trace_fp = fopen(path, "w");
}

static FILE* tf(void) {
    pthread_once(&trace_once, trace_open);
    return trace_fp;
}

void __wrap_svt_od_ec_encode_cdf_q15(OdEcEnc* enc, int32_t s, const uint16_t* icdf, int32_t nsyms) {
    FILE* f = tf();
    if (f) {
        /* Mirror the Rust print: first three icdf entries, 0-padded.
           (For nsyms >= 3 entries 0..2 all exist: nsyms-1 icdf values + 0 sentinel.) */
        unsigned i0 = icdf[0];
        unsigned i1 = nsyms >= 2 ? icdf[1] : 0;
        unsigned i2 = nsyms >= 3 ? icdf[2] : 0;
        fprintf(f, "W CDF nsyms=%d s=%d icdf=[%u,%u,%u] rng=%u\n", nsyms, s, i0, i1, i2, enc->rng);
    }
    __real_svt_od_ec_encode_cdf_q15(enc, s, icdf, nsyms);
}

void __wrap_svt_od_ec_encode_bool_q15(OdEcEnc* enc, int32_t val, unsigned f_q15) {
    FILE* f = tf();
    if (f)
        fprintf(f, "W BOOL val=%d f=%u rng=%u\n", val, f_q15, enc->rng);
    __real_svt_od_ec_encode_bool_q15(enc, val, f_q15);
}

void __wrap_svt_od_ec_encode_bool_eq_q15(OdEcEnc* enc, int32_t val) {
    FILE* f = tf();
    if (f)
        fprintf(f, "W BOOLEQ val=%d rng=%u\n", val, enc->rng);
    __real_svt_od_ec_encode_bool_eq_q15(enc, val);
}

void __wrap_svt_od_ec_enc_init(OdEcEnc* enc) {
    FILE* f = tf();
    if (f)
        fprintf(f, "W INIT ec=%p\n", (void*)enc);
    __real_svt_od_ec_enc_init(enc);
}

void __wrap_svt_od_ec_enc_reset(OdEcEnc* enc) {
    FILE* f = tf();
    if (f)
        fprintf(f, "W RESET ec=%p\n", (void*)enc);
    __real_svt_od_ec_enc_reset(enc);
}

unsigned char* __wrap_svt_od_ec_enc_done(OdEcEnc* enc, uint32_t* nbytes) {
    unsigned char* ret = __real_svt_od_ec_enc_done(enc, nbytes);
    FILE*          f   = tf();
    if (f) {
        fprintf(f, "W DONE nbytes=%u ec=%p\n", nbytes ? *nbytes : 0, (void*)enc);
        fflush(f);
    }
    return ret;
}
