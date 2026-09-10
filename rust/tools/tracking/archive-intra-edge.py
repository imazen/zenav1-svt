from pathlib import Path
import hashlib, json, subprocess, sys, shutil, tarfile
import boto3
from boto3.s3.transfer import TransferConfig
sys.path.insert(0, '/home/lilith/work/zen/zenmetrics/scripts/lib')
from zen_s3env import resolve_full
root=Path('/home/lilith/tmp/av1-imazen26-intra-edge-corrected-2026-09-08')
provenance=json.loads((root/'provenance.json').read_text())
versions={'zenav1-svt':'8656f2b9','zenmetrics':'f9a5a8fa','zenav1-aom':'HEAD','zenrav1e':'HEAD'}
def sha(data):return hashlib.sha256(data).hexdigest()
source_dir=root/'measured-source';source_dir.mkdir(exist_ok=True)
counts={}
for name,record in provenance['repositories'].items():
 repo=Path('/home/lilith/work/zen')/name;count=0
 for name_in_repo,expected in record['source_files'].items():
  target=source_dir/name/name_in_repo
  source=repo/name_in_repo
  data=source.read_bytes() if source.is_file() else b''
  if sha(data)!=expected:
   data=subprocess.check_output(['git','show',f'{versions[name]}:{name_in_repo}'],cwd=repo)
  if sha(data)!=expected:raise RuntimeError(f'cannot recover measured source {name}/{name_in_repo}')
  target.parent.mkdir(parents=True,exist_ok=True);target.write_bytes(data);count+=1
 counts[name]=count
(root/'measured-source-manifest.json').write_text(json.dumps({'matched_recorded_hashes':counts,'recovery_candidates':versions},indent=2)+'\n')
# Preserve the later verification harness separately: its output must reproduce
# the measured input and OBU hashes before comparing all decoded samples.
verifier=Path('/home/lilith/work/zen/zenmetrics/benchmarks/av1-compare')
shutil.copyfile(verifier/'target/x86_64-unknown-linux-gnu/release/zenmetrics-av1-compare',root/'reconstruction-verifier')
measured_hashes={json.loads(line)['binary_sha256'] for line in (root/'rows.jsonl').read_text().splitlines()}
verified_hashes={json.loads(line)['verifier_binary_sha256'] for line in (root/'reconstruction-verification.jsonl').read_text().splitlines()}
assert measured_hashes=={sha((root/'measured-comparator').read_bytes())}
assert verified_hashes=={sha((root/'reconstruction-verifier').read_bytes())}
(root/'archival-toolchain.json').write_text(json.dumps({
 'note':'Toolchain queried at archival; timed/replay binary hashes identify actual executables.',
 'rustc':subprocess.check_output(['rustc','-Vv'],text=True),
 'cargo':subprocess.check_output(['cargo','-V'],text=True),
 'cc':subprocess.check_output(['cc','--version'],text=True),
 'target':'x86_64-unknown-linux-gnu', 'profile':'release',
 'measured_build_command':'cargo build --manifest-path ../zenmetrics/benchmarks/av1-compare/Cargo.toml --release --target x86_64-unknown-linux-gnu --bin zenmetrics-av1-compare -j 4',
 'timed_environment':{'RAYON_NUM_THREADS':'1','OMP_NUM_THREADS':'1','run-heavy jobs':1}
},indent=2)+'\n')

verifier_dir=root/'verifier-source';verifier_dir.mkdir(exist_ok=True)
shutil.copytree(verifier/'src',verifier_dir/'src',dirs_exist_ok=True)
for name in ['Cargo.toml','Cargo.lock','build.rs','analyze.py','time_budget.py']:
 shutil.copyfile(verifier/name,verifier_dir/name)
source_images=root/'canonical-source';source_images.mkdir(exist_ok=True)
for source in provenance['request']['inputs']:
 source=Path(source);shutil.copyfile(source,source_images/source.name)
