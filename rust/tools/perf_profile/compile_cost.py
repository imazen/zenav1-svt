#!/usr/bin/env python3
"""Measure marginal Archmage API compile cost; run under run-heavy.

--snapshots contains archmage-base, archmage-pr96 and archmage-pr97 trees.
The recorded experiment used 1bfc3c5b, 9981d4f9 and 43f8baed respectively.
"""
import argparse
import csv
import json
import os
from pathlib import Path
import random
import subprocess
import time

ap = argparse.ArgumentParser(description=__doc__)
ap.add_argument('--snapshots', type=Path, required=True)
ap.add_argument('--out', type=Path, required=True)
args = ap.parse_args()
root = args.snapshots.resolve()
work = args.out.resolve()
work.mkdir(exist_ok=False)
env = {k: v for k, v in os.environ.items() if not k.startswith('CARGO_PROFILE_')}
env['CARGO_INCREMENTAL'] = '0'
for key in ('RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'CARGO_TARGET_DIR', 'CARGO_BUILD_TARGET', 'RUSTC_WRAPPER'):
    env.pop(key, None)
source = '''#![forbid(unsafe_code)]
use archmage::prelude::*;
use magetypes::simd::generic::f32x8;
#[arcane]
pub fn consumer(token: X64V3Token, values: &[f32;8]) -> f32 {
    let v = f32x8::<X64V3Token>::load(token, values);
    (v * v).reduce_add()
}
'''
jobs = []
for revision in ('base', 'pr96', 'pr97'):
    for wide in (False, True):
        name = f'{revision}-w512-{int(wide)}'
        directory = work / name
        (directory / 'src').mkdir(parents=True)
        (directory / 'src/lib.rs').write_text(source)
        dependency = root / f'archmage-{revision}'
        features = '["std", "w512"]' if wide else '["std"]'
        (directory / 'Cargo.toml').write_text(f'''[workspace]
[package]
name="compile_cost_probe"
version="0.0.0"
edition="2024"
[dependencies]
archmage={{path="{dependency}",default-features=false,features=["std"]}}
magetypes={{path="{dependency}/magetypes",default-features=false,features={features}}}
''')
        for mode in ('check', 'build'):
            jobs.append((name, directory, mode))

def run(command, directory, log):
    result = subprocess.run(command, cwd=directory, env=env, stdout=subprocess.PIPE,
                            stderr=log, text=True, check=True)
    log.write(result.stdout)
    return result.stdout

with (work / 'build.log').open('w') as log, (work / 'raw.tsv').open('x') as out:
    writer = csv.writer(out, delimiter='\t')
    writer.writerow(['variant', 'mode', 'round', 'elapsed_s'])
    # Warm dependency/network/filesystem state first; these are not samples.
    for name, directory, mode in jobs:
        run(['cargo', mode, '--release', '-j', '4'], directory, log)
    rng = random.Random(20260906)
    for trial in range(5):
        rng.shuffle(jobs)
        for name, directory, mode in jobs:
            # Rebuild both edited libraries and their consumer; proc-macro
            # dependencies stay cached. This measures marginal API cost.
            run(['cargo', 'clean', '--release', '-p', 'archmage', '-p', 'magetypes', '-p', 'compile_cost_probe'], directory, log)
            start = time.perf_counter()
            output = run(['cargo', mode, '--release', '--offline', '-j', '4', '--message-format=json'], directory, log)
            elapsed = time.perf_counter() - start
            artifacts = [json.loads(line) for line in output.splitlines()]
            rebuilt = {a['target']['name'] for a in artifacts
                       if a.get('reason') == 'compiler-artifact' and not a['fresh']}
            assert {'archmage', 'magetypes', 'compile_cost_probe'} <= rebuilt, rebuilt
            writer.writerow([name, mode, trial, elapsed]); out.flush()
            print(name, mode, trial, f'{elapsed:.3f}s', flush=True)
(work / 'method.json').write_text(json.dumps({'rounds': 5, 'seed': 20260906,
    'scope': 'archmage, magetypes and identical old-API consumer rebuilt; other dependencies warm',
    'target_cpu': 'baseline, runtime X64V3 token', 'incremental': False,
    'source': source}, indent=2) + '\n')
