#!/usr/bin/env python3
"""Live C grain gate: matched input/config, OBU identity and decoder output.

Run under scripts/run-heavy. All dumps stay in the requested scratch directory.
Existing INTER limitations remain in force; the two-frame probe explicitly uses
its established experimental switch and requires identity on that exact cell.
"""
from pathlib import Path
import hashlib, os, subprocess, sys
HERE=Path(__file__).resolve().parent
OUT=Path(sys.argv[1]).resolve();OUT.mkdir(parents=True,exist_ok=True)
DEC=os.environ.get('AOMDEC','aomdec')
def run(args,env,log):
    with log.open('wb') as f: subprocess.run([str(a) for a in args],env=env,stdout=f,stderr=subprocess.STDOUT,check=True)
def case(name,w,h,depth,knobs,frames=1,c_invalid=False):
    d=OUT/name;d.mkdir(exist_ok=True)
    env=dict(os.environ,SVTAV1_BD=str(depth),SVTAV1_FINAL_RECON=str(d/'recon'),**knobs)
    if frames>1:env.update(SVTAV1_FRAMES=str(frames),SVT_FRAMES=str(frames),SVTAV1_INTER_EXPERIMENTAL='1',SVTAV1_INTRA_PERIOD='64',SVTAV1_HIER_LEVELS='0',SVT_INTRA_PERIOD='-1',SVT_HIER_LEVELS='0',SVT_PRED_STRUCT='1')
    preset=8 if frames>1 else 10
    content='gradient' if frames>1 else 'grain'
    run([HERE/'identity_run',content,w,h,40,preset,d/'rs'],env,d/'rs.log')
    run([HERE/'capture_c_trace/capture_c_trace',w,h,40,preset,d/'rs.yuv',d/'c.obu',depth],env,d/'c.log')
    r=(d/'rs.obu').read_bytes();c=(d/'c.obu').read_bytes()
    if c_invalid:
        assert hashlib.sha256(c).hexdigest() == 'ea2bb35d34557b7b77299957744fafee7953876da8942224304592d0fb87f52f', f'{name}: C witness changed; review before updating'
        # Pinned C emits an undecodable stream for this non-mi-aligned
        # superres cell. Preserve the witness; require valid Rust output below.
        with (d/'c-decode.log').open('wb') as f:
            decoded=subprocess.run([DEC,'--i420','--rawvideo','-o',str(d/'c-decoded.yuv'),str(d/'c.obu')],stdout=f,stderr=f)
        assert decoded.returncode != 0 and b'Corrupt frame detected' in (d/'c-decode.log').read_bytes(), f'{name}: C failure expectation is stale'
    elif r!=c:
        subprocess.run([sys.executable,str(HERE/'fh_fields.py'),str(d/'c.obu'),str(d/'rs.obu')],stdout=(d/'fields.txt').open('w'))
        raise AssertionError(f'{name}: C {len(c)}B != Rust {len(r)}B; {d}')
    run([DEC,'--i420','--rawvideo',f'--output-bit-depth={depth}','-o',d/'decoded.yuv',d/'rs.obu'],env,d/'decode.log')
    recon=(d/'recon').read_bytes() if frames==1 else b''.join((d/f'recon.f{i}').read_bytes() for i in range(frames))
    assert (d/'decoded.yuv').read_bytes()==recon, f'{name}: decoder != grain reconstruction; {d}'
    run([DEC,'--i420','--rawvideo','--skip-film-grain',f'--output-bit-depth={depth}','-o',d/'clean.yuv',d/'rs.obu'],env,d/'decode-clean.log')
    changed=(d/'clean.yuv').read_bytes()!=recon
    if knobs.get('SVT_GRAIN_TABLE') == 'no_y':assert not changed, f'{name}: omitted chroma points affected output'
    elif knobs.get('SVT_GRAIN_TABLE'):assert changed, f'{name}: supplied grain had no output effect'
    print(f'PASS {name}: {len(r)}B, C={"decode failure witness" if c_invalid else "identical"}, recon decoder-exact, grain_live={changed}',flush=True)
    return changed
live=0
for depth in [8,10]:
    for adaptive in [0,1]:
        for apply in [0,1]:
            name=f'd{depth}_adaptive{adaptive}_apply{apply}'
            live+=case(name,128,128,depth,{'SVT_GRAIN_STRENGTH':'25','SVT_GRAIN_APPLY':str(apply),'SVT_GRAIN_ADAPTIVE':str(adaptive)})
    live+=case(f'table_d{depth}',128,128,depth,{'SVT_GRAIN_TABLE':'1','SVT_GRAIN_STRENGTH':'50','SVT_GRAIN_APPLY':'1'})
# Partial blocks, including non-SB and non-mi-aligned dimensions.
for w,h in [(64,64),(70,66),(136,72)]:
    live+=case(f'partial_{w}x{h}',w,h,8,{'SVT_GRAIN_STRENGTH':'25','SVT_GRAIN_APPLY':'1'})
for ignore in [False,True]:
    knobs={'SVT_GRAIN_TABLE':'1'}
    if ignore:knobs['SVT_GRAIN_IGNORE_REF']='1'
    name=f'inter_ignore{int(ignore)}';live+=case(name,64,64,8,knobs,2)
    run([sys.executable,HERE/'fh_fields.py','--index','1',OUT/name/'c.obu',OUT/name/'rs.obu'],os.environ,OUT/name/'inter-fields.txt')
    fields=(OUT/name/'inter-fields.txt').read_text()
    assert 'update_grain' in fields
    assert ('film_grain_params_ref_idx' in fields) != ignore, f'{name}: reuse branch not reached'
for w,h in [(70,66),(136,72)]:
    live+=case(f'partial10_{w}x{h}',w,h,10,{'SVT_GRAIN_STRENGTH':'25','SVT_GRAIN_APPLY':'1'})
live+=case('odd_table',71,67,8,{'SVT_GRAIN_TABLE':'1'})
for denom in [9,12,16]:
    for table in [False,True]:
        knobs={'SVT_GRAIN_STRENGTH':'25','SVT_GRAIN_APPLY':'1',
               'SVTAV1_SUPERRES':str(denom),'SVT_SUPERRES_KF_DENOM':str(denom)}
        if table:knobs['SVT_GRAIN_TABLE']='1'
        live+=case(f'superres{denom}_table{int(table)}',128,128,8,knobs)
live+=case('superres_partial_c_bug',70,66,8,{'SVT_GRAIN_STRENGTH':'25','SVT_GRAIN_APPLY':'1',
    'SVTAV1_SUPERRES':'12','SVT_SUPERRES_KF_DENOM':'12'},c_invalid=True)
for depth in [8,10]:
    for mode in ['cfl','no_y']:
        live+=case(f'table_{mode}_d{depth}',128,128,depth,{'SVT_GRAIN_TABLE':mode})
assert live>=27, f'not enough live grain cases: {live}'
print(f'PASS 29 stream cells; {live} show a grain-on/off pixel difference',flush=True)
