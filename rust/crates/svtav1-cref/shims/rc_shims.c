/*
 * C shims for the rate-control differential oracle (lane wp-ratecontrol,
 * `Codec/rc_process.c` + `Codec/pass2_strategy.c`).
 *
 * Most of this group's C surface is directly linkable (see
 * `src/rate_control.rs` — `svt_av1_rc_bits_per_mb`,
 * `svt_av1_get_cqp_kf_boost_from_r0`, `svt_av1_get_gfu_boost_from_r0_lap`,
 * `svt_av1_calculate_boost_bits` and every const table are plain exported
 * symbols bound with no wrapper at all). Only the functions that take an
 * SVT struct need a shim, and they are here.
 *
 * RULE, inherited from ref_shims.c and re-stated because it was paid for:
 * A SHIM MUST NOT KEEP PER-CALL STATE IN A `static`. cargo runs a test
 * binary's tests on several threads, so a `static` scratch struct is a data
 * race that fails as an occasional wrong NUMBER, not as a crash. Every
 * struct below is a STACK local, zero-initialised per call.
 */
#include <stdint.h>
#include <string.h>

#include "definitions.h"
#include "rc_process.h"

/* `svt_av1_compute_qdelta_by_rate` (rc_process.c:290) takes a RATE_CONTROL*
   but reads exactly two fields off it — `rc->best_quality` and
   `rc->worst_quality` (rc_process.c:314, as the binary search's bounds).
   Verified by reading the function body: nothing else on `rc` is touched, and
   the callee `find_qindex_by_rate` takes those two as plain ints. So a
   stack-allocated, zeroed RATE_CONTROL with those two set drives the REAL
   exported symbol on the real code path. */
int32_t ref_rc_compute_qdelta_by_rate(int32_t best_quality, int32_t worst_quality, int32_t frame_type, int32_t qindex,
                                      double rate_target_ratio, int32_t bit_depth, int32_t is_screen_content_type) {
    RATE_CONTROL rc;
    memset(&rc, 0, sizeof(rc));
    rc.best_quality  = best_quality;
    rc.worst_quality = worst_quality;
    return svt_av1_compute_qdelta_by_rate(
        &rc, (FrameType)frame_type, qindex, rate_target_ratio, bit_depth, is_screen_content_type);
}

/* `svt_av1_convert_qindex_to_q` (rc_process.c:185) is exported and takes only
   scalars, but its second argument is the `EbBitDepth` enum whose numeric
   values are 8/10/12 rather than 0/1/2; the shim pins the cast in C so the
   Rust binding cannot get the enum ABI wrong. */
double ref_rc_convert_qindex_to_q(int32_t qindex, int32_t bit_depth) {
    return svt_av1_convert_qindex_to_q(qindex, (EbBitDepth)bit_depth);
}
