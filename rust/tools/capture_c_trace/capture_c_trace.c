/*
 * capture_c_trace — minimal public-API driver for the in-tree C SVT-AV1
 * static library, used by the bitstream-identity harness
 * (rust/tools/identity_diff.sh).
 *
 * Encodes exactly ONE raw I420 8-bit frame from a .yuv file in
 * still-picture/AVIF CQP mode at a matched config (the same knob set the
 * repo's perf/parity gates use for SvtAv1EncApp: --rc 0 --aq-mode 0
 * --qp Q --avif 1 --lp 1 -n 1) and writes the raw OBU stream (concatenated
 * output-packet payloads: TD + SH + Frame OBU) to the output path.
 *
 * When linked with tools/capture_c_trace/build.sh, every arithmetic-coder
 * operation the library performs is intercepted via -Wl,--wrap= and logged
 * to the file named by $SVT_TRACE_OUT (see wrap_odec.c). Header bits
 * (sequence/frame header) go through the AomWriteBitBuffer path, NOT the
 * od_ec coder — those are compared at the byte level by identity_diff.py.
 *
 * Usage: capture_c_trace <width> <height> <cli_qp 0..63> <preset> <in.yuv> <out.obu>
 * Env: SVT_TILE_ROWS (default: unset -> library default, 0 tile rows) —
 *      direct passthrough to cfg.tile_rows, i.e. TileRowsLog2 (task #86;
 *      same log2 units as the Rust driver's SVTAV1_TILE_ROWS_LOG2 —
 *      EbSvtAv1Enc.h:607-611 documents the field as "Log 2 Tile Rows").
 *
 *      SVT_HDR_MODE=1 selects the FORK oracle. That is a BUILD-TIME switch,
 *      handled by build.sh/the wrapper (different lib + different binary);
 *      this file is compiled unchanged for both. What it does add is the
 *      SVT_FORK_* knob passthrough below.
 *
 *      SVT_FORK_<FIELD> (all optional): explicit override for a fork /
 *      fork-defaulted config field. ABSENT means "leave at whatever
 *      svt_av1_enc_init_handle loaded", so with none set this driver is
 *      byte-for-byte its previous self in mainline mode, and in fork mode it
 *      reproduces the MODE1 library defaults. Needed because the fork's
 *      feature knobs (ac_bias, sharp_tx, noise_norm_strength, ...) are NOT
 *      inside `#if SVT_HDR_MODE` in enc_settings.c — they are neutralized
 *      unconditionally (enc_settings.c:1181-1203) — so MODE1-by-default is
 *      only the fork's UNCONDITIONAL deltas plus the six defaults the fork
 *      does flip (bit depth, preset, QM on 6..10, variance boost on,
 *      tf_strength 1, sharpness 1). Exercising a fork FEATURE against the C
 *      oracle requires setting it here and to the identical value on the Rust
 *      side. Names match the EbSvtAv1EncConfiguration fields, upper-cased:
 *      SVT_FORK_AC_BIAS, SVT_FORK_SHARP_TX, SVT_FORK_TX_BIAS,
 *      SVT_FORK_COMPLEX_HVS, SVT_FORK_NOISE_NORM_STRENGTH,
 *      SVT_FORK_ALT_LAMBDA_FACTORS, SVT_FORK_ALT_SSIM_TUNING, SVT_FORK_TUNE,
 *      SVT_FORK_CDEF_SCALING, SVT_FORK_NOISE_ADAPTIVE_FILTERING,
 *      SVT_FORK_NOISE_STRENGTH, SVT_FORK_NOISE_STRENGTH_CHROMA,
 *      SVT_FORK_NOISE_CHROMA_FROM_LUMA, SVT_FORK_NOISE_SIZE,
 *      SVT_FORK_KF_TF_STRENGTH, SVT_FORK_TF_STRENGTH, SVT_FORK_SHARPNESS,
 *      SVT_FORK_QP_SCALE_COMPRESS_STRENGTH, SVT_FORK_ENABLE_QM,
 *      SVT_FORK_MIN_QM_LEVEL, SVT_FORK_MAX_QM_LEVEL,
 *      SVT_FORK_MIN_CHROMA_QM_LEVEL, SVT_FORK_MAX_CHROMA_QM_LEVEL,
 *      SVT_FORK_ENABLE_VARIANCE_BOOST, SVT_FORK_VARIANCE_BOOST_STRENGTH,
 *      SVT_FORK_VARIANCE_OCTILE, SVT_FORK_VARIANCE_BOOST_CURVE,
 *      SVT_FORK_HBD_MDS.
 *
 * NOT part of the cargo workspace build — compiled on demand by build.sh.
 */
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "EbSvtAv1.h"
#include "EbSvtAv1Enc.h"

