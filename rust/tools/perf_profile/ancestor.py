#!/usr/bin/env python3
"""Nearest-named-ancestor attribution of one symbol family's SELF samples.

`selftime.py` answers "which symbol burned the time". This answers "who CALLED
it" — which matters whenever one leaf kernel serves several stages and the
question is which stage to attack. `docs/perf-status.md`'s 2026-08-13 allocator
ranking was produced this way by hand; this is that method as a script.

Usage:
    ancestor.py <sample.txt> <target-regex> <ancestor-regex> <arm_ms> <arm_samples>

It walks /usr/bin/sample's call graph, reconstructs the stack from the
indentation column, derives each node's SELF count as (its count - the sum of
its children's), and for every self sample on a node matching <target-regex>
credits it to the nearest enclosing node matching <ancestor-regex>. Symbols are
demangled through `rustfilt` first, so both regexes are written against
demangled names.

<arm_ms> and <arm_samples> convert shares to milliseconds: the profile gives
shares, the paired gate (benchmarks/perf_*.tsv) gives the scale. Pass the paired
per-encode ms for the cell and the profile's own total sample count.

ALWAYS RUN A CONTROL ARM. A regex that matches nothing reports nothing, and a
regex that matches everything reports the driver. The 2026-09-02 inter run used
the videokey arm (a video-mode key frame, no inter frame) as the control: it
returned 0.09 ms against the inter arm's 2.93, which is what makes the
difference attributable to the inter frame rather than to the regex.
"""
import re
import subprocess
import sys
from collections import defaultdict

LINE = re.compile(r'^(?P<pre>[ +!:|]*)(?P<cnt>\d+) (?P<rest>\S.*)$')
SYM = re.compile(r'^(?P<sym>.*?)\s+\(in (?P<bin>[^)]*)\)')


def rows(path):
    lines = open(path, errors='replace').read().split('\n')
    start = next(i for i, l in enumerate(lines) if l.startswith('Call graph:'))
    for l in lines[start + 1:]:
        m = LINE.match(l)
        if not m:
            continue
        s = SYM.match(m.group('rest'))
        sym = s.group('sym') if s else m.group('rest').split('  ')[0]
        yield len(m.group('pre')), int(m.group('cnt')), sym


def demangle(names):
    p = subprocess.run(['rustfilt'], input='\n'.join(names), capture_output=True, text=True)
    return p.stdout.split('\n')


def attribute(path, target_re, anc_re):
    raw = list(rows(path))
    syms = demangle([s for _, _, s in raw])
    ev = [(d, c, syms[i]) for i, (d, c, _) in enumerate(raw)]

    child = defaultdict(int)
    stack = []
    for i, (d, c, _s) in enumerate(ev):
        while stack and stack[-1][0] >= d:
            stack.pop()
        if stack:
            child[stack[-1][1]] += c
        stack.append((d, i))

    tgt, anc = re.compile(target_re), re.compile(anc_re)
    out, total = defaultdict(int), 0
    stack = []
    for i, (d, c, s) in enumerate(ev):
        while stack and stack[-1][0] >= d:
            stack.pop()
        self_c = c - child[i]
        if self_c > 0 and tgt.search(s):
            a = 'ROOT/unattributed'
            for _dd, j in reversed(stack):
                if anc.search(ev[j][2]):
                    a = ev[j][2]
                    break
            out[a] += self_c
            total += self_c
        stack.append((d, i))
    return out, total


if __name__ == '__main__':
    if len(sys.argv) != 6:
        print(__doc__)
        sys.exit(2)
    path, target_re, anc_re = sys.argv[1], sys.argv[2], sys.argv[3]
    arm_ms, arm_samples = float(sys.argv[4]), int(sys.argv[5])
    out, total = attribute(path, target_re, anc_re)
    print(f"# {path}")
    print(f"# {total} self samples matching /{target_re}/ "
          f"= {total / arm_samples * arm_ms:.3f} ms of the {arm_ms} ms arm")
    for k, v in sorted(out.items(), key=lambda kv: -kv[1]):
        print(f"{v:7d}  {100 * v / total:5.1f}%  {v / arm_samples * arm_ms:8.3f} ms  {k}")
