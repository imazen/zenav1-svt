from pathlib import Path
import hashlib,json,subprocess,shutil
root=Path('/home/lilith/tmp/svt-tracking/pristine-v4.2.0')
output=root/'mainline-decode';output.mkdir(exist_ok=False)
seen=set();rows=[];failures=0;covered=0
for suite in ['rust-mainline-full','rust-mainline-research']:
 for line in (root/suite/'results.jsonl').read_text().splitlines():
  row=json.loads(line);assert row['verdict']=='IDENTICAL'
  covered+=1;key=row['rust_sha256']
  if key in seen: continue
  seen.add(key);s=row['settings'];w=int(s['width']);h=int(s['height']);depth=int(s['bit_depth'])
  src=root/suite/row['cell']/'rs.obu';decoded=output/'current.yuv'
  if decoded.exists(): decoded.unlink()
  argv=['aomdec','--rawvideo','--output-bit-depth=0','-o',str(decoded),str(src)]
  run=subprocess.run(argv,stdout=subprocess.PIPE,stderr=subprocess.PIPE,timeout=180)
  want=(w*h+2*((w+1)//2)*((h+1)//2))*(2 if depth==10 else 1)
  got=decoded.stat().st_size if decoded.exists() else -1
  ok=run.returncode==0 and got==want
  rows.append({'suite':suite,'cell':row['cell'],'obu_sha256':key,'argv':argv,'exit_code':run.returncode,'expected_decoded_bytes':want,'decoded_bytes':got,'pass':ok})
  if not ok:
   failures+=1;(output/(key+'.log')).write_bytes(run.stderr)
   if decoded.exists():decoded.rename(output/(key+'.yuv'))
  elif decoded.exists(): decoded.unlink()
summary={'cells_covered_by_byte_identity':covered,'unique_streams_decoded':len(seen),'failures':failures,'decoder_sha256':hashlib.sha256(Path(shutil.which('aomdec')).read_bytes()).hexdigest()}
(output/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
(output/'results.jsonl').write_text(''.join(json.dumps(r,sort_keys=True)+'\n' for r in rows))
print(json.dumps(summary));raise SystemExit(bool(failures))
