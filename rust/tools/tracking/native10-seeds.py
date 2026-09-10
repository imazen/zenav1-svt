from pathlib import Path
import os,subprocess
root=Path('/home/lilith/tmp/svt-tracking/chroma-native-boundary/native10-seeds');root.mkdir(exist_ok=True)
repo=Path('/home/lilith/work/zen/zenav1-svt/rust')
with (root/'rs.log').open('w') as log:
 subprocess.run([str(repo/'tools/identity_run'),'raw:/home/lilith/tmp/svt-tracking/preset-fill-worst-replay/measured.yuv','376','512','10','1',str(root/'rs')],env=dict(os.environ,SVTAV1_BD='10',SVTAV1_HBD_SRC='1',SVTAV1_SEED_DUMP='1',SVTAV1_CHAIN_DUMP='1'),stdout=log,stderr=log,check=True)
with (root/'c.log').open('w') as log:
 subprocess.run([str(repo/'tools/capture_c_trace/capture_c_trace'),'376','512','10','1',str(root/'rs.yuv'),str(root/'c.obu'),'10'],env=dict(os.environ,SVT_SEED_OUT=str(root/'c.seed')),stdout=log,stderr=log,check=True)
