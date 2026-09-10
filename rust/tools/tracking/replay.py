import hashlib
import json
from pathlib import Path
import subprocess

root = Path(__file__).parent
repo = Path('/home/lilith/work/zen/zenav1-svt/rust')
jobs = json.loads((root / 'jobs.json').read_text())
results = []
for i, job in enumerate(jobs):
    cfg = job['config']
    assert cfg['bit_depth'] == 8 and cfg['chroma'] == '420'
    assert cfg['tune'] is None and cfg['scm'] is None
    assert cfg['threads'] == 1 and cfg['sb128'] is False
    dest = root / f'cell-{i:03}'
    dest.mkdir(exist_ok=True)
    args = [str(cfg[k]) for k in ('width', 'height', 'quantizer', 'speed')]
    with (dest / 'rust.log').open('w') as log:
        subprocess.run([str(repo / 'tools/identity_run'), 'raw:' + job['input'],
                        *args, str(dest / 'rust')], cwd=repo, stdout=log, stderr=log, check=True)
    with (dest / 'c.log').open('w') as log:
        subprocess.run([str(repo / 'tools/capture_c_trace/capture_c_trace'),
                        *args, job['input'], str(dest / 'c.obu'), '8'],
                       cwd=repo, stdout=log, stderr=log, check=True)
    c = (dest / 'c.obu').read_bytes()
    rust = (dest / 'rust.obu').read_bytes()
    row = dict(job, c_sha256=hashlib.sha256(c).hexdigest(),
               rust_sha256=hashlib.sha256(rust).hexdigest(),
               c_bytes=len(c), rust_bytes=len(rust), identical=c == rust)
    row['reference_unchanged'] = row['c_sha256'] == job['expected_c_sha256']
    results.append(row)
    (root / 'results.json').write_text(json.dumps(results, indent=2) + '\n')
    if not row['identical'] or not row['reference_unchanged']:
        print('FAIL', i, cfg, flush=True)
    elif (i + 1) % 20 == 0:
        print('checked', i + 1, flush=True)
summary = dict(total=len(results), identical=sum(r['identical'] for r in results),
               reference_unchanged=sum(r['reference_unchanged'] for r in results),
               formerly_different=sum(r['expected_c_sha256'] != r['old_rust_sha256'] for r in results),
               corrected=sum(r['identical'] and r['expected_c_sha256'] != r['old_rust_sha256'] for r in results))
(root / 'summary.json').write_text(json.dumps(summary, indent=2) + '\n')
print(json.dumps(summary), flush=True)
raise SystemExit(0 if summary['identical'] == len(jobs) and summary['reference_unchanged'] == len(jobs) else 1)
