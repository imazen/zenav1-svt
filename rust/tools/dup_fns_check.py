#!/usr/bin/env python3
"""No two functions in the encoder and dsp crates have the same body.

A second copy of a helper is how a fix lands in one place and not the other.
This hashes every function body over 120 characters (comments and
whitespace stripped, the function's own name ignored, the rest of the
signature kept) in the product sources of svtav1-encoder and svtav1-dsp, and
fails on any group of two or more that is not in ALLOWED below. Fix a
finding by keeping one definition and re-exporting or calling it from the
other site.

ALLOWED holds the pairs that mirror C duplicating a body under two names:
each side cites its own C function, and the duplicate is C's, not ours.
"""
import hashlib
import re
import sys
from collections import defaultdict
from pathlib import Path

RUST = Path(__file__).resolve().parents[1]
ROOTS = [RUST / "crates/svtav1-encoder/src", RUST / "crates/svtav1-dsp/src"]
MIN_BODY = 120
ALLOWED = [
    # C svt_aom_mv_err_cost_light (av1me.c:126) and svt_av1_mv_bit_cost_light
    # (rd_cost.c:59) have the same body.
    {"mv_err_cost_light", "mv_bit_cost_light"},
    # C compute_intra_pd0_th and compute_subres_th (enc_mode_config.c) have the
    # same body.
    {"compute_intra_pd0_th", "compute_subres_th"},
]
FN = re.compile(r"\bfn\s+(\w+)\s*(<[^{;]*?>)?\s*\(")


def bodies():
    for root in ROOTS:
        for f in sorted(root.rglob("*.rs")):
            if f.name == "tests.rs" or "tests" in f.relative_to(root).parts:
                continue
            s = f.read_text()
            for m in FN.finditer(s):
                j = s.find("{", m.end())
                semi = s.find(";", m.end())
                if j < 0 or 0 <= semi < j:
                    continue
                depth = 0
                for k in range(j, len(s)):
                    if s[k] == "{":
                        depth += 1
                    elif s[k] == "}":
                        depth -= 1
                        if depth == 0:
                            break
                body = re.sub(r"\s+", " ", re.sub(r"//[^\n]*", "", s[j:k + 1]))
                if len(body) < MIN_BODY:
                    continue
                sig = re.sub(r"fn\s+\w+", "fn _", re.sub(r"\s+", " ", s[m.start():j]))
                yield f.relative_to(RUST), m.group(1), hashlib.sha1((sig + body).encode()).hexdigest()


def main() -> int:
    groups = defaultdict(list)
    for path, name, h in bodies():
        groups[h].append((path, name))
    bad = 0
    for g in groups.values():
        if len(g) < 2:
            continue
        names = {n for _, n in g}
        if any(names <= allowed for allowed in ALLOWED):
            continue
        bad += 1
        print("identical bodies: " + " | ".join(f"{p}:{n}" for p, n in g))
    if bad:
        print(f"dup_fns_check: {bad} group(s) of duplicated functions; keep one definition")
        return 1
    print("dup_fns_check: OK")
    return 0


if __name__ == "__main__":
    sys.exit(main())
