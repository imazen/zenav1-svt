#!/usr/bin/env python3
"""Profile prebuilt Rust and C still drivers on one named real-image cell.

Run under run-heavy. Whole-process samples include constructors and teardown;
use still_pairs.py for encode timing. Outputs must be byte-identical.
"""
import argparse
import csv
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import subprocess


def sha(path):
    h = hashlib.sha256()
    with Path(path).open('rb') as f:
        for block in iter(lambda: f.read(1024 * 1024), b''):
            h.update(block)
    return h.hexdigest()


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    for flag in ('port', 'reference', 'cells', 'out'):
        ap.add_argument('--' + flag, type=Path, required=True)
    ap.add_argument('--cell', required=True)
    ap.add_argument('--sudo-perf', action='store_true', help='run only perf record through sudo -n')
    ap.add_argument('--cpu', type=int, required=True)
    ap.add_argument('--warmups', type=int, required=True)
    ap.add_argument('--event', required=True)
    ap.add_argument('--period', type=int, default=200000)
    args = ap.parse_args()
    if args.warmups < 0 or args.period <= 0:
        ap.error('warmups must be nonnegative and period positive')
    cells = [c for c in csv.DictReader(args.cells.open(), delimiter='\t') if c['name'] == args.cell]
    if len(cells) != 1:
        ap.error('cell must match exactly one manifest row')
    cell = cells[0]
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=False)
    env = {k: v for k, v in os.environ.items() if not k.startswith(('SVT_', 'SVTAV1_'))}
    env['DEBUGINFOD_URLS'] = ''
    meta = {'cell': cell, 'cpu': args.cpu, 'warmups': args.warmups,
            'host': platform.node(), 'platform': platform.platform(),
            'event': args.event, 'period': args.period, 'call_graph': 'dwarf,8192',
            'scope': 'whole process, including constructors, teardown and logging',
            'sudo_perf': args.sudo_perf,
            'checkout_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
            'input_sha256': sha(cell['yuv']), 'arms': {}}

    def save():
        (out / 'meta.json').write_text(json.dumps(meta, indent=2) + '\n')

    for arm, binary in [('rust', args.port), ('c', args.reference)]:
        prefix = out / arm
        params = [cell[k] for k in ('width', 'height', 'qp', 'preset')]
        driver = ([str(binary.resolve()), 'raw:' + cell['yuv'], *params, str(prefix), str(args.warmups)]
                  if arm == 'rust' else [str(binary.resolve()), *params, cell['yuv'], str(prefix)+'.obu', str(args.warmups)])
        perf = Path(str(prefix) + '.perf')
        record = Path(str(prefix) + '.record.log')
        report = Path(str(prefix) + '.self.txt')
        command = ['perf', 'record', '-e', args.event, '-c', str(args.period),
                   '-g', '--call-graph', 'dwarf,8192', '-o', str(perf), '--',
                   'taskset', '-c', str(args.cpu), *driver]
        if args.sudo_perf:
            command = ['sudo', '-n', *command]
        meta['arms'][arm] = {'binary': str(binary.resolve()), 'binary_sha256': sha(binary), 'command': command}
        save()
        print(f'{arm}: recording {args.cell}', flush=True)
        with record.open('w') as log:
            subprocess.run(command, env=env, stdout=log, stderr=subprocess.STDOUT, check=True)
        if args.sudo_perf:
            subprocess.run(['sudo', '-n', 'chown', f'{os.getuid()}:{os.getgid()}', str(perf)], check=True)
        samples = re.search(r'\((\d+) samples\)', record.read_text())
        if not samples or int(samples[1]) == 0:
            raise RuntimeError('perf did not report nonzero captured samples')
        with report.open('w') as log:
            subprocess.run(['perf', 'report', '--stdio', '--no-children', '--percent-limit', '0.3',
                            '--sort', 'symbol', '-i', str(perf)], env=env,
                           stdout=log, stderr=subprocess.STDOUT, check=True)
        text = report.read_text()
        lost = re.search(r'Total Lost Samples:\s*(\d+)', text)
        if not lost or int(lost[1]) != 0:
            raise RuntimeError('lost-sample count is absent or nonzero')
        symbols = re.findall(r'^\s*([0-9.]+)%\s+\[\.\]\s+(.+?)\s+-\s+-\s*$', text, re.M)
        if not symbols:
            raise RuntimeError('no user-space symbols parsed from perf report')
        with Path(str(prefix)+'.self.tsv').open('w') as f:
            w = csv.writer(f, delimiter='\t', lineterminator='\n')
            w.writerow(['self_percent', 'symbol']); w.writerows(symbols)
        meta['arms'][arm].update(samples=int(samples[1]), lost_samples=int(lost[1]),
            files={str(p): {'sha256': sha(p), 'bytes': p.stat().st_size} for p in (perf, record, report)})
        save()
        print(f'{arm}: {samples[1]} samples, zero lost', flush=True)
    if (out/'rust.obu').read_bytes() != (out/'c.obu').read_bytes():
        raise RuntimeError('profiled outputs differ')
    meta['identical'] = True
    save()
    print('profiled outputs ident=Y', flush=True)


if __name__ == '__main__':
    main()