shutil.copyfile('/home/lilith/tmp/svt-tracking/imazen26-smoke/manifest.json',source_images/'manifest.json')
logs=root/'validation-logs';logs.mkdir(exist_ok=True)
for name in ['imazen26-intra-edge-corrected.log','zen-edge-measurement-recon.log','zen-edge-replay-tests.log','zen-edge-replay-build.log','zen-edge-owner-final-nextest.log','zen-edge-owner-spotcheck.log','zen-edge-hybrid-full.log','zen-edge-mainline-full.log','zen-edge-mainline-research.log']:
 shutil.copyfile(Path('/home/lilith/tmp/svt-tracking')/name,logs/name)
# Keep exact pinned C source revisions for rebuilding the static adapters.
for name,repo in [('libaom',Path('/home/lilith/work/zen/zenav1-aom/upstream')),('c-svt',Path('/home/lilith/work/zen/zenav1-svt/reference/svt-av1'))]:
 rev=subprocess.check_output(['git','rev-parse','HEAD'],cwd=repo,text=True).strip()
 if subprocess.run(['git','diff','--quiet','HEAD','--'],cwd=repo).returncode:raise RuntimeError(f'dirty C source {name}')
 with (source_dir/f'{name}-{rev}.tar').open('wb') as target:
  subprocess.run(['git','archive','--format=tar','HEAD'],cwd=repo,stdout=target,check=True)
pristine=Path('/home/lilith/tmp/svt-tracking/pristine-v4.2.0/source/Bin/Release/SvtAv1EncApp')
shutil.copyfile(pristine,source_dir/'pristine-SvtAv1EncApp')
pristine_provenance=json.loads(Path('/home/lilith/tmp/svt-tracking/zen-edge-mainline-full/provenance.json').read_text())
assert sha(pristine.read_bytes())==pristine_provenance['app_sha256']
with (source_dir/'pristine-svt-9292ec8e.tar').open('wb') as target:
 subprocess.run(['git','archive','--format=tar','9292ec8e32bce26f781f277ec8739b53426c4300'],cwd='/home/lilith/work/zen/zenav1-svt/reference/svt-av1',stdout=target,check=True)
archive=Path('/home/lilith/tmp/svt-tracking/intra-edge-corrected-evidence.tar.gz')
with tarfile.open(archive,'w:gz',compresslevel=3) as tar:
 tar.add(root,arcname='intra-edge-corrected')
 for name in ['zen-edge-hybrid-full','zen-edge-mainline-full','zen-edge-mainline-research']:
  tar.add(Path('/home/lilith/tmp/svt-tracking')/name,arcname='parity/'+name)
 tar.add(Path('/home/lilith/tmp/svt-tracking/zen-edge-hybrid-full.tsv'),arcname='parity/hybrid-full.tsv')
 tar.add(Path('/home/lilith/tmp/av1-imazen26-intra-edge-2026-09-08'),arcname='superseded-pre-fix-measurement')
digest=sha(archive.read_bytes());key=f'benchmarks/av1-compare/2026-09-08/intra-edge-corrected/evidence-{digest}.tar.gz'
ep,ak,sk,kind,reach=resolve_full();client=boto3.client('s3',endpoint_url=ep,aws_access_key_id=ak,aws_secret_access_key=sk,region_name='auto')
client.head_bucket(Bucket='zentrain')
client.upload_file(str(archive),'zentrain',key,ExtraArgs={'Metadata':{'sha256':digest}},Config=TransferConfig(max_concurrency=1,use_threads=False))
remote=client.get_object(Bucket='zentrain',Key=key)['Body'];check=hashlib.sha256()
for part in iter(lambda:remote.read(1024*1024),b''):check.update(part)
assert check.hexdigest()==digest,'uploaded bytes differ'
manifest={'store_kind':kind,'bucket':'zentrain','key':key,'uri':'s3://zentrain/'+key,'bytes':archive.stat().st_size,'sha256':digest,'download_sha256_verified':True,'measured_source_files':counts}
(root/'artifact-location.json').write_text(json.dumps(manifest,indent=2)+'\n')
print(json.dumps(manifest))
