#!/usr/bin/env python3
"""Field-level sequence-header differ for two raw OBU streams.

`identity_diff.py` localizes a divergence to a SYMBOL using the arithmetic-coder
op traces, which needs a `--wrap`-capable linker (GNU ld). On a host without one
— every macOS box — its verdict degrades to "the bytes differ", which is useless
for a header bug: a one-bit field error shifts every following field and the byte
offset names nothing.

This decodes the sequence header OBU of both streams per spec 5.5.1 and prints
the fields side by side, so a divergence names the FIELD. It has no dependencies
and does not need the C library.

    tools/sh_fields.py <c.obu> <rust.obu>

Built for the inter campaign (docs/INTER-ENCODE-PLAN.md): the video-mode
sequence header has ~14 fields the still (reduced) header never writes, and
they were where the first two defects lived.

CAVEAT, the same one `identity_diff.py` carries: this walks the header with its
own model of the layout. If an EARLIER field's value changes a later field's
presence, the names after that point can be wrong — the first DIFFERS line is
the fact, the ones after it are hints. Stop reading at the first one.
"""
import sys
def leb(b,i):
    v=0;s=0
    while True:
        x=b[i];i+=1;v|=(x&0x7f)<<s;s+=7
        if not(x&0x80):return v,i
def obus(b):
    i=0;out=[]
    while i<len(b):
        h=b[i];t=(h>>3)&0xf;ext=(h>>2)&1;hs=(h>>1)&1;j=i+1
        if ext:j+=1
        sz,j=(leb(b,j) if hs else (len(b)-j,j))
        out.append((t,b[j:j+sz]));i=j+sz
    return out
class BR:
    def __init__(s,b):s.b=b;s.p=0
    def f(s,n):
        v=0
        for _ in range(n):
            v=(v<<1)|((s.b[s.p>>3]>>(7-(s.p&7)))&1);s.p+=1
        return v
def decode_sh(p):
    r=BR(p);o=[]
    def rd(name,n):
        v=r.f(n);o.append((name,v));return v
    rd('seq_profile',3); still=rd('still_picture',1); red=rd('reduced_still_picture_header',1)
    if red:
        rd('seq_level_idx[0]',5)
    else:
        ti=rd('timing_info_present',1)
        idd=rd('initial_display_delay_present_flag',1)
        cnt=rd('operating_points_cnt_minus_1',5)
        for i in range(cnt+1):
            rd(f'op_idc[{i}]',12); lvl=rd(f'seq_level_idx[{i}]',5)
            if lvl>7: rd(f'seq_tier[{i}]',1)
            if idd:
                pf=rd(f'idd_present_for_op[{i}]',1)
                if pf: rd(f'initial_display_delay_minus_1[{i}]',4)
    wb=rd('frame_width_bits_minus_1',4); hb=rd('frame_height_bits_minus_1',4)
    rd('max_frame_width_minus_1',wb+1); rd('max_frame_height_minus_1',hb+1)
    if not red: rd('frame_id_numbers_present',1)
    rd('use_128x128_superblock',1); rd('enable_filter_intra',1); rd('enable_intra_edge_filter',1)
    if not red:
        rd('enable_interintra_compound',1); rd('enable_masked_compound',1)
        rd('enable_warped_motion',1); rd('enable_dual_filter',1)
        oh=rd('enable_order_hint',1)
        if oh: rd('enable_jnt_comp',1); rd('enable_ref_frame_mvs',1)
        sc=rd('seq_choose_screen_content_tools',1)
        force_sc = 2 if sc else rd('seq_force_screen_content_tools',1)
        if force_sc>0:
            im=rd('seq_choose_integer_mv',1)
            if not im: rd('seq_force_integer_mv',1)
        if oh: rd('order_hint_bits_minus_1',3)
    rd('enable_superres',1); rd('enable_cdef',1); rd('enable_restoration',1)
    rd('high_bitdepth',1)
    rd('mono_chrome',1); rd('color_description_present',1)
    return o
def sh_of(path):
    for t,p in obus(open(path,'rb').read()):
        if t==1: return decode_sh(p)
    return []
a=sh_of(sys.argv[1]); b=sh_of(sys.argv[2])
print(f"{'field':42} {'C':>6} {'port':>6}")
for (na,va),(nb,vb) in zip(a,b):
    flag='' if (na==nb and va==vb) else '   <-- DIFFERS'
    print(f"{na:42} {va:>6} {vb:>6}{flag}")
