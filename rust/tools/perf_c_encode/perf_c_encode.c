/*
 * perf_c_encode — encode-only WALL-TIME harness for the in-tree C SVT-AV1
 * static library. The C half of the G4 performance gate (tools/perf_gate.sh).
 *
 * This is the timing sibling of tools/capture_c_trace: it drives the SAME
 * public API at the SAME matched still-picture/CQP config (--rc 0 --aq-mode 0
 * --qp Q --avif 1 --lp 1 -n 1, 4:2:0 8-bit) so the bytes it emits are the very
 * bytes capture_c_trace/SvtAv1EncApp emit — but it carries NO -Wl,--wrap= trace
 * interposers (those would dominate the timing) and it MEASURES the encode.
 *
 * What is timed (apples-to-apples with the port's `encode_frame_420`):
 *   ONLY the per-frame encode work — svt_av1_enc_send_picture(frame) +
 *   send(EOS) + drain get_packet loop. The one-time setup (init_handle /
 *   set_parameter / svt_av1_enc_init — table build, buffer alloc, thread
 *   spawn) is done BEFORE the clock starts, exactly as the port excludes
 *   `EncodePipeline::new` from its timed region. Setup is C's analogue of the
 *   port constructor; both harnesses time the frame encode, not the ctor.
 *
 * Warmup: `[warmup]` full init->encode->deinit cycles run first (untimed) to
 * warm the allocator / OS page cache / branch predictors; only the final
 * cycle's send->drain region is reported. Symmetric with the port harness's
 * warmup encodes.
 *
 * Usage: perf_c_encode <width> <height> <cli_qp 0..63> <preset> <in.yuv> <out.obu> [warmup=1]
 * Output (stdout, machine-readable, one line): "ENCODE_NS=<n> BYTES=<m>"
 *        everything else (errors) -> stderr.
 *
 * NOT part of the cargo workspace build — compiled on demand by build.sh.
 */
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#include "EbSvtAv1.h"
#include "EbSvtAv1Enc.h"

static void die(const char* msg, int32_t err) {
    fprintf(stderr, "perf_c_encode: %s (err=0x%x)\n", msg, (unsigned)err);
    exit(1);
}

static int64_t now_ns(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (int64_t)ts.tv_sec * 1000000000LL + (int64_t)ts.tv_nsec;
}

/* One full init->encode->deinit cycle. TIMES ONLY the send->drain region
 * (setup excluded, matching the port's timed `encode_frame_420`). When
 * `out_path` is non-NULL the drained OBU stream is written there. Returns the
 * send->drain elapsed nanoseconds; *out_bytes gets the coded size. */
