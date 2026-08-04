#!/usr/bin/env bash
# Which screen-content tool causes a divergence -- palette, IntraBC, or neither?
# Re-encodes each cell under SVTAV1_SC_TOOLS={default,nopalette,noibc,none} and
# prints the first differing byte offset against the real C encoder.
#
# READ THE SIZES, not just the offsets: on graph.png at q32, turning palette OFF
# moves the port FURTHER from C (3792 -> 4186 against C's 3781), which is how we
# know the port's palette is winning real RD rather than over-picking.
# Which screen-content tool causes the graph.png divergence? Prints the first
# differing byte offset per configuration; "none" = byte-identical.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
cd "$HERE/.."
. "$HERE/lib_corpus.sh"
IMG=${IMG:-$(corpus_dir codec-corpus/gb82-sc)/graph.png}
O=${TMPDIR:-/tmp}/scb.$$; rm -rf $O; mkdir -p $O
cell() { # qp preset mode
  local m=$3
  SVTAV1_SC_TOOLS="$m" "$HERE/identity_run" "crop:$IMG" 512 512 "$1" "$2" $O/rs >/dev/null 2>&1 || { echo "  q$1 p$2 $m: rs-err"; return; }
  SVT_TRACE_OUT=/dev/null "$HERE/capture_c_trace/capture_c_trace" 512 512 "$1" "$2" $O/rs.yuv $O/c.obu 8 >/dev/null 2>&1 || { echo "  q$1 p$2 $m: c-err"; return; }
  python3 - "$O" "$1" "$2" "$m" <<'PY'
import sys,os
d,q,p,m=sys.argv[1:5]
a=open(os.path.join(d,'c.obu'),'rb').read(); b=open(os.path.join(d,'rs.obu'),'rb').read()
i=next((k for k in range(min(len(a),len(b))) if a[k]!=b[k]), None)
v="IDENTICAL" if (i is None and len(a)==len(b)) else f"diff@{i} (C={len(a)}B rs={len(b)}B)"
print(f"  q{q} p{p} {m:<10}: {v}")
PY
}
for p in 0 2 4; do
  for q in 63 32; do
    for m in default nopalette noibc none; do cell $q $p $m; done
    echo
  done
done
