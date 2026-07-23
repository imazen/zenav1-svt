#!/usr/bin/env python3
"""Chroma DC-prediction drift localizer: join C's SVT_UVLOOP_OUT (all-blocks
mode) against the port's NSQDBG CFLARB dump and report the FIRST block, in C
walk order, whose chroma DC-prediction ORIGIN SAMPLES diverge.

Usage:
  SVT_UVLOOP_OUT=c.uvloop_all capture_c_trace W H QP P in.yuv c.obu
  SVTAV1_NSQDBG=1 identity_run <content> W H QP P rs 2>&1 | grep CFLARB > rs.cflarb_all
  tools/uvdc_join.py c.uvloop_all rs.cflarb_all

Why: an MD chroma-neighbour drift is coding-INVISIBLE until it flips a
decision, and the decoder's block records ignore angle deltas — so the first
*coded* flip can sit far downstream of the first drifted *input*. The DC
prediction origin sample (C: `pu/pv` on the uv=0 ind-uv evaluation; port:
`udc/vdc` on the CFLARB line) is a per-block readout of the MD chroma
neighbour state; the first mismatch bounds the producer block to the
immediately preceding committed blocks. Used to pin the bd8 ind-uv sort root
(79cc43d3c): SB rows 0 + SB(1,0) clean, drift ignites in SB(1,1) at the block
right AFTER the first tie-order-divergent uv winner.

Coverage note: the port prints CFLARB only where the CfL gate runs (both luma
dims <= 32), so >=64 shapes appear as C-only rows — expected, not a drift.
"""
import re
import sys

c_path, rs_path = sys.argv[1], sys.argv[2]

c_first = {}
c_order = []
c_re = re.compile(
    r"UVLOOP org=\((\d+),(\d+)\) (\d+)x(\d+) mode=(-?\d+) uv=(-?\d+) uvd=(-?\d+) "
    r"full=(\d+) cbb=(\d+) crb=(\d+) cbd=(\d+) crd=(\d+) pu=(\d+) pv=(\d+)")
for line in open(c_path):
    m = c_re.search(line)
    if not m:
        continue
    x, y, w, h, mode, uv, uvd, full, cbb, crb, cbd, crd, pu, pv = map(int, m.groups())
    key = (x, y, w, h)
    if uv == 0 and uvd == 0 and key not in c_first:
        c_first[key] = (pu, pv)
        c_order.append(key)

rs_first = {}
rs_re = re.compile(
    r"CFLARB mi=\((\d+),(\d+)\) (\d+)x(\d+) m=(-?\d+) .* udc=(\d+) vdc=(\d+)")
for line in open(rs_path):
    m = rs_re.search(line)
    if not m:
        continue
    r, c, w, h, mode, udc, vdc = map(int, m.groups())
    key = (c * 4, r * 4, w, h)
    if key not in rs_first:
        rs_first[key] = (udc, vdc)

n_match = n_diff = n_conly = 0
first_diffs = []
for key in c_order:
    if key not in rs_first:
        n_conly += 1
        continue
    if c_first[key] == rs_first[key]:
        n_match += 1
    else:
        n_diff += 1
        if len(first_diffs) < 15:
            x, y, w, h = key
            first_diffs.append(
                f"DCDIFF org=({x},{y}) {w}x{h} mi=({y//4},{x//4}) "
                f"C=({c_first[key][0]},{c_first[key][1]}) port=({rs_first[key][0]},{rs_first[key][1]})")
rs_only = len(rs_first) - n_match - n_diff
print(f"blocks: C={len(c_order)} port={len(rs_first)} joined-match={n_match} "
      f"joined-DIFF={n_diff} C-only={n_conly} port-only={rs_only}")
for d in first_diffs:
    print(d)
