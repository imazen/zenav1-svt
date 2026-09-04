#!/usr/bin/env python3
"""Per-block MDS1/MDS3 admission join: C `SVT_FULLCOST_OUT` (SVT_FULLCOST_XY=all)
against the port's `SVTAV1_CANDDBG=1 SVTAV1_NSQDBG=1` stderr trace.

C side, one `CFULL` line per (candidate, MD stage) with the per-class stage
counts `n0..n3`, `m1bc` and `pm1` (= ctx->perform_mds1); `sl=` is the slice
type (I_SLICE == 1). Port side, one `NSQDBG PMDS1` line per MDS1 candidate and
one `NSQDBG CAND` line per MDS3 candidate. Blocks are keyed on the pixel origin
and the block dims; the port prints `mi=(y/4, x/4)`, C prints `org=(x,y)`.

Usage: mds3_admission_join.py <c.fullcost> <port.trace> [--slice N] [--verbose]

Prints a summary (blocks joined, MDS1/MDS3 count agreement, identical admitted
sets) and every block whose MDS3 admitted set differs, as
  <x>,<y> <w>x<h>  C st1=<n> st3=<n> n1tot=<n> pm1=<v> | port mds1=<n> mds3=<n>
  followed by the two candidate sets (mode,fi,delta,uv).

Exit status 0 when every joined block's MDS3 set is identical, 1 otherwise.
The join is blind to candidates the port does not print (no std feature, or an
SVTAV1_NSQDBG SB pin) -- run without a pin, and check the totals line first.
"""
import re
import sys

C_RE = re.compile(
    r'^CFULL sl=(?P<sl>\d+) org=\((?P<x>\d+),(?P<y>\d+)\) (?P<w>\d+)x(?P<h>\d+) st=(?P<st>\d+) '
    r'mode=(?P<mode>-?\d+) fi=(?P<fi>-?\d+) ang=(?P<ang>-?\d+) uv=(?P<uv>-?\d+) ibc=(?P<ibc>\d+) '
    r'ycb=\d+ ydist=(?P<ydist>\d+) cost=(?P<cost>\d+) cls=(?P<cls>\d+) n0=(?P<n0>[\d,]+) n1=(?P<n1>[\d,]+) '
    r'n2=(?P<n2>[\d,]+) n3=(?P<n3>[\d,]+) m1bc=(?P<m1bc>-?\d+)'
    r'(?: pm1=(?P<pm1>\d+))?(?: sq=(?P<sq>\d+) mds=(?P<mds>\d+))?')
P1_RE = re.compile(r'^NSQDBG PMDS1 mi=\((\d+),(\d+)\) (\d+)x(\d+) mode=(-?\d+) fi=(-?\d+) delta=(-?\d+) uv=(-?\d+) coeff_rate=\d+ dist=(\d+) full=(\d+)')
P3_RE = re.compile(r'^NSQDBG CAND mi=\((\d+),(\d+)\) (\d+)x(\d+) ci=\d+ mode=(-?\d+) fi=(-?\d+) delta=(-?\d+) uv=(-?\d+) ibc=\d+ txd=\d+ enddepth=\d+ flr=\d+ fcr=\d+ coeff_rate=\d+ dist=(\d+) full=(\d+)')

# C's filter_intra_mode is FILTER_INTRA_MODES (5) when filter-intra is off; the
# port prints fi=0 for off and 1..5 for the five modes? No: the port's `fi` is
# the C enum value when on. Compare (mode, fi_on, delta) with fi normalised:
# treat C fi==5 and port fi==5 as "off" identically -- both print the enum.


