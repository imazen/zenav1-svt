from pathlib import Path
import subprocess,json,hashlib
root=Path('/home/lilith/tmp/svt-tracking/pristine-v4.2.0')
d=Path('/home/lilith/work/zen/zenav1-svt/rust/svtav1/tests/fixtures/reference_chroma')
rows=[]
for depth in [8,10]:
 for preset in [-1,0]:
  for name,app in [('mainline',root/'capture-mainline'),('hybrid',root/'capture-hybrid-nowrap')]:
   src=d/f'diag64-{depth}.yuv';out=d/f'diag64-{depth}-p{preset}-{name}.obu'
   argv=[str(app),'64','64','48',str(preset),str(src),str(out),str(depth)]
   with (root/f'fixture-{depth}-{preset}-{name}.log').open('wb') as log:
    subprocess.run(argv,stdout=log,stderr=log,check=True,timeout=180)
   rows.append({'depth':depth,'preset':preset,'qp':48,'reference':name,'argv':argv,'input_sha256':hashlib.sha256(src.read_bytes()).hexdigest(),'output_sha256':hashlib.sha256(out.read_bytes()).hexdigest(),'bytes':out.stat().st_size,'app_sha256':hashlib.sha256(app.read_bytes()).hexdigest()})
(d/'provenance.json').write_text(json.dumps({'mainline_revision':'9292ec8e32bce26f781f277ec8739b53426c4300','hybrid_revision':'3115c0c1b23e860dfd75c94f6740e0298182dd13','hybrid_hdr_mode':0,'input_description':'identity_run diag64; native10 = (sample8 << 2) + ((sample_index * 3) % 4)','rows':rows},indent=2)+'\n')
for r in rows: print(r['depth'],r['preset'],r['reference'],r['bytes'])
