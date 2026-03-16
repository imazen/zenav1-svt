/* Complete golden data extractor for C SVT-AV1.
 * Build from cbuild/:
 *   gcc -O0 -g ../svtav1-rs/tools/extract_golden.c \
 *     Source/Lib/Codec/CMakeFiles/CODEC.dir/transforms.c.o \
 *     Source/Lib/Codec/CMakeFiles/CODEC.dir/inv_transforms.c.o \
 *     -lm -o /tmp/extract_golden
 */
#include <stdio.h>
#include <stdint.h>
#include <string.h>

extern const int32_t svt_aom_eb_av1_cospi_arr_data[7][64];
extern const int32_t svt_aom_eb_av1_sinpi_arr_data[7][5];

/* Forward transforms (stage_range ignored) */
extern void svt_av1_fdct4_new(const int32_t*, int32_t*, int8_t, const int8_t*);
extern void svt_av1_fdct8_new(const int32_t*, int32_t*, int8_t, const int8_t*);
extern void svt_av1_fdct16_new(const int32_t*, int32_t*, int8_t, const int8_t*);
extern void svt_av1_fadst4_new(const int32_t*, int32_t*, int8_t, const int8_t*);
extern void svt_av1_fadst8_new(const int32_t*, int32_t*, int8_t, const int8_t*);
extern void svt_av1_fidentity4_c(const int32_t*, int32_t*, int8_t, const int8_t*);
extern void svt_av1_fidentity8_c(const int32_t*, int32_t*, int8_t, const int8_t*);

/* Inverse transforms (stage_range used for clamping) */
extern void svt_av1_idct4_new(const int32_t*, int32_t*, int8_t, const int8_t*);
extern void svt_av1_idct8_new(const int32_t*, int32_t*, int8_t, const int8_t*);
extern void svt_av1_iadst4_new(const int32_t*, int32_t*, int8_t, const int8_t*);
extern void svt_av1_iadst8_new(const int32_t*, int32_t*, int8_t, const int8_t*);
extern void svt_av1_iidentity4_c(const int32_t*, int32_t*, int8_t, const int8_t*);
extern void svt_av1_iidentity8_c(const int32_t*, int32_t*, int8_t, const int8_t*);

static void p(const char *l, const int32_t *a, int n) {
    printf("  %s: [", l);
    for (int i = 0; i < n; i++) { printf("%d%s", a[i], i<n-1?", ":""); }
    printf("]\n");
}

/* Wide stage_range so clamp_value doesn't trigger */
static const int8_t wide_range[12] = {31,31,31,31,31,31,31,31,31,31,31,31};

static void fwd(const char *name,
    void (*f)(const int32_t*,int32_t*,int8_t,const int8_t*),
    const int32_t *in, int n) {
    int32_t out[64]={0};
    f(in, out, 12, NULL);
    printf("--- %s ---\n", name); p("in ", in, n); p("out", out, n);
}

static void inv(const char *name,
    void (*f)(const int32_t*,int32_t*,int8_t,const int8_t*),
    const int32_t *in, int n) {
    int32_t out[64]={0};
    f(in, out, 12, wide_range);
    printf("--- %s ---\n", name); p("in ", in, n); p("out", out, n);
}

