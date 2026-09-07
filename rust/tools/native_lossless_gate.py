#!/usr/bin/env python3
"""Native QP0: C byte parity for color, source parity for color/monochrome.

Run under run-heavy; pass an artifact directory as the sole argument.
All cells retain their encoded streams and source/decoder samples.
"""
import os
from pathlib import Path
import subprocess
import sys


def run(args, env, log):
    with log.open('wb') as output:
        return subprocess.run(args, env=env, stdout=output, stderr=output).returncode


def main():
    if len(sys.argv) != 2:
        raise SystemExit('usage: native_lossless_gate.py <artifact-directory>')
    tools = Path(__file__).resolve().parent
    artifacts = Path(sys.argv[1]).resolve()
    artifacts.mkdir(parents=True, exist_ok=True)
    total = exact = 0
    differences = []
    for mono in (False, True):
        for w, h, tile in ((64, 64, 0), (96, 80, 0), (128, 128, 1)):
            for content in ('gradient', 'diag', 'uniform', 'screen'):
                for preset in range(14):
                    cell = f'{"mono" if mono else "color"}-{content}-{w}x{h}-t{tile}-p{preset}'
                    out = artifacts / cell
                    out.mkdir(exist_ok=True)
                    prefix = out / 'rs'
                    env = dict(os.environ, SVTAV1_BD='10', SVTAV1_HBD_SRC='1',
                               SVTAV1_TILE_ROWS_LOG2=str(tile), SVTAV1_TILE_COLS_LOG2=str(tile))
                    # Do not let an ambient mono flag turn the color gate into
                    # a second monochrome comparison.
                    env.pop('SVTAV1_MONO', None)
                    if mono:
                        env['SVTAV1_MONO'] = '1'
                    rc = run([str(tools / 'identity_run'), content, str(w), str(h),
                              '0', str(preset), str(prefix)], env, out / 'rs.log')
                    if rc:
                        raise SystemExit(f'{cell}: port error {rc}; artifacts {out}')
                    rc = run([os.environ.get('AOMDEC', 'aomdec'), '--rawvideo',
                              '--output-bit-depth=10', '-o', str(out / 'dec.yuv'),
                              str(out / 'rs.obu')], os.environ, out / 'dec.log')
                    source = (out / 'rs.yuv').read_bytes()
                    if mono:
                        source = source[:w * h * 2]
                    if not any(value & 3 for value in source[::2]):
                        raise SystemExit(f'{cell}: native low-bit premise failed; artifacts {out}')
                    if rc or (out / 'dec.yuv').read_bytes() != source:
                        raise SystemExit(f'{cell}: SOURCE PIXEL FAILURE; artifacts {out}')
                    total += 1
                    if not mono:
                        env_c = dict(os.environ, SVT_TRACE_OUT='/dev/null',
                                     SVT_TILE_ROWS=str(tile), SVT_TILE_COLUMNS=str(tile))
                        rc = run([str(tools / 'capture_c_trace/capture_c_trace'),
                                  str(w), str(h), '0', str(preset), str(out / 'rs.yuv'),
                                  str(out / 'c.obu'), '10'], env_c, out / 'c.log')
                        if rc:
                            raise SystemExit(f'{cell}: C error {rc}; artifacts {out}')
                        if (out / 'rs.obu').read_bytes() == (out / 'c.obu').read_bytes():
                            exact += 1
                        else:
                            differences.append(cell)
                            print('BYTE_DIFF', cell, flush=True)
                print('DONE', 'mono' if mono else 'color', content, w, h, tile,
                      'source', total, 'C exact', exact, flush=True)
    print('TOTAL SOURCE', total, 'C EXACT', exact, 'DIFFERENCES', differences, flush=True)
    if differences:
        raise SystemExit(1)


if __name__ == '__main__':
    main()
