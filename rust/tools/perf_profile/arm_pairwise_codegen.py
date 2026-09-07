#!/usr/bin/env python3
"""Compare the two complete NEON ME bodies in the ARM pairwise benchmark.

Use a kernel_tiers release binary built on macOS ARM. Check every instruction
and branch target through the sole return, plus total function lengths. Cold
panic-location metadata after the return differs because the source files do.
"""
import json
import pathlib
import re
import subprocess
import sys


def functions(assembly):
    result = {}
    current = None
    for line in assembly.splitlines():
        if line.endswith(":"):
            current = line[:-1]
            result[current] = []
        elif current and re.match(r"[0-9a-f]{16}\t", line):
            address, text = line.split("\t", 1)
            result[current].append((int(address, 16), text))
    return result


def hot_body(body):
    assert sum(text == "ret" for _, text in body) == 1
    start, end = body[0][0], body[-1][0] + 4
    result = []
    for _, text in body:
        # Normalize local branch addresses; preserve register names, opcodes,
        # loads, arithmetic constants and the precise branch destination.
        text = re.sub(r"0x[0-9a-f]+", lambda m:
                      f"pc+{int(m[0], 16)-start}" if start <= int(m[0], 16) < end
                      else m[0], text)
        result.append(text)
        if text == "ret":
            return result
    raise AssertionError("return not found")


def main():
    binary = pathlib.Path(sys.argv[1]).resolve()
    parsed = functions(subprocess.check_output(["otool", "-tvV", str(binary)], text=True))
    rows = []
    for suffix in ("block_sad_neon", "block_sum_sse_neon"):
        names = [n for n in parsed if n.endswith(suffix)]
        assert len(names) == 2, names
        hand = next(n for n in names if "arm_pairwise_reference" in n)
        api = next(n for n in names if n != hand)
        a, b = parsed[hand], parsed[api]
        assert len(a) == len(b), (suffix, len(a), len(b))
        assert hot_body(a) == hot_body(b), suffix
        rows.append({"kernel": suffix, "total_instructions_each": len(a),
                     "hot_instructions_each": len(hot_body(a)), "hot_bodies_identical": True})
    print(json.dumps({"binary": str(binary), "comparisons": rows}, indent=2))


if __name__ == "__main__":
    main()
