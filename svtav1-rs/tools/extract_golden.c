/* Extract golden data from C SVT-AV1 forward transform functions.
 * These are the functions our Rust code must match exactly.
 */
#include <stdio.h>
#include <stdint.h>
#include <string.h>

extern const int32_t svt_aom_eb_av1_cospi_arr_data[7][64];
extern const int32_t svt_aom_eb_av1_sinpi_arr_data[7][5];

extern void svt_av1_fdct4_new(const int32_t *input, int32_t *output,
                              int8_t cos_bit, const int8_t *stage_range);
extern void svt_av1_fdct8_new(const int32_t *input, int32_t *output,
                              int8_t cos_bit, const int8_t *stage_range);
extern void svt_av1_fdct16_new(const int32_t *input, int32_t *output,
                               int8_t cos_bit, const int8_t *stage_range);
extern void svt_av1_fadst4_new(const int32_t *input, int32_t *output,
                               int8_t cos_bit, const int8_t *stage_range);
extern void svt_av1_fadst8_new(const int32_t *input, int32_t *output,
                               int8_t cos_bit, const int8_t *stage_range);

static void p(const char *label, const int32_t *arr, int n) {
    printf("  %s: [", label);
    for (int i = 0; i < n; i++) { printf("%d", arr[i]); if (i<n-1) printf(", "); }
    printf("]\n");
}

static void t(const char *name,
              void (*f)(const int32_t*, int32_t*, int8_t, const int8_t*),
              const int32_t *in, int n) {
    int32_t out[64] = {0};
    f(in, out, 12, NULL);
    printf("--- %s ---\n", name); p("in ", in, n); p("out", out, n);
}

int main(void) {
    printf("# C SVT-AV1 golden data (cos_bit=12)\n\n");

    /* Verify cospi Q12 table */
    const int32_t *c = svt_aom_eb_av1_cospi_arr_data[2]; /* row 2 = Q12 */
    printf("cospi_q12[0]=%d [16]=%d [32]=%d [48]=%d [63]=%d\n\n",
           c[0], c[16], c[32], c[48], c[63]);

    const int32_t *s = svt_aom_eb_av1_sinpi_arr_data[2]; /* row 2 = Q12 */
    printf("sinpi_q12: [%d, %d, %d, %d, %d]\n\n", s[0], s[1], s[2], s[3], s[4]);

    /* fdct4 */
    { int32_t in[]={100,100,100,100}; t("fdct4_dc", svt_av1_fdct4_new, in, 4); }
    { int32_t in[]={0,0,0,0}; t("fdct4_zero", svt_av1_fdct4_new, in, 4); }
    { int32_t in[]={100,-50,200,-150}; t("fdct4_mixed", svt_av1_fdct4_new, in, 4); }
    { int32_t in[]={1,0,0,0}; t("fdct4_impulse", svt_av1_fdct4_new, in, 4); }
    { int32_t in[]={1,-1,1,-1}; t("fdct4_alt", svt_av1_fdct4_new, in, 4); }

    /* fdct8 */
    { int32_t in[]={100,100,100,100,100,100,100,100}; t("fdct8_dc", svt_av1_fdct8_new, in, 8); }
    { int32_t in[]={0,0,0,0,0,0,0,0}; t("fdct8_zero", svt_av1_fdct8_new, in, 8); }
    { int32_t in[]={50,-25,100,-75,200,-150,80,-40}; t("fdct8_mixed", svt_av1_fdct8_new, in, 8); }
    { int32_t in[]={1,-1,1,-1,1,-1,1,-1}; t("fdct8_alt", svt_av1_fdct8_new, in, 8); }

    /* fdct16 */
    { int32_t in[16]; for(int i=0;i<16;i++) in[i]=50; t("fdct16_dc", svt_av1_fdct16_new, in, 16); }
    { int32_t in[16]; for(int i=0;i<16;i++) in[i]=i*10-80; t("fdct16_ramp", svt_av1_fdct16_new, in, 16); }

    /* fadst4 */
    { int32_t in[]={0,0,0,0}; t("fadst4_zero", svt_av1_fadst4_new, in, 4); }
    { int32_t in[]={100,-50,200,-150}; t("fadst4_mixed", svt_av1_fadst4_new, in, 4); }

    /* fadst8 */
    { int32_t in[]={0,0,0,0,0,0,0,0}; t("fadst8_zero", svt_av1_fadst8_new, in, 8); }
    { int32_t in[]={50,-25,100,-75,200,-150,80,-40}; t("fadst8_mixed", svt_av1_fadst8_new, in, 8); }

    return 0;
}
