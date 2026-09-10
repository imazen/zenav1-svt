from pathlib import Path
import os,subprocess,json
root=Path('/home/lilith/tmp/svt-tracking/chroma-native-boundary')
for p,q in [(1,30),(4,10),(4,30),(5,10),(5,30)]:
 out=root/f'bd10-p{p}-q{q}'
 with out.with_suffix('.log').open('w') as log:
  r=subprocess.run(['tools/identity_diff.sh','376','512',str(q),str(p),'raw:/home/lilith/tmp/svt-tracking/preset-fill-worst-replay/measured.yuv',str(out)],cwd='/home/lilith/work/zen/zenav1-svt/rust',env=dict(os.environ,SVTAV1_BD='10',SVTAV1_HBD_SRC='1'),stdout=log,stderr=log)
 print(p,q,r.returncode,flush=True)
rows=[]
for bd in (8,10):
 for p in (-1,0,1,4,5):
  for q in (10,30):
   report=(root/f'bd{bd}-p{p}-q{q}'/'report.txt').read_text()
   rows.append(dict(bit_depth=bd,preset=p,qp=q,identical='VERDICT: IDENTICAL' in report,report=report))
summary=dict(total=len(rows),passed=sum(r['identical'] for r in rows),rows=rows)
(root/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
print('boundary',summary['passed'],'/',summary['total'],flush=True)
