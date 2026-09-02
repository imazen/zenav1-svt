#!/usr/bin/env python3
"""Compare the C encoder's saved end-of-frame FRAME_CONTEXT against the port's.

The two dumps are produced by:
  C     tools/capture_c_trace/wrap_recon.c's
        __wrap_svt_av1_reset_cdf_symbol_counters  (SVT_FCTX_OUT)
  port  crate::port_frame_cdf::FrameCdfs::dump_to (SVTAV1_FCTX_OUT)

both AFTER the symbol-counter reset, i.e. exactly the bytes C copies into
EbReferenceObject::frame_context and the port stores on ReferenceFrame.

That makes this the oracle for CDF CONTINUATION, and it does NOT need the
inter tile walk to work first: a frame's saved state can be proven right
before any later frame consumes it.

Usage: fctx_diff.py <c.fctx> <rs.fctx> [--frame N] [--max-fields N]

Exit 0 iff every field the port carries matches C's for the requested frame.
Fields C dumps that the port has no storage for are listed as MISSING and are
NOT failures on their own — the port names them in port_frame_cdf's docs — but
they ARE reported, because "absent" and "equal" must never look the same.
"""
import sys


def load(path):
    frames = {}
    with open(path) as f:
        for line in f:
            if not line.startswith("FCTX "):
                continue
            parts = line.split()
            n = int(parts[1])
            name = parts[2]
            count = int(parts[3])
            vals = [int(x) for x in parts[4 : 4 + count]]
            if len(vals) != count:
                raise SystemExit(f"{path}: {name} declares {count} values, has {len(vals)}")
            frames.setdefault(n, {})[name] = vals
    return frames


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    frame = 0
    maxf = 12
    for a in sys.argv[1:]:
        if a.startswith("--frame="):
            frame = int(a.split("=", 1)[1])
        if a.startswith("--max-fields="):
            maxf = int(a.split("=", 1)[1])
    if len(args) != 2:
        raise SystemExit(__doc__)
    c = load(args[0])
    r = load(args[1])
    if frame not in c:
        raise SystemExit(f"C dump has no frame {frame} (has {sorted(c)})")
    if frame not in r:
        raise SystemExit(f"port dump has no frame {frame} (has {sorted(r)})")
    cf, rf = c[frame], r[frame]

    missing = [k for k in cf if k not in rf]
    extra = [k for k in rf if k not in cf]
    bad = []
    for name, cv in cf.items():
        rv = rf.get(name)
        if rv is None:
            continue
        if len(rv) != len(cv):
            bad.append((name, "LENGTH", len(cv), len(rv), None))
            continue
        diffs = [i for i, (a, b) in enumerate(zip(cv, rv)) if a != b]
        if diffs:
            bad.append((name, "VALUES", len(diffs), len(cv), (diffs[0], cv[diffs[0]], rv[diffs[0]])))

    print(f"frame {frame}: {len(cf)} C fields, {len(rf)} port fields")
    if missing:
        print(f"  MISSING from the port ({len(missing)}): {' '.join(sorted(missing))}")
    if extra:
        print(f"  EXTRA in the port ({len(extra)}): {' '.join(sorted(extra))}")
    shared = len(cf) - len(missing)
    print(f"  compared {shared} shared fields: {shared - len(bad)} identical, {len(bad)} differ")
    for name, kind, a, b, first in sorted(bad)[:maxf]:
        if kind == "LENGTH":
            print(f"    {name}: LENGTH C={a} port={b}")
        else:
            i, cv, rv = first
            print(f"    {name}: {a}/{b} values differ, first at [{i}] C={cv} port={rv}")
    if len(bad) > maxf:
        print(f"    ... and {len(bad) - maxf} more")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
