from pathlib import Path
import os,subprocess
root=Path('/home/lilith/tmp/svt-tracking/chroma-native-boundary/native10-prefilter');root.mkdir(exist_ok=True)
repo=Path('/home/lilith/work/zen/zenav1-svt/rust')
with (root/'rs.log').open('w') as log:
 subprocess.run([str(repo/'tools/identity_run'),'raw:/home/lilith/tmp/svt-tracking/preset-fill-worst-replay/measured.yuv','376','512','10','1',str(root/'rs')],env=dict(os.environ,SVTAV1_BD='10',SVTAV1_HBD_SRC='1',SVTAV1_RECON10_BIN=str(root/'rs')),stdout=log,stderr=log,check=True)
with (root/'c.log').open('w') as log:
 subprocess.run([str(repo/'tools/capture_c_trace/capture_c_trace'),'376','512','10','1',str(root/'rs.yuv'),str(root/'c.obu'),'10'],env=dict(os.environ,SVT_RECON_OUT=str(root/'c.sse'),SVT_RECON_BIN=str(root/'c')),stdout=log,stderr=log,check=True)
import array,json
results=[]
for plane,w in enumerate((376,188,188)):
 c=array.array('H');c.frombytes((root/f'c.p{plane}').read_bytes())
 r=array.array('H');r.frombytes((root/f'rs.p{plane}').read_bytes())
 assert len(c)==len(r)
 diff=[(i,x,y) for i,(x,y) in enumerate(zip(c,r)) if x!=y]
 row=dict(plane=plane,differing_samples=len(diff),first=[(i%w,i//w,x,y) for i,x,y in diff[:4]])
 results.append(row);print(row)
(root/'summary.json').write_text(json.dumps(results,indent=2)+'\n')
