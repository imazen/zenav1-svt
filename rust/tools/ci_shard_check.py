#!/usr/bin/env python3
"""Static guard for the sharded `gates` job in .github/workflows/rust-gates.yml.

The gates job is split across a `shard` matrix, and every gate step selects its
shard with `if: matrix.shard == N`. That mechanism has one catastrophic failure
mode: if the matrix is removed, or a step is guarded for a shard that is not in
the matrix, the `if` is simply FALSE and the step is SKIPPED -- and a job whose
steps all skip reports SUCCESS. CI would go green while running no gates at all.

This check makes that impossible to land silently. It asserts:
  1. the gates job declares a shard matrix;
  2. every step after the setup block carries exactly one shard guard;
  3. every guard names a shard that is actually in the matrix;
  4. every shard in the matrix owns at least one gate;
  5. the guarded steps partition the gate set -- union == all, no overlap.

Run: python3 rust/tools/ci_shard_check.py [--check]
Exits non-zero with a specific diagnosis on any violation.
"""
from __future__ import annotations
import re, sys, pathlib

try:
    import yaml
except ImportError:
    sys.exit("ci_shard_check: PyYAML required (pip install pyyaml)")

WF = pathlib.Path(__file__).resolve().parents[2] / ".github" / "workflows" / "rust-gates.yml"
# Steps before this index are shared setup (checkout, toolchain, caches, oracles)
# and MUST run in every shard, so they carry no shard guard.
GATE_START = 11

def main() -> int:
    if not WF.is_file():
        print(f"ci_shard_check: missing {WF}", file=sys.stderr)
        return 2
    doc = yaml.safe_load(WF.read_text())
    job = doc.get("jobs", {}).get("gates")
    if job is None:
        print("ci_shard_check: no `gates` job", file=sys.stderr)
        return 1

    matrix = (job.get("strategy") or {}).get("matrix") or {}
    shards = matrix.get("shard")
    if not shards:
        print("ci_shard_check: the `gates` job declares NO shard matrix, but its "
              "steps are guarded on `matrix.shard`. Every guard would evaluate "
              "false, every gate would SKIP, and the job would still report "
              "success. Restore `strategy.matrix.shard`.", file=sys.stderr)
        return 1
    shards = [int(s) for s in shards]

    steps = job.get("steps") or []
    setup, gates = steps[:GATE_START], steps[GATE_START:]

    bad = []
    for s in setup:
        cond = s.get("if")
        if isinstance(cond, str) and "matrix.shard" in cond:
            bad.append(s.get("name") or s.get("uses"))
    if bad:
        print(f"ci_shard_check: setup steps must run in EVERY shard but are "
              f"shard-guarded: {bad}", file=sys.stderr)
        return 1

    owned: dict[int, list[str]] = {n: [] for n in shards}
    unguarded = []
    for s in gates:
        name = s.get("name") or s.get("uses") or "<unnamed>"
        cond = s.get("if")
        m = re.findall(r"matrix\.shard\s*==\s*'?(\d+)'?", cond) if isinstance(cond, str) else []
        if len(m) != 1:
            unguarded.append((name, cond))
            continue
        n = int(m[0])
        if n not in owned:
            print(f"ci_shard_check: step {name!r} is guarded for shard {n}, which "
                  f"is NOT in the matrix {shards}. It would never run.", file=sys.stderr)
            return 1
        owned[n].append(name)

    if unguarded:
        print("ci_shard_check: these gate steps have no single shard guard, so "
              "they would run in EVERY shard (duplicated work) or, with a "
              "malformed condition, in none:", file=sys.stderr)
        for name, cond in unguarded:
            print(f"    {name!r}  if={cond!r}", file=sys.stderr)
        return 1

    empty = [n for n, v in owned.items() if not v]
    if empty:
        print(f"ci_shard_check: shards {empty} own no gates -- they burn a runner "
              f"doing only setup. Rebalance or shrink the matrix.", file=sys.stderr)
        return 1

    total = sum(len(v) for v in owned.values())
    if total != len(gates):
        print(f"ci_shard_check: partition is not exact: {total} guarded vs "
              f"{len(gates)} gate steps.", file=sys.stderr)
        return 1

    print(f"ci_shard_check: OK — {len(gates)} gate steps partitioned across "
          f"shards {shards}: " + ", ".join(f"{n}:{len(owned[n])}" for n in shards))
    return 0

if __name__ == "__main__":
    sys.exit(main())
