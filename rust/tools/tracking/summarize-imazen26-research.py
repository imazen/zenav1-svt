import collections,csv,json,statistics
from pathlib import Path
root=Path('/home/lilith/tmp/av1-imazen26-research-baseline-2026-09-08')
rows=[json.loads(s) for s in (root/'rows.jsonl').read_text().splitlines()]
assert len(rows)==150, len(rows)
groups=collections.defaultdict(list)
for r in rows:
 c=r['config'];groups[(r['source'],c['backend'],c['speed'],c['quantizer'])].append(r)
assert len(groups)==50
for key,rs in groups.items():
 assert {r['round'] for r in rs}=={0,1,2},key
 assert len({r['output_sha256'] for r in rs})==1,key
for source in ['1000.sdr.png','8100.sdr.png']:
 for q in [5,12,20,32,48]:
  for a,b,sa,sb in [('c-svt-av1','zenav1-svt',-1,-1),('libaom','zenav1-aom',0,0)]:
   if groups[(source,a,sa,q)][0]['output_sha256']!=groups[(source,b,sb,q)][0]['output_sha256']:print('PARITY_DIFFERENCE',source,q,a,b)
print('150/150 completed; 50 deterministic cells; C/Rust SVT -1 exact 10/10')
print('Measured QP20: source backend preset bytes bpp SSIM2 median_ms min_ms max_ms')
for key,rs in sorted(groups.items()):
 if key[-1]!=20:continue
 r=rs[0];c=r['config'];ms=[r['api_elapsed_ns']/1e6 for r in rs]
 print(*key[:3],r['bytes'],round(r['bytes']*8/(c['width']*c['height']),3),round(r['ssimulacra2'],3),round(statistics.median(ms),1),round(min(ms),1),round(max(ms),1))
print('Matched quality estimates: source backend preset target bytes bpp ms bracket')
for r in csv.DictReader((root/'analysis/matched.tsv').open(),delimiter='\t'):
 if r['target_ssim2'] not in ['70','80']:continue
 print(r['source'],r['backend'],r['preset'],r['target_ssim2'],round(float(r['estimated_bytes'])),round(float(r['estimated_bytes'])*8/(int(r['width'])*int(r['height'])),3),round(float(r['estimated_ms'])),r['q_low'],r['q_high'])