static void die(const char* msg, int32_t err) {
    fprintf(stderr, "capture_c_trace: %s (err=0x%x)\n", msg, (unsigned)err);
    exit(1);
}

/* ---- SVT_FORK_* knob passthrough (see the header comment) ---------------- *
 * Each setter is a no-op when its env var is absent, so the config the library
 * loaded stays untouched. Applied AFTER the still-picture/CQP config block and
 * BEFORE svt_av1_enc_set_parameter, and echoed to stderr so a gate log records
 * exactly which fork config produced the bytes. */

/* Every override is echoed; the caller's log is then self-describing.
 * The env NAME is spelled out rather than stringified from `field`, so the
 * documented upper-case spelling is what is actually looked up (a `#field`
 * form silently searches the lower-case name and every override is ignored —
 * which is both silent and indistinguishable from "the knob has no effect"). */
#define FORK_SET(envname, field, conv)                                           \
    do {                                                                         \
        const char* _v = getenv(envname);                                        \
        if (_v) {                                                                \
            cfg.field = conv(_v);                                                \
            fprintf(stderr, "capture_c_trace: fork knob %s=%s\n", envname, _v);  \
        }                                                                        \
    } while (0)

#define FORK_I(s)   ((int)atoi(s))
#define FORK_U8(s)  ((uint8_t)atoi(s))
#define FORK_I8(s)  ((int8_t)atoi(s))
#define FORK_I32(s) ((int32_t)atoi(s))
#define FORK_B(s)   ((bool)(atoi(s) != 0))
#define FORK_D(s)   (atof(s))