def parse_c(path, want_slice):
    """One record per VISIT of a block. C visits the same origin+dims once as
    its own square and again as a sub-block of each AB shape of the parent
    square (different neighbour context, different candidate costs), so the
    rows are grouped into visits: a maximal run of consecutive rows on one
    (origin, dims) [and one (sq, mds) when the dump carries them], split
    wherever an st=1 row follows an st=3 row. `blocks[key]` is the list of
    visits; each visit carries the st1/st3 candidate lists, n1tot, pm1, sq."""
    blocks = {}
    order_seq = []
    cur_key = None
    cur = None
    last_st = 0
    for line in open(path, errors='replace'):
        m = C_RE.match(line)
        if not m:
            continue
        sl = int(m.group('sl'))
        if want_slice is not None and sl != want_slice:
            continue
        x, y, w, h = (int(m.group(k)) for k in ('x', 'y', 'w', 'h'))
        st = int(m.group('st'))
        cand = (int(m.group('mode')), int(m.group('fi')), int(m.group('ang')), int(m.group('uv')))
        n1 = sum(int(v) for v in m.group('n1').split(','))
        pm1 = m.group('pm1')
        sq = m.group('sq')
        mds = m.group('mds')
        key = (x, y, w, h)
        vkey = (key, sq, mds)
        if vkey != cur_key or (st == 1 and last_st == 3):
            cur = {'st1': [], 'st2': [], 'st3': [], 'n1tot': n1, 'pm1': pm1, 'sq': sq, 'mds': mds,
                   'ord': len(order_seq), 'cost1': {}, 'cost3': {}}
            order_seq.append(key)
            blocks.setdefault(key, []).append(cur)
            cur_key = vkey
        last_st = st
        cur['n1tot'] = n1
        cur['pm1'] = pm1
        cost = (int(m.group('ydist')), int(m.group('cost')))
        if st == 1:
            cur['st1'].append(cand)
            cur['cost1'].setdefault(cand[:3], cost)
        elif st == 2:
            cur['st2'].append(cand)
        elif st == 3:
            cur['st3'].append(cand)
            cur['cost3'].setdefault(cand[:3], cost)
    return blocks


def parse_port(path):
    blocks = {}
    for line in open(path, errors='replace'):
        m = P1_RE.match(line)
        key = 'mds1'
        if not m:
            m = P3_RE.match(line)
            key = 'mds3'
        if not m:
            continue
        my, mx, w, h = (int(m.group(i)) for i in (1, 2, 3, 4))
        cand = (int(m.group(5)), int(m.group(6)), int(m.group(7)), int(m.group(8)))
        b = blocks.setdefault((mx * 4, my * 4, w, h), {'mds1': [], 'mds3': [], 'cost1': {}, 'cost3': {}})
        b[key].append(cand)
        b['cost1' if key == 'mds1' else 'cost3'].setdefault(cand[:3], (int(m.group(9)), int(m.group(10))))
    return blocks


