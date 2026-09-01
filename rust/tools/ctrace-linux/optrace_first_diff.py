#!/usr/bin/env python3
"""First diverging arithmetic-coder OP between a C `--wrap` trace and the
port's symtrace, for ONE frame.

    optrace_first_diff.py <c.trace> <rs.trace> [--c-seg N] [--port-seg N]
                          [--context N] [--segments]

Why this exists beside `identity_diff.py`: that tool walks the whole stream
(headers, OBU layer, per-frame verdicts) and reports an op index, but its
alignment assumes ONE frame per trace and the still driver's op prologue. On a
video-mode cell the C trace carries EVERY frame (its `W RESET` lines are the
frame boundaries) and opens with a `W INIT` the port has no counterpart for, so
the index it prints is shifted and the op it names is not the op that diverged.
The byte VERDICT is unaffected; only the localization is. This script does the
two normalizations that make the two streams comparable and nothing else:

  * SPLIT each trace on `W RESET` and compare ONE segment from each. Every
    `AomWriter` a run creates emits a `W RESET`, and a run creates more than
    the frames it packs: C's segments are its FRAMES (so frame 0 is segment 0),
    while the port's include the per-SB CDF-chain simulation's writers and the
    tile re-walks — MEASURED on `gradient 72x88 q40 p4` video, where C has 2
    segments (two frames) and the port has FIVE. Defaults are C segment 0 and
    the port's LAST segment (the real pack); `--segments` prints the inventory
    so a wrong pick is visible rather than silent, and both are overridable.
    Concatenating the port's segments is what a naive reading does and it
    reports a divergence at op 3 of a byte-IDENTICAL cell.
  * rewrite `W BOOL val=v f=p rng=r` and `W BOOLEQ val=v rng=r` (C
    `aom_write_bit`, f = 16384) as the equivalent 2-symbol CDF writes, so C's
    bool-coder paths and the port's `write_symbol` spell the same op.

Then it prints the FIRST index at which the two disagree, with context — which
is the whole localization when the byte divergence is inside the tile payload
and every decodable field (tree, modes, levels, recon) already matches.

Reading the answer: an unexpected EXTRA op on one side is a syntax element the
other encoder did not write. Grep its `icdf` value in
`crates/svtav1-encoder/src/entropy/default_cdfs.rs` to name the CDF table, and
the table names the element. `docs/INTER-ENCODE-PLAN.md` §1j is the worked
example (a `TX_SIZE_CDF` write under TX_MODE_LARGEST).

Exit status: 0 if the compared prefixes agree over the shorter trace, 1 if they
diverge.
"""
import re
import sys

BOOL = re.compile(r"W BOOL val=(\d+) f=(\d+) rng=(\d+)")
# C `aom_write_bit` — an equiprobable literal, which the port writes as a
# 2-symbol CDF at 16384. Same op, two spellings (`identity_diff.py` header).
BOOLEQ = re.compile(r"W BOOLEQ val=(\d+)(?: rng=(\d+))?")
CDF = re.compile(r"W CDF nsyms=(\d+) s=(\d+) icdf=\[([^\]]*)\] rng=(\d+)")
# Terminator. Both sides report the byte count; the tail differs (C prints its
# EC pointer, the port the first bytes), so only `nbytes` is comparable.
DONE = re.compile(r"W DONE nbytes=(\d+)")


def segments(path):
    """-> [[op string]], one list per `W RESET` (one per AomWriter)."""
    segs = []
    with open(path) as f:
        for raw in f:
            line = raw.strip()
            if line.startswith("W INIT"):
                continue  # C-side allocation marker; the port has none
            if line.startswith("W RESET"):
                segs.append([])
                continue
            if not segs:
                continue  # ops before the first RESET belong to no writer
            ops = segs[-1]
            m = BOOL.match(line)
            if m:
                ops.append(f"CDF n=2 s={m.group(1)} icdf=[{m.group(2)}] rng={m.group(3)}")
                continue
            m = BOOLEQ.match(line)
            if m:
                ops.append(f"CDF n=2 s={m.group(1)} icdf=[16384] rng={m.group(2) or '?'}")
                continue
            m = CDF.match(line)
            if m:
                n = int(m.group(1))
                # The port pads icdf to three entries; C prints what it has.
                icdf = ",".join(m.group(3).split(",")[: max(1, n - 1)])
                ops.append(f"CDF n={n} s={m.group(2)} icdf=[{icdf}] rng={m.group(4)}")
                continue
            m = DONE.match(line)
            if m:
                ops.append(f"DONE nbytes={m.group(1)}")
                continue
            # Everything else on these streams is prose, not an op: the port's
            # stderr also carries `identity_run: REFUSED ...` (the expected
            # inter refusal) and SVTAV1_TRACEMARK `# BLK` markers.
            if not line.startswith("W "):
                continue
            ops.append(line)
    return segs


def opt(argv, name, default):
    for a in argv:
        if a.startswith(f"--{name}="):
            return int(a.split("=", 1)[1])
    return default


def main():
    argv = sys.argv[1:]
    args = [a for a in argv if not a.startswith("--")]
    if len(args) != 2:
        print(__doc__)
        return 2
    ctx = opt(argv, "context", 6)
    csegs = segments(args[0])
    rsegs = segments(args[1])
    if "--segments" in argv or not csegs or not rsegs:
        print(f"C segments   ({len(csegs)}): {[len(x) for x in csegs]}")
        print(f"port segments({len(rsegs)}): {[len(x) for x in rsegs]}")
        if not csegs or not rsegs:
            return 2
    ci = opt(argv, "c-seg", 0)
    ri = opt(argv, "port-seg", len(rsegs) - 1)
    c = csegs[ci]
    r = rsegs[ri]
    print(f"ops: C seg {ci} = {len(c)}, port seg {ri} = {len(r)} (normalized)")
    for i, (a, b) in enumerate(zip(c, r)):
        if a != b:
            print(f"FIRST DIVERGING OP: {i}")
            for j in range(max(0, i - ctx), min(len(c), i + 3)):
                mark = ">>" if j == i else "  "
                print(f"{mark} {j:6d} C: {c[j]}")
                print(f"          port: {r[j] if j < len(r) else '(none)'}")
            return 1
    if len(c) != len(r):
        print(f"prefixes agree; op COUNT differs (C {len(c)}, port {len(r)})")
        return 1
    print("op streams identical over the compared segments")
    return 0


if __name__ == "__main__":
    sys.exit(main())
