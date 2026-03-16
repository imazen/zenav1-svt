/* Extract intra prediction and 2D transform golden data from C SVT-AV1.
 * Build from cbuild/:
 *   gcc -O0 -g -I../Source/Lib/Codec -I../Source/Lib/C_DEFAULT \
 *     -I../Source/Lib/Globals -I../Source/API -DNDEBUG \
 *     ../svtav1-rs/tools/extract_intra_golden.c \
 *     Source/Lib/C_DEFAULT/CMakeFiles/C_DEFAULT.dir/*.o \
 *     Source/Lib/Codec/CMakeFiles/CODEC.dir/transforms.c.o \
 *     Source/Lib/Codec/CMakeFiles/CODEC.dir/inv_transforms.c.o \
 *     Source/Lib/Codec/CMakeFiles/CODEC.dir/intra_prediction.c.o \
 *     -lm -o /tmp/extract_intra
 */
#include <stdio.h>
#include <stdint.h>
#include <string.h>

/* Intra prediction functions from C_DEFAULT */
extern void svt_aom_dc_predictor_c(uint8_t *dst, int stride,
    int bw, int bh, const uint8_t *above, const uint8_t *left);
extern void svt_aom_dc_left_predictor_c(uint8_t *dst, int stride,
    int bw, int bh, const uint8_t *above, const uint8_t *left);
extern void svt_aom_dc_top_predictor_c(uint8_t *dst, int stride,
    int bw, int bh, const uint8_t *above, const uint8_t *left);
extern void svt_aom_dc_128_predictor_c(uint8_t *dst, int stride,
    int bw, int bh, const uint8_t *above, const uint8_t *left);
extern void svt_aom_v_predictor_c(uint8_t *dst, int stride,
    int bw, int bh, const uint8_t *above, const uint8_t *left);
extern void svt_aom_h_predictor_c(uint8_t *dst, int stride,
    int bw, int bh, const uint8_t *above, const uint8_t *left);
extern void svt_aom_paeth_predictor_c(uint8_t *dst, int stride,
    int bw, int bh, const uint8_t *above, const uint8_t *left);

static void pu8(const char *l, const uint8_t *a, int n) {
    printf("  %s: [", l);
    for (int i = 0; i < n; i++) { printf("%d%s", a[i], i<n-1?", ":""); }
    printf("]\n");
}

int main(void) {
    printf("# Intra prediction golden data from C SVT-AV1\n\n");

    /* DC prediction 4x4 */
    {
        uint8_t above[4] = {100, 100, 100, 100};
        uint8_t left[4]  = {100, 100, 100, 100};
        uint8_t dst[16] = {0};
        svt_aom_dc_predictor_c(dst, 4, 4, 4, above, left);
        printf("--- dc_4x4_uniform ---\n");
        pu8("above", above, 4); pu8("left", left, 4);
        pu8("dst", dst, 16);
    }
    {
        uint8_t above[8] = {10, 20, 30, 40, 50, 60, 70, 80};
        uint8_t left[8]  = {80, 70, 60, 50, 40, 30, 20, 10};
        uint8_t dst[64] = {0};
        svt_aom_dc_predictor_c(dst, 8, 8, 8, above, left);
        printf("--- dc_8x8_gradient ---\n");
        pu8("above", above, 8); pu8("left", left, 8);
        pu8("row0", dst, 8); pu8("row7", dst+56, 8);
    }
    /* DC 128 (no neighbors) */
    {
        uint8_t dst[16] = {0};
        svt_aom_dc_128_predictor_c(dst, 4, 4, 4, NULL, NULL);
        printf("--- dc_128_4x4 ---\n");
        pu8("dst", dst, 16);
    }
    /* V prediction */
    {
        uint8_t above[4] = {10, 20, 30, 40};
        uint8_t left[4]  = {0, 0, 0, 0};
        uint8_t dst[16] = {0};
        svt_aom_v_predictor_c(dst, 4, 4, 4, above, left);
        printf("--- v_4x4 ---\n");
        pu8("above", above, 4);
        pu8("row0", dst, 4); pu8("row3", dst+12, 4);
    }
    /* H prediction */
    {
        uint8_t above[4] = {0, 0, 0, 0};
        uint8_t left[4]  = {10, 20, 30, 40};
        uint8_t dst[16] = {0};
        svt_aom_h_predictor_c(dst, 4, 4, 4, above, left);
        printf("--- h_4x4 ---\n");
        pu8("left", left, 4);
        pu8("row0", dst, 4); pu8("row3", dst+12, 4);
    }
    /* Paeth prediction */
    {
        /* Paeth uses above[-1] as top_left. The C function reads above[-1]. */
        uint8_t buf[5] = {50, 10, 20, 30, 40}; /* buf[0]=top_left, buf[1..4]=above */
        uint8_t *above = buf + 1; /* above[-1] = 50 = top_left */
        uint8_t left[4] = {60, 70, 80, 90};
        uint8_t dst[16] = {0};
        svt_aom_paeth_predictor_c(dst, 4, 4, 4, above, left);
        printf("--- paeth_4x4 ---\n");
        printf("  top_left: %d\n", buf[0]);
        pu8("above", above, 4);
        pu8("left", left, 4);
        pu8("row0", dst, 4); pu8("row1", dst+4, 4); pu8("row2", dst+8, 4); pu8("row3", dst+12, 4);
    }
    /* Paeth 8x8 */
    {
        uint8_t buf[9] = {100, 50, 60, 70, 80, 90, 100, 110, 120};
        uint8_t *above = buf + 1;
        uint8_t left[8] = {200, 190, 180, 170, 160, 150, 140, 130};
        uint8_t dst[64] = {0};
        svt_aom_paeth_predictor_c(dst, 8, 8, 8, above, left);
        printf("--- paeth_8x8 ---\n");
        printf("  top_left: %d\n", buf[0]);
        pu8("above", above, 8);
        pu8("left", left, 8);
        pu8("row0", dst, 8); pu8("row7", dst+56, 8);
    }

    /* CDF update golden data */
    printf("\n# CDF update golden data\n");
    {
        /* Replicate update_cdf from cabac_context_model.h */
        /* CDF for nsymbs=4: [24576, 16384, 8192, 0, count=0] */
        uint16_t cdf[5] = {24576, 16384, 8192, 0, 0};
        printf("--- cdf_4sym_initial ---\n");
        printf("  cdf: [%d, %d, %d, %d, count=%d]\n", cdf[0], cdf[1], cdf[2], cdf[3], cdf[4]);

        /* Simulate update_cdf(cdf, val=2, nsymbs=4) */
        int val = 2, nsymbs = 4;
        int count = cdf[nsymbs];
        int rate = 4 + (count >> 4) + (nsymbs > 3);
        for (int i = 0; i < nsymbs - 1; i++) {
            if (i < val)
                cdf[i] += (32768 - cdf[i]) >> rate;
            else
                cdf[i] -= cdf[i] >> rate;
        }
        cdf[nsymbs] += (count < 32);
        printf("--- cdf_4sym_after_val2 ---\n");
        printf("  cdf: [%d, %d, %d, %d, count=%d]\n", cdf[0], cdf[1], cdf[2], cdf[3], cdf[4]);
    }

    return 0;
}
