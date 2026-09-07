/* Oracles call the pinned exported implementations; no DSP is transcribed. */
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include "definitions.h"
#include "noise_model.h"
#include "noise_util.h"
#include "grainSynthesis.h"
#include "pic_buffer_desc.h"
void svt_aom_setup_common_rtcd_internal(uint64_t);
void svt_aom_setup_rtcd_internal(EbCpuFlags);
/* Rust serializes these calls: RTCD and C synthesis have process-global state. */
static void init(void) { svt_aom_setup_common_rtcd_internal(0); svt_aom_setup_rtcd_internal(0); }
#define FFT(N) \
void svt_aom_fft##N##x##N##_float_c(const float*,float*,float*); \
void svt_aom_ifft##N##x##N##_float_c(const float*,float*,float*);
FFT(2) FFT(4) FFT(8) FFT(16) FFT(32)
void ref_fg_fft(int n,int inverse,const float* in,float* tmp,float* out) {
#define CASE(N) case N: if(inverse) svt_aom_ifft##N##x##N##_float_c(in,tmp,out); else svt_aom_fft##N##x##N##_float_c(in,tmp,out); break;
    switch(n) { CASE(2) CASE(4) CASE(8) CASE(16) CASE(32) default: abort(); }
}
void ref_fg_filter(int n,float* data,float psd) {svt_aom_noise_tx_filter_c(n,data,psd);}
void ref_fg_synthesis(const AomFilmGrain* p,uint16_t* y,uint16_t* u,uint16_t* v,int w,int h,int ys,int cs,int depth) {
    init();
    if(depth>8) {svt_av1_add_film_grain_run(p,(uint8_t*)y,(uint8_t*)u,(uint8_t*)v,h,w,ys,cs,1,1,1);return;}
    int lengths[3]={ys*h,cs*((h+1)/2),cs*((h+1)/2)};uint16_t* src[3]={y,u,v};uint8_t* b[3];
    for(int c=0;c<3;c++){b[c]=malloc(lengths[c]);for(int i=0;i<lengths[c];i++)b[c][i]=src[c][i];}
    svt_av1_add_film_grain_run(p,b[0],b[1],b[2],h,w,ys,cs,0,1,1);
    for(int c=0;c<3;c++){for(int i=0;i<lengths[c];i++)src[c][i]=b[c][i];free(b[c]);}
}
void ref_fg_flat(const uint16_t* data,int w,int h,int stride,int bs,int depth,uint8_t* flat,double* plane,double* block,int ox,int oy) {
    init();AomFlatBlockFinder f;svt_aom_flat_block_finder_init(&f,bs,depth,1);
    svt_aom_flat_block_finder_extract_block_c(&f,(const uint8_t*)data,w,h,stride,ox,oy,plane,block);
    svt_aom_flat_block_finder_run(&f,(const uint8_t*)data,w,h,stride,flat);
    svt_aom_flat_block_finder_free(&f);
}
int ref_fg_wiener(const uint16_t* y,const uint16_t* u,const uint16_t* v,uint16_t* dy,uint16_t* du,uint16_t* dv,int w,int h,int ys,int cs,int bs,int depth,float* psd) {
    init();const uint8_t* data[3]={(const uint8_t*)y,(const uint8_t*)u,(const uint8_t*)v};uint8_t* out[3]={(uint8_t*)dy,(uint8_t*)du,(uint8_t*)dv};int strides[3]={ys,cs,cs},sub[2]={1,1};
    return svt_aom_wiener_denoise_2d(data,out,w,h,strides,sub,psd,bs,depth,1);
}
int ref_fg_model(const uint16_t* y,const uint16_t* u,const uint16_t* v,uint16_t* dy,uint16_t* du,uint16_t* dv,int w,int h,int ys,int cs,int depth,int strength,int adaptive,AomFilmGrain* grain) {
    init();AomDenoiseAndModel ctx={0};DenoiseAndModelInitData info={0};
    info.noise_level=strength;info.encoder_bit_depth=depth;info.encoder_color_format=EB_YUV420;
    info.width=w;info.height=h;info.y_stride=ys;info.u_stride=info.v_stride=cs;info.adaptive_film_grain=adaptive;
    if(svt_aom_denoise_and_model_ctor(&ctx,&info))return 0;
    const uint16_t* src[3]={y,u,v};uint16_t* dst[3]={dy,du,dv};uint8_t* bytes[3];uint8_t* inc[3];
    int strides[3]={ys,cs,cs};int heights[3]={h,h/2,h/2};int incstride[3];
    for(int c=0;c<3;c++) {
        int n=strides[c]*heights[c];incstride[c]=(strides[c]+3)/4;bytes[c]=calloc(n,1);inc[c]=calloc(incstride[c]*heights[c],1);
        for(int row=0;row<heights[c];row++)for(int x=0;x<strides[c];x++) {
            int i=row*strides[c]+x;bytes[c][i]=src[c][i]>>(depth-8);
            if(depth>8)inc[c][row*incstride[c]+x/4]|=(src[c][i]&3)<<(6-2*(x%4));
        }
    }
    EbPictureBufferDesc pic={0};pic.width=w;pic.height=h;pic.y_stride=ys;pic.u_stride=pic.v_stride=cs;
    pic.y_buffer=bytes[0];pic.u_buffer=bytes[1];pic.v_buffer=bytes[2];
    pic.y_buffer_bit_inc=inc[0];pic.u_buffer_bit_inc=inc[1];pic.v_buffer_bit_inc=inc[2];
    pic.y_stride_bit_inc=incstride[0]*4;pic.u_stride_bit_inc=incstride[1]*4;pic.v_stride_bit_inc=incstride[2]*4;
    int ok=svt_aom_denoise_and_model_run(&ctx,&pic,grain,depth>8);
    for(int c=0;c<3;c++) {int n=strides[c]*heights[c];for(int i=0;i<n;i++)dst[c][i]=depth>8?((uint16_t*)ctx.denoised[c])[i]:ctx.denoised[c][i];free(bytes[c]);free(inc[c]);}
    ctx.dctor(&ctx);return ok;
}
int ref_fg_solver(int bins,int depth,const double* means,const double* stds,int count,int maxpoints,double* a,double* b,double* x,double* points) {
    AomNoiseStrengthSolver s;svt_aom_noise_strength_solver_init(&s,bins,depth);
    for(int i=0;i<count;i++)svt_aom_noise_strength_solver_add_measurement(&s,means[i],stds[i]);
    int ok=svt_aom_noise_strength_solver_solve(&s);int result=-1;
    if(ok) {AomNoiseStrengthLut lut={0};svt_aom_noise_strength_solver_fit_piecewise(&s,maxpoints,&lut);result=lut.num_points;memcpy(points,lut.points,result*2*sizeof(double));svt_aom_noise_strength_lut_free(&lut);}
    memcpy(a,s.eqns.A,bins*bins*sizeof(double));memcpy(b,s.eqns.b,bins*sizeof(double));memcpy(x,s.eqns.x,bins*sizeof(double));
    free(s.eqns.A);free(s.eqns.b);free(s.eqns.x);return result;
}
