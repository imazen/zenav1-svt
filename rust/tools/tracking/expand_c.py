from pathlib import Path
p=Path('/home/lilith/work/zen/zenmetrics/benchmarks/av1-compare/src/c_api.c');s=p.read_text()
s=s.replace('unsigned q, unsigned speed, unsigned threads, Output *out)', 'unsigned q, unsigned speed, unsigned threads,\n              unsigned bd, unsigned sx, unsigned sy, unsigned mono, int tune, int scm, unsigned sb128, Output *out)')
s=s.replace('    aom_image_t image;', '    aom_image_t image;\n    memset(&image,0,sizeof(image));\n    int allocated=0;\n    if (tune!=-1 || scm!=-1) return -10;')
s=s.replace('    cfg.g_w = w; cfg.g_h = h; cfg.g_threads = threads;','''    cfg.g_w = w; cfg.g_h = h; cfg.g_threads = threads;
    cfg.g_bit_depth=(aom_bit_depth_t)bd; cfg.g_input_bit_depth=bd;
    cfg.monochrome=mono;
    cfg.g_profile=bd==12?2:(!mono && sx==0?1:(!mono && sy==0?2:0));''')
s=s.replace('aom_codec_enc_init(&ctx, aom_codec_av1_cx(), &cfg, 0)', 'aom_codec_enc_init(&ctx, aom_codec_av1_cx(), &cfg, bd>8?AOM_CODEC_USE_HIGHBITDEPTH:0)')
s=s.replace('CTRL(AV1E_SET_SUPERBLOCK_SIZE, AOM_SUPERBLOCK_SIZE_64X64);','CTRL(AV1E_SET_SUPERBLOCK_SIZE, sb128?AOM_SUPERBLOCK_SIZE_128X128:AOM_SUPERBLOCK_SIZE_64X64);')
s=s.replace('    if (!aom_img_wrap(&image, AOM_IMG_FMT_I420, w, h, 1, (uint8_t *)pixels)) { rc = -5; goto done; }','''    aom_img_fmt_t fmt=sx==0?AOM_IMG_FMT_I444:(sy==0?AOM_IMG_FMT_I422:AOM_IMG_FMT_I420);
    if (bd>8) fmt=(aom_img_fmt_t)(fmt|AOM_IMG_FMT_HIGHBITDEPTH);
    if (!aom_img_alloc(&image,fmt,w,h,32)) {rc=-5;goto done;}
    allocated=1;
    image.bit_depth=bd; image.monochrome=mono; image.range=AOM_CR_STUDIO_RANGE;
    const uint8_t *src=pixels;
    for (unsigned p=0;p<3;++p) {
        unsigned pw=p?(w+(1u<<sx)-1)>>sx:w, ph=p?(h+(1u<<sy)-1)>>sy:h;
        unsigned bytes=pw*(bd>8?2:1);
        for (unsigned y=0;y<ph;++y) {
            uint8_t *dst=image.planes[p]+(size_t)y*image.stride[p];
            if (mono && p) memset(dst,0,bytes);
            else {memcpy(dst,src,bytes);src+=bytes;}
        }
    }''')
s=s.replace('    if (initialized) aom_codec_destroy(&ctx);','    if (allocated) aom_img_free(&image);\n    if (initialized) aom_codec_destroy(&ctx);',1)
s=s.replace('    EbComponentType *ctx = NULL;','''    if (mono || sx!=1 || sy!=1 || bd>10 || sb128) return -10;
    size_t frame_bytes=((size_t)w*h+(size_t)(w/2)*(h/2)*2)*(bd>8?2:1);
    /* malloc provides the alignment required by the native high-depth input. */
    uint8_t *owned=malloc(frame_bytes);if(!owned)return -11;
    memcpy(owned,pixels,frame_bytes);pixels=owned;
    EbComponentType *ctx = NULL;''',1)
s=s.replace('        return -2;\n    }\n    cfg.source_width', '        free(owned);return -2;\n    }\n    cfg.source_width',1)
s=s.replace('cfg.avif = 1; cfg.encoder_bit_depth = 8;','cfg.avif = 1; cfg.encoder_bit_depth = bd;\n    if(tune>=0)cfg.tune=(uint8_t)tune;\n    if(scm>=0)cfg.screen_content_mode=(uint32_t)scm;')
s=s.replace('io.cb = (uint8_t *)pixels + (size_t)w*h;','io.cb = (uint8_t *)pixels + (size_t)w*h*(bd>8?2:1);')
s=s.replace('io.cr = io.cb + (size_t)(w/2)*(h/2);','io.cr = io.cb + (size_t)(w/2)*(h/2)*(bd>8?2:1);')
s=s.replace('in.n_filled_len = w*h + (w*h)/2;', 'in.n_filled_len = (uint32_t)frame_bytes;')
s=s.replace('    svt_av1_enc_deinit_handle(ctx);\n    return rc;', '    svt_av1_enc_deinit_handle(ctx);\n    free(owned);\n    return rc;',1)
s+='''
/* Generic output copy. The requested format is checked before any write, so
 * caller buffers cannot be overrun by a valid stream of a different format. */
int zm_decode_planar(const uint8_t *obu,size_t len,unsigned w,unsigned h,unsigned bd,
                     unsigned sx,unsigned sy,unsigned mono,uint16_t *dst) {
    aom_codec_ctx_t ctx; aom_codec_dec_cfg_t cfg={0};cfg.threads=1;
    memset(&ctx,0,sizeof(ctx));if(aom_codec_dec_init(&ctx,aom_codec_av1_dx(),&cfg,0))return -1;
    int rc=-2;
    if(!aom_codec_decode(&ctx,obu,len,NULL)) {
        aom_codec_iter_t it=NULL;aom_image_t *img=aom_codec_get_frame(&ctx,&it);
        if(img && img->d_w==w && img->d_h==h && img->bit_depth==bd && img->monochrome==(int)mono &&
           (mono || (img->x_chroma_shift==sx && img->y_chroma_shift==sy)) && img->range==AOM_CR_STUDIO_RANGE) {
            rc=0;
            for(unsigned p=0;p<(mono?1u:3u);++p) {
                unsigned pw=p?(w+(1u<<sx)-1)>>sx:w,ph=p?(h+(1u<<sy)-1)>>sy:h;
                for(unsigned y=0;y<ph;++y) {
                    const uint8_t *row=img->planes[p]+(size_t)y*img->stride[p];
                    for(unsigned x=0;x<pw;++x) *dst++=(img->fmt&AOM_IMG_FMT_HIGHBITDEPTH)?((const uint16_t*)row)[x]:row[x];
                }
            }
            if(aom_codec_get_frame(&ctx,&it))rc=-3;
        }
    }
    aom_codec_destroy(&ctx);return rc;
}
'''
p.write_text(s)