def main():
    args = [a for a in sys.argv[1:] if not a.startswith('--')]
    want_slice = None
    if '--slice' in sys.argv:
        want_slice = int(sys.argv[sys.argv.index('--slice') + 1])
        args = [a for a in args if a != str(want_slice)]
    verbose = '--verbose' in sys.argv
    first_n = 2
    if '--first' in sys.argv:
        first_n = int(sys.argv[sys.argv.index('--first') + 1])
        args = [a for a in args if a != str(first_n)]
    c = parse_c(args[0], want_slice)
    p = parse_port(args[1])
    keys = sorted(set(c) | set(p))
    joined = c_only = p_only = 0
    mds1_eq = mds3_eq = set3_eq = 0
    c_st1 = c_st3 = p_m1 = p_m3 = 0
    c_visits = c_sq_visits = c_ab_visits = 0
    pm1_zero = pm1_one = 0
    diffs = []
    for k in keys:
        cv, pb = c.get(k), p.get(k)
        if cv is None:
            p_only += 1
            diffs.append((k, None, pb))
            continue
        if pb is None:
            c_only += 1
            diffs.append((k, cv, None))
            continue
        joined += 1
        c_visits += len(cv)
        # The port evaluates a leaf once, in its SQUARE context: join that
        # against C's square visit (sq == dims when the dump carries sq;
        # otherwise the FIRST visit, which is the square one in C's walk),
        # and count the AB-shape visits the port has no counterpart for.
        x, y, w, h = k
        sqv = [v for v in cv if v['sq'] is None or int(v['sq']) == max(w, h)]
        c_sq_visits += len(sqv)
        c_ab_visits += len(cv) - len(sqv)
        cb = sqv[0] if sqv else cv[0]
        c_st1 += len(cb['st1']); c_st3 += len(cb['st3'])
        p_m1 += len(pb['mds1']); p_m3 += len(pb['mds3'])
        if cb['pm1'] == '0':
            pm1_zero += 1
        elif cb['pm1'] == '1':
            pm1_one += 1
        if len(cb['st1']) == len(pb['mds1']):
            mds1_eq += 1
        if len(cb['st3']) == len(pb['mds3']):
            mds3_eq += 1
        cs = sorted((m_, f, d) for (m_, f, d, _u) in cb['st3'])
        ps = sorted((m_, f, d) for (m_, f, d, _u) in pb['mds3'])
        if cs == ps:
            set3_eq += 1
        else:
            diffs.append((k, cb, pb))
    print(f"blocks: C={len(c)} port={len(p)} joined={joined} c_only={c_only} port_only={p_only}")
    print(f"C visits of joined blocks: {c_visits} (square-context {c_sq_visits}, AB-shape sub-block {c_ab_visits})")
    print(f"MDS1 rows (square visit): C st1={c_st1} port PMDS1={p_m1}   (per-block count equal on {mds1_eq}/{joined})")
    print(f"MDS3 rows (square visit): C st3={c_st3} port CAND={p_m3}    (per-block count equal on {mds3_eq}/{joined})")
    print(f"MDS3 admitted SET identical (mode,fi,delta): {set3_eq}/{joined}; C perform_mds1: 0 on {pm1_zero}, 1 on {pm1_one} of {joined}")
    def ordkey(t):
        k, cb, pb = t
        if cb is None:
            return 1 << 30
        v = cb[0] if isinstance(cb, list) else cb
        return v['ord']
    diffs.sort(key=ordkey)
    shown = 0
    for k, cb, pb in diffs:
        x, y, w, h = k
        if cb is not None and pb is not None and shown < first_n:
            shown += 1
            print(f"FIRST-DIFF #{shown} (C coding order {cb['ord']}) {x},{y} {w}x{h}: MDS1 per-candidate (mode,fi,delta): C (ydist,cost) | port (dist,full)")
            allc = sorted(set(cb['cost1']) | set(pb['cost1']))
            neq = 0
            for cnd in allc:
                cc, pc = cb['cost1'].get(cnd), pb['cost1'].get(cnd)
                flag = '' if cc == pc else '  <-- differs'
                if cc != pc:
                    neq += 1
                print(f"      {cnd}: C {cc} | port {pc}{flag}")
            print(f"      MDS1 candidates: {len(allc)}, cost-equal {len(allc) - neq}, differing {neq}")
        if cb is None:
            print(f"PORT-ONLY {x},{y} {w}x{h} port mds1={len(pb['mds1'])} mds3={len(pb['mds3'])}")
            continue
        if pb is None:
            v = cb[0]
            print(f"C-ONLY    {x},{y} {w}x{h} visits={len(cb)} C st1={len(v['st1'])} st3={len(v['st3'])} n1tot={v['n1tot']} pm1={v['pm1']} sq={v['sq']}")
            continue
        print(f"DIFF {x},{y} {w}x{h}  C st1={len(cb['st1'])} st3={len(cb['st3'])} n1tot={cb['n1tot']} pm1={cb['pm1']} sq={cb['sq']} | "
              f"port mds1={len(pb['mds1'])} mds3={len(pb['mds3'])}")
        print(f"     C st3 : {sorted(cb['st3'])}")
        print(f"     port  : {sorted(pb['mds3'])}")
        if verbose:
            print(f"     C st1 : {sorted(cb['st1'])}")
            print(f"     port1 : {sorted(pb['mds1'])}")
    sys.exit(0 if (set3_eq == joined and c_only == 0 and p_only == 0) else 1)


if __name__ == '__main__':
    main()
