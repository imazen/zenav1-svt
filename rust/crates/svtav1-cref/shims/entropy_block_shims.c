/*
 * C shims for the PER-BLOCK bitstream-emission differential — the pieces of
 * `write_modes_b` / `svt_aom_write_modes_sb` (Codec/entropy_coding.c) that the
 * wx-entropy lane ports.
 *
 * Its own translation unit, like entropy_inter_shims.c beside it, so this lane
 * never shares an editable file with a concurrent lane.
 *
 * TWO evidence tiers live here and the entry points say which they are:
 *
 *  - tier 1 (`docs/WORKING-ON-THIS.md` §4): a call into a REAL exported symbol,
 *    i.e. into the RELEASE ARCHIVE's compiled code.
 *      svt_aom_is_masked_compound_type      inter_prediction.c:34
 *      svt_aom_get_wedge_params_bits        inter_prediction.c:2053
 *      svt_aom_get_wedge_bits_lookup        inter_prediction.c:2019
 *      svt_aom_is_interintra_wedge_used     inter_prediction.c:2015
 *
 *  - tier 1-header: `static INLINE` in a HEADER, so there is no symbol to call.
 *    This TU compiles the header's own source text, which is the C source but
 *    NOT the archive's copy of it — a compiler-flag difference between this TU
 *    and the archive could in principle diverge (see WORKING-ON-THIS §5 trap 3,
 *    `-DNDEBUG` as an ABI flag). Stronger than a transcription, weaker than a
 *    call into the archive; labelled as such in the test.
 *      is_interinter_compound_used          inter_prediction.h:288
 *      is_any_masked_compound_used          inter_prediction.h:303
 *      block_signals_txsize                 entropy_coding.c:4418 (see below)
 *
 * Shim state is per-call and on the stack (never a `static` buffer): cargo runs
 * a test binary's tests on several threads.
 */
#include <stddef.h>
#include <stdint.h>
#include <string.h>

#include "definitions.h"
#include "EbSvtAv1.h"
#include "block_structures.h"
#include "common_utils.h"
#include "inter_prediction.h"

/* ---- the exported declarations this TU drives ---- */
int     svt_aom_is_masked_compound_type(COMPOUND_TYPE type);
int     svt_aom_get_wedge_params_bits(BlockSize bsize);
int32_t svt_aom_get_wedge_bits_lookup(BlockSize bsize);
int     svt_aom_is_interintra_wedge_used(BlockSize bsize);

/* ---------------------------------------------------------------------------
 * Compound-type predicates (write_modes_b step 9 / step 7 gates).
 * ------------------------------------------------------------------------ */

/* tier 1 — exported. */
int ref_eb_is_masked_compound_type(int comp_type) {
    return svt_aom_is_masked_compound_type((COMPOUND_TYPE)comp_type);
}

/* tier 1 — exported. */
int ref_eb_wedge_params_bits(int bsize) { return svt_aom_get_wedge_params_bits((BlockSize)bsize); }

/* tier 1 — exported. */
int ref_eb_wedge_bits_lookup(int bsize) {
    return (int)svt_aom_get_wedge_bits_lookup((BlockSize)bsize);
}

/* tier 1 — exported. */
int ref_eb_is_interintra_wedge_used(int bsize) {
    return svt_aom_is_interintra_wedge_used((BlockSize)bsize);
}

/* tier 1-header — `static INLINE` in inter_prediction.h:284. */
int ref_eb_is_comp_ref_allowed(int bsize) { return (int)is_comp_ref_allowed((BlockSize)bsize); }

/* tier 1-header — `static INLINE` in inter_prediction.h:288. */
int ref_eb_is_interinter_compound_used(int comp_type, int bsize) {
    return is_interinter_compound_used((CompoundType)comp_type, (BlockSize)bsize);
}

/* tier 1-header — `static INLINE` in inter_prediction.h:303. */
int ref_eb_is_any_masked_compound_used(int bsize) {
    return is_any_masked_compound_used((BlockSize)bsize);
}