static int64_t encode_once(uint32_t w, uint32_t h, uint32_t qp, int8_t preset,
                           const uint8_t* yuv, size_t frame_bytes, size_t ysz, size_t csz,
                           size_t cw, const char* out_path, uint32_t* out_bytes) {
    /* STEP 1: handle + library defaults. */
    EbComponentType*         handle = NULL;
    EbSvtAv1EncConfiguration cfg;
    memset(&cfg, 0, sizeof(cfg));
    EbErrorType err = svt_av1_enc_init_handle(&handle, &cfg);
    if (err != EB_ErrorNone)
        die("svt_av1_enc_init_handle", err);

    /* STEP 2: matched still-picture/AVIF CQP config (identical knob set to
     * tools/capture_c_trace.c — the proven byte-identity oracle). Everything
     * not set here stays at the library defaults. Tile fields are left at the
     * init_handle default sentinel (-1) -> resolves to 0/0, i.e. single tile,
     * matching capture_c_trace with no SVT_TILE_* env. */
    cfg.source_width           = w;
    cfg.source_height          = h;
    cfg.enc_mode               = preset;
    cfg.rate_control_mode      = 0;    /* CQP/CRF */
    cfg.aq_mode                = 0;    /* rc 0 + aq 0 == CQP */
    cfg.qp                     = qp;   /* CLI domain 0..63 */
    cfg.avif                   = true; /* still_picture=1 + reduced_still_picture_header=1 */
    cfg.level_of_parallelism   = 1;    /* --lp 1 */
    cfg.encoder_bit_depth      = 8;
    cfg.encoder_color_format   = EB_YUV420;
    cfg.frame_rate_numerator   = 30;
    cfg.frame_rate_denominator = 1;

    err = svt_av1_enc_set_parameter(handle, &cfg);
    if (err != EB_ErrorNone)
        die("svt_av1_enc_set_parameter", err);

    /* STEP 3: init (table build, buffer alloc, thread spawn). UNTIMED. */
    err = svt_av1_enc_init(handle);
    if (err != EB_ErrorNone)
        die("svt_av1_enc_init", err);

    EbSvtIOFormat io;
    memset(&io, 0, sizeof(io));
    io.luma      = (uint8_t*)yuv;
    io.cb        = (uint8_t*)yuv + ysz;
    io.cr        = (uint8_t*)yuv + (ysz + csz);
    io.y_stride  = w;
    io.cb_stride = (uint32_t)cw;
    io.cr_stride = (uint32_t)cw;

    EbBufferHeaderType in_hdr;
    memset(&in_hdr, 0, sizeof(in_hdr));
    in_hdr.size         = sizeof(EbBufferHeaderType);
    in_hdr.p_buffer     = (uint8_t*)&io;
    in_hdr.n_filled_len = (uint32_t)frame_bytes;
    in_hdr.pts          = 0;
    in_hdr.pic_type     = EB_AV1_INVALID_PICTURE;

    EbBufferHeaderType eos_hdr;
    memset(&eos_hdr, 0, sizeof(eos_hdr));
    eos_hdr.size     = sizeof(EbBufferHeaderType);
    eos_hdr.flags    = EB_BUFFERFLAG_EOS;
    eos_hdr.pic_type = EB_AV1_INVALID_PICTURE;

    /* ---- TIMED REGION: encode one frame ---- */
    const int64_t t0 = now_ns();
    err               = svt_av1_enc_send_picture(handle, &in_hdr);
    if (err != EB_ErrorNone)
        die("svt_av1_enc_send_picture", err);
    err = svt_av1_enc_send_picture(handle, &eos_hdr);
    if (err != EB_ErrorNone)
        die("svt_av1_enc_send_picture(EOS)", err);

    FILE*    fo     = out_path ? fopen(out_path, "wb") : NULL;
    uint32_t nbytes = 0;
    for (;;) {
        EbBufferHeaderType* pkt = NULL;
        err                     = svt_av1_enc_get_packet(handle, &pkt, 1 /* pic_send_done */);
        if (err == EB_ErrorMax)
            die("encode error from svt_av1_enc_get_packet", err);
        if (pkt == NULL)
            break;
        if (pkt->n_filled_len) {
            if (fo)
                fwrite(pkt->p_buffer, 1, pkt->n_filled_len, fo);
            nbytes += pkt->n_filled_len;
        }
        const uint32_t last = (pkt->flags & EB_BUFFERFLAG_EOS) != 0;
        svt_av1_enc_release_out_buffer(&pkt);
        if (last)
            break;
    }
    const int64_t t1 = now_ns();
    /* ---- END TIMED REGION ---- */

    if (fo)
        fclose(fo);

    svt_av1_enc_deinit(handle);
    svt_av1_enc_deinit_handle(handle);

    if (out_bytes)
        *out_bytes = nbytes;
    return t1 - t0;
}

int main(int argc, char** argv) {
    if (argc != 7 && argc != 8) {
        fprintf(stderr,
                "usage: %s <width> <height> <cli_qp 0..63> <preset> <in.yuv> <out.obu> [warmup=1]\n",
                argv[0]);
        return 2;
    }
    const uint32_t w      = (uint32_t)atoi(argv[1]);
    const uint32_t h      = (uint32_t)atoi(argv[2]);
    const uint32_t qp     = (uint32_t)atoi(argv[3]);
    const int8_t   preset = (int8_t)atoi(argv[4]);
    const char*    in_yuv = argv[5];
    const char*    out    = argv[6];
    const int      warmup = (argc == 8) ? atoi(argv[7]) : 1;

    const size_t ysz         = (size_t)w * h;
    const size_t cw          = ((size_t)w + 1) / 2; /* AV1 4:2:0 ceiling chroma */
    const size_t ch          = ((size_t)h + 1) / 2;
    const size_t csz         = cw * ch;
    const size_t frame_bytes = ysz + 2 * csz; /* 8-bit */

    uint8_t* yuv = malloc(frame_bytes);
    if (!yuv)
        die("oom", 0);
    FILE* fi = fopen(in_yuv, "rb");
    if (!fi)
        die("cannot open input .yuv", 0);
    if (fread(yuv, 1, frame_bytes, fi) != frame_bytes)
        die("short read (need w*h*3/2 bytes of 8-bit I420)", 0);
    fclose(fi);

    /* Untimed warmup cycles (allocator / page-cache / predictor warmup). */
    for (int k = 0; k < warmup; k++)
        (void)encode_once(w, h, qp, preset, yuv, frame_bytes, ysz, csz, cw, NULL, NULL);

    /* Timed cycle (writes the .obu for the byte-identity check). */
    uint32_t      bytes = 0;
    const int64_t ns    = encode_once(w, h, qp, preset, yuv, frame_bytes, ysz, csz, cw, out, &bytes);

    printf("ENCODE_NS=%lld BYTES=%u\n", (long long)ns, bytes);
    free(yuv);
    return 0;
}
