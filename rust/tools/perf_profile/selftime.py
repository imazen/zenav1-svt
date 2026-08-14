#!/usr/bin/env python3
"""Parse /usr/bin/sample call-graph output into per-symbol SELF sample counts.

Line shape:
    <prefix chars> <count> <symbol>  (in <binary>) + <off>  [<addr>]  [file:line]
The prefix is a mix of spaces and the '+ ! : |' tree-drawing characters; depth is
determined by the column at which the count begins.
"""
import re
import sys
from collections import defaultdict

LINE = re.compile(r'^(?P<pre>[ +!:|]*)(?P<cnt>\d+) (?P<rest>\S.*)$')
SYM = re.compile(r'^(?P<sym>.*?)\s+\(in (?P<bin>[^)]*)\)')


def parse(path):
    lines = open(path, errors='replace').read().split('\n')
    try:
        start = next(i for i, l in enumerate(lines) if l.startswith('Call graph:'))
    except StopIteration:
        sys.exit('no call graph in ' + path)
    end = next(i for i, l in enumerate(lines)
               if i > start and l.startswith('Total number in stack'))
    stack = []            # list of (col, idx)
    nodes = []            # (sym, bin, count, children_total)
    total_root = 0
    for l in lines[start + 1:end]:
        m = LINE.match(l)
        if not m:
            continue
        col = len(m.group('pre'))
        cnt = int(m.group('cnt'))
        rest = m.group('rest')
        ms = SYM.match(rest)
        if ms:
            sym, binr = ms.group('sym').strip(), ms.group('bin')
        else:
            sym, binr = rest.split('  ')[0].strip(), '?'
        while stack and stack[-1][0] >= col:
            stack.pop()
        # `sample` prints <deduplicated_symbol> when an address maps to several
        # linker-folded (ICF) names. Attribute those samples to the PARENT frame
        # so they land in the right functional class instead of a black hole.
        if sym == '<deduplicated_symbol>' and stack:
            sym = 'dedupof_' + nodes[stack[-1][1]][0]
        idx = len(nodes)
        nodes.append([sym, binr, cnt, 0])
        if stack:
            nodes[stack[-1][1]][3] += cnt
        else:
            total_root += cnt
        stack.append((col, idx))
    self_by_sym = defaultdict(int)
    for sym, binr, cnt, ch in nodes:
        s = cnt - ch
        if s < 0:
            s = 0
        self_by_sym[(sym, binr)] += s
    return self_by_sym, total_root


if __name__ == '__main__':
    sb, tot = parse(sys.argv[1])
    print(f'# total root samples {tot}  sum-self {sum(sb.values())}')
    for (sym, binr), v in sorted(sb.items(), key=lambda kv: -kv[1]):
        if v == 0:
            continue
        print(f'{v}\t{binr}\t{sym}')