int main(int argc, char** argv) {
    if (argc != 7 && argc != 8) {
        fprintf(stderr,
                "usage: %s <width> <height> <cli_qp 0..63> <preset> <in.yuv> <out.obu> [bit_depth=8|10]\n",
                argv[0]);
        return 2;
    }
    const uint32_t w      = (uint32_t)atoi(argv[1]);
    const uint32_t h      = (uint32_t)atoi(argv[2]);
    const uint32_t qp     = (uint32_t)atoi(argv[3]);
    const int8_t   preset = (int8_t)atoi(argv[4]);
    const char*    in_yuv = argv[5];
    const char*    out    = argv[6];
    /* Optional 8th arg = encoder bit depth (default 8, so every existing
       6-arg caller is byte-identical). At bd10 the input .yuv is PACKED u16
       little-endian (2 bytes/sample), matching this fork's packed-u16 intake. */
    const uint32_t bit_depth   = (argc == 8) ? (uint32_t)atoi(argv[7]) : 8;
    const size_t   sample_size = (bit_depth > 8) ? 2 : 1;

    const size_t ysz = (size_t)w * h;
    /* AV1 4:2:0 CEILING chroma dims ((w+1)/2). The .yuv the Rust harness
       writes is laid out ceiling-strided, matching the port's ceiling chroma
       intake; for EVEN dims ceiling == floor, so every pre-existing caller is
       byte-identical. Task #95 goal 1 (odd true dims, e.g. 65x65): the C
       library internally reads FLOOR chroma (luma_width>>1) columns/rows from
       this ceiling-strided buffer (resource_coordination_process.c:491) — for
       the flat u=v=128 synthetic chroma the ignored last ceiling col/row are
       128 too, so both encoders see identical chroma content. */
    const size_t cw = ((size_t)w + 1) / 2;
    const size_t ch = ((size_t)h + 1) / 2;
    const size_t csz = cw * ch;
    const size_t frame_bytes = (ysz + 2 * csz) * sample_size;

    uint8_t* yuv = malloc(frame_bytes);
    if (!yuv)
        die("oom", 0);
    FILE* fi = fopen(in_yuv, "rb");
    if (!fi)
        die("cannot open input .yuv", 0);
    if (fread(yuv, 1, frame_bytes, fi) != frame_bytes)
        die("short read (need w*h*3/2 * sample_size bytes of I420)", 0);
    fclose(fi);

    /* STEP 1: handle + library defaults. */
    EbComponentType*         handle = NULL;
    EbSvtAv1EncConfiguration cfg;
    memset(&cfg, 0, sizeof(cfg));
    EbErrorType err = svt_av1_enc_init_handle(&handle, &cfg);
    if (err != EB_ErrorNone)
        die("svt_av1_enc_init_handle", err);

    /* STEP 2: matched still-picture/AVIF CQP config; everything else stays
     * at the library defaults loaded by init_handle. */
    cfg.source_width           = w;
    cfg.source_height          = h;
    cfg.enc_mode               = preset;
    cfg.rate_control_mode      = 0;   /* CQP/CRF */
    cfg.aq_mode                = 0;   /* rc 0 + aq 0 == CQP */
    cfg.qp                     = qp;  /* CLI domain 0..63 */
    cfg.avif                   = true; /* still_picture=1 + reduced_still_picture_header=1 */
    cfg.level_of_parallelism   = 1;   /* --lp 1 */
    cfg.encoder_bit_depth      = bit_depth;
    cfg.encoder_color_format   = EB_YUV420;
    cfg.frame_rate_numerator   = 30; /* matches the F30:1 y4m the perf gate feeds the app */
    cfg.frame_rate_denominator = 1;
    /* task #86: tile rows, log2 domain — direct passthrough into
     * cfg.tile_rows, which the public API documents as "Log 2 Tile Rows...
     * 0 means no tiling, 1 means split into 2" (EbSvtAv1Enc.h:607-611).
     * Absent the env var, cfg.tile_rows stays at the DEFAULT sentinel
     * (-1) that svt_av1_enc_init_handle populated, resolving to 0 tiles
     * exactly like today (enc_handle.c:4520-4522) — the regression
     * baseline is untouched. */
    const char* tile_rows_env = getenv("SVT_TILE_ROWS");
    if (tile_rows_env) {
        cfg.tile_rows = atoi(tile_rows_env);
    }

    /* Fork / fork-defaulted knobs. Types per EbSvtAv1Enc.h; absent env var =
     * untouched, so this whole block is inert for every pre-existing caller. */
    FORK_SET("SVT_FORK_AC_BIAS", ac_bias, FORK_D);
    FORK_SET("SVT_FORK_QP_SCALE_COMPRESS_STRENGTH", qp_scale_compress_strength, FORK_D);
    FORK_SET("SVT_FORK_HBD_MDS", hbd_mds, FORK_I);
    FORK_SET("SVT_FORK_SHARP_TX", sharp_tx, FORK_U8);
    FORK_SET("SVT_FORK_TX_BIAS", tx_bias, FORK_U8);
    FORK_SET("SVT_FORK_COMPLEX_HVS", complex_hvs, FORK_U8);
    FORK_SET("SVT_FORK_NOISE_NORM_STRENGTH", noise_norm_strength, FORK_U8);
    FORK_SET("SVT_FORK_NOISE_ADAPTIVE_FILTERING", noise_adaptive_filtering, FORK_U8);
    FORK_SET("SVT_FORK_CDEF_SCALING", cdef_scaling, FORK_U8);
    FORK_SET("SVT_FORK_NOISE_STRENGTH", noise_strength, FORK_U8);
    FORK_SET("SVT_FORK_NOISE_CHROMA_FROM_LUMA", noise_chroma_from_luma, FORK_U8);
    FORK_SET("SVT_FORK_KF_TF_STRENGTH", kf_tf_strength, FORK_U8);
    FORK_SET("SVT_FORK_TF_STRENGTH", tf_strength, FORK_U8);
    FORK_SET("SVT_FORK_TUNE", tune, FORK_U8);
    FORK_SET("SVT_FORK_VARIANCE_BOOST_STRENGTH", variance_boost_strength, FORK_U8);
    FORK_SET("SVT_FORK_VARIANCE_OCTILE", variance_octile, FORK_U8);
    FORK_SET("SVT_FORK_VARIANCE_BOOST_CURVE", variance_boost_curve, FORK_U8);
    FORK_SET("SVT_FORK_MIN_QM_LEVEL", min_qm_level, FORK_U8);
    FORK_SET("SVT_FORK_MAX_QM_LEVEL", max_qm_level, FORK_U8);
    FORK_SET("SVT_FORK_MIN_CHROMA_QM_LEVEL", min_chroma_qm_level, FORK_U8);
    FORK_SET("SVT_FORK_MAX_CHROMA_QM_LEVEL", max_chroma_qm_level, FORK_U8);
    FORK_SET("SVT_FORK_NOISE_STRENGTH_CHROMA", noise_strength_chroma, FORK_I32);
    FORK_SET("SVT_FORK_NOISE_SIZE", noise_size, FORK_I8);
    FORK_SET("SVT_FORK_SHARPNESS", sharpness, FORK_I8);
    FORK_SET("SVT_FORK_ALT_LAMBDA_FACTORS", alt_lambda_factors, FORK_B);
    FORK_SET("SVT_FORK_ALT_SSIM_TUNING", alt_ssim_tuning, FORK_B);
    FORK_SET("SVT_FORK_ENABLE_QM", enable_qm, FORK_B);
    FORK_SET("SVT_FORK_ENABLE_VARIANCE_BOOST", enable_variance_boost, FORK_B);

    err = svt_av1_enc_set_parameter(handle, &cfg);
    if (err != EB_ErrorNone)
        die("svt_av1_enc_set_parameter", err);

    /* STEP 3 */
    err = svt_av1_enc_init(handle);
    if (err != EB_ErrorNone)
        die("svt_av1_enc_init", err);

    /* STEP 4: one frame, then EOS (app_process_cmd.c pattern). */
    EbSvtIOFormat io;
    memset(&io, 0, sizeof(io));
    io.luma      = yuv;
    io.cb        = yuv + ysz * sample_size;
    io.cr        = yuv + (ysz + csz) * sample_size;
    io.y_stride  = w; /* strides are in SAMPLES (pixels), not bytes */
    io.cb_stride = (uint32_t)cw; /* ceiling chroma stride (matches the .yuv layout) */
    io.cr_stride = (uint32_t)cw;

    EbBufferHeaderType in_hdr;
    memset(&in_hdr, 0, sizeof(in_hdr));
    in_hdr.size         = sizeof(EbBufferHeaderType);
    in_hdr.p_buffer     = (uint8_t*)&io;
    in_hdr.n_filled_len = (uint32_t)frame_bytes;
    in_hdr.pts          = 0;
    in_hdr.pic_type     = EB_AV1_INVALID_PICTURE; /* encoder decides; frame 0 is a key frame */
    err                 = svt_av1_enc_send_picture(handle, &in_hdr);
    if (err != EB_ErrorNone)
        die("svt_av1_enc_send_picture", err);

    EbBufferHeaderType eos_hdr;
    memset(&eos_hdr, 0, sizeof(eos_hdr));
    eos_hdr.size     = sizeof(EbBufferHeaderType);
    eos_hdr.flags    = EB_BUFFERFLAG_EOS;
    eos_hdr.pic_type = EB_AV1_INVALID_PICTURE;
    err              = svt_av1_enc_send_picture(handle, &eos_hdr);
    if (err != EB_ErrorNone)
        die("svt_av1_enc_send_picture(EOS)", err);

    /* STEP 5: drain packets; concatenated payloads == raw OBU stream. */
    FILE* fo = fopen(out, "wb");
    if (!fo)
        die("cannot open output", 0);
    uint32_t npkt = 0, nbytes = 0;
    for (;;) {
        EbBufferHeaderType* pkt = NULL;
        err                     = svt_av1_enc_get_packet(handle, &pkt, 1 /* pic_send_done */);
        if (err == EB_ErrorMax)
            die("encode error from svt_av1_enc_get_packet", err);
        if (pkt == NULL)
            break;
        if (pkt->n_filled_len) {
            fwrite(pkt->p_buffer, 1, pkt->n_filled_len, fo);
            nbytes += pkt->n_filled_len;
            npkt++;
            fprintf(stderr, "capture_c_trace: packet %u: %u bytes, pts=%lld, flags=0x%x\n", npkt,
                    pkt->n_filled_len, (long long)pkt->pts, pkt->flags);
        }
        const uint32_t last = (pkt->flags & EB_BUFFERFLAG_EOS) != 0;
        svt_av1_enc_release_out_buffer(&pkt);
        if (last)
            break;
    }
    fclose(fo);
    fprintf(stderr, "capture_c_trace: wrote %u bytes (%u packets) to %s\n", nbytes, npkt, out);

    svt_av1_enc_deinit(handle);
    svt_av1_enc_deinit_handle(handle);
    free(yuv);
    return 0;
}