int main(void) {
    printf("# Golden data from C SVT-AV1 (cos_bit=12)\n\n");

    /* ===== Forward DCT ===== */
    { int32_t in[]={100,100,100,100}; fwd("fdct4_dc", svt_av1_fdct4_new, in, 4); }
    { int32_t in[]={0,0,0,0}; fwd("fdct4_zero", svt_av1_fdct4_new, in, 4); }
    { int32_t in[]={100,-50,200,-150}; fwd("fdct4_mixed", svt_av1_fdct4_new, in, 4); }
    { int32_t in[]={1,0,0,0}; fwd("fdct4_impulse", svt_av1_fdct4_new, in, 4); }
    { int32_t in[]={1,-1,1,-1}; fwd("fdct4_alt", svt_av1_fdct4_new, in, 4); }

    { int32_t in[]={100,100,100,100,100,100,100,100}; fwd("fdct8_dc", svt_av1_fdct8_new, in, 8); }
    { int32_t in[]={0,0,0,0,0,0,0,0}; fwd("fdct8_zero", svt_av1_fdct8_new, in, 8); }
    { int32_t in[]={50,-25,100,-75,200,-150,80,-40}; fwd("fdct8_mixed", svt_av1_fdct8_new, in, 8); }
    { int32_t in[]={1,-1,1,-1,1,-1,1,-1}; fwd("fdct8_alt", svt_av1_fdct8_new, in, 8); }

    { int32_t in[16]; for(int i=0;i<16;i++) in[i]=50; fwd("fdct16_dc", svt_av1_fdct16_new, in, 16); }
    { int32_t in[16]; for(int i=0;i<16;i++) in[i]=i*10-80; fwd("fdct16_ramp", svt_av1_fdct16_new, in, 16); }

    /* ===== Forward ADST ===== */
    { int32_t in[]={0,0,0,0}; fwd("fadst4_zero", svt_av1_fadst4_new, in, 4); }
    { int32_t in[]={100,-50,200,-150}; fwd("fadst4_mixed", svt_av1_fadst4_new, in, 4); }
    { int32_t in[]={0,0,0,0,0,0,0,0}; fwd("fadst8_zero", svt_av1_fadst8_new, in, 8); }
    { int32_t in[]={50,-25,100,-75,200,-150,80,-40}; fwd("fadst8_mixed", svt_av1_fadst8_new, in, 8); }

    /* ===== Forward Identity ===== */
    { int32_t in[]={100,200,300,400}; fwd("fidentity4", svt_av1_fidentity4_c, in, 4); }
    { int32_t in[]={100,100,100,100,100,100,100,100}; fwd("fidentity8", svt_av1_fidentity8_c, in, 8); }

    /* ===== Inverse DCT ===== */
    { int32_t in[]={283,0,0,0}; inv("idct4_dc", svt_av1_idct4_new, in, 4); }
    { int32_t in[]={0,0,0,0}; inv("idct4_zero", svt_av1_idct4_new, in, 4); }
    { int32_t in[]={71,135,-141,327}; inv("idct4_from_fdct4_mixed", svt_av1_idct4_new, in, 4); }

    { int32_t in[]={566,0,0,0,0,0,0,0}; inv("idct8_dc", svt_av1_idct8_new, in, 8); }
    { int32_t in[]={0,0,0,0,0,0,0,0}; inv("idct8_zero", svt_av1_idct8_new, in, 8); }
    { int32_t in[]={99,87,-66,3,92,-27,-141,554}; inv("idct8_from_fdct8_mixed", svt_av1_idct8_new, in, 8); }

    /* ===== Inverse ADST ===== */
    { int32_t in[]={0,0,0,0}; inv("iadst4_zero", svt_av1_iadst4_new, in, 4); }
    { int32_t in[]={26,163,-145,319}; inv("iadst4_from_fadst4_mixed", svt_av1_iadst4_new, in, 4); }

    /* ===== Inverse Identity ===== */
    { int32_t in[]={100,200,300,400}; inv("iidentity4", svt_av1_iidentity4_c, in, 4); }
    { int32_t in[]={200,200,200,200,200,200,200,200}; inv("iidentity8", svt_av1_iidentity8_c, in, 8); }

    /* ===== Roundtrip: fdct4 -> idct4 ===== */
    {
        int32_t orig[] = {100,-50,200,-150};
        int32_t fwd_out[4]={0}, inv_out[4]={0};
        svt_av1_fdct4_new(orig, fwd_out, 12, NULL);
        svt_av1_idct4_new(fwd_out, inv_out, 12, wide_range);
        printf("--- roundtrip_dct4 ---\n");
        p("orig", orig, 4);
        p("fwd ", fwd_out, 4);
        p("inv ", inv_out, 4);
        printf("  scale: inv[i]/orig[i] ≈ ");
        for (int i=0;i<4;i++) printf("%.2f ", orig[i]?inv_out[i]/(double)orig[i]:0);
        printf("\n");
    }

    /* ===== Roundtrip: fdct8 -> idct8 ===== */
    {
        int32_t orig[] = {50,-25,100,-75,200,-150,80,-40};
        int32_t fwd_out[8]={0}, inv_out[8]={0};
        svt_av1_fdct8_new(orig, fwd_out, 12, NULL);
        svt_av1_idct8_new(fwd_out, inv_out, 12, wide_range);
        printf("--- roundtrip_dct8 ---\n");
        p("orig", orig, 8);
        p("fwd ", fwd_out, 8);
        p("inv ", inv_out, 8);
        printf("  scale: inv[i]/orig[i] ≈ ");
        for (int i=0;i<8;i++) printf("%.2f ", orig[i]?inv_out[i]/(double)orig[i]:0);
        printf("\n");
    }

    return 0;
}
