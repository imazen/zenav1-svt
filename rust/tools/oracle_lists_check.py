#!/usr/bin/env python3
"""Every name in oracles/excludes/*.txt and oracles/divergent/*.txt is a test.

Both lists hold nextest test names (`module::test`, with an
`encoder_parity::` prefix for the encoder's parity modules, which compile
into the encoder crate's unit-test binary). A name that matches no test is
silent: an exclude stops excluding and a divergent pin can never clear. That
happened on 2026-09-26, when the parity tests moved into the crate and every
encoder name gained the prefix; CI's ratchet caught the divergent list, and
nothing would have caught the excludes.

The check is static: `a::b::test` must name `fn test` inside the module file
`b.rs` under some crate's tests/ directory, and an `encoder_parity::` name
must be a module that tests/encoder_parity.rs declares.
"""
import re
import sys
from pathlib import Path

RUST = Path(__file__).resolve().parents[1]
TEST_DIRS = [RUST / "crates/svtav1-encoder/tests", RUST / "crates/svtav1-dsp/tests",
             RUST / "svtav1/tests", RUST / "crates/svtav1-types/tests"]
AGG = RUST / "crates/svtav1-encoder/tests/encoder_parity.rs"


def main() -> int:
    in_lib = set(re.findall(r"^mod (\w+);", AGG.read_text(), re.M))
    files = {}
    for d in TEST_DIRS:
        for f in d.glob("*.rs"):
            files.setdefault(f.stem, []).append(f)
    bad = 0
    for lst in sorted((RUST / "oracles").glob("*/*.txt")):
        for n, line in enumerate(lst.read_text().splitlines(), 1):
            name = line.strip()
            if not name or name.startswith("#"):
                continue
            parts = name.split("::")
            prefixed = parts[0] == "encoder_parity"
            if prefixed:
                parts = parts[1:]
            if len(parts) < 2:
                print(f"{lst.relative_to(RUST)}:{n}: `{name}` is not module::test")
                bad += 1
                continue
            mod, test = parts[0], parts[-1]
            where = f"{lst.relative_to(RUST)}:{n}: `{name}`"
            if (mod in in_lib) != prefixed:
                want = "needs" if mod in in_lib else "must not have"
                print(f"{where} {want} the `encoder_parity::` prefix")
                bad += 1
                continue
            if not any(re.search(rf"\bfn {re.escape(test)}\s*\(", f.read_text())
                       for f in files.get(mod, [])):
                print(f"{where} names no test function")
                bad += 1
    if bad:
        print(f"oracle_lists_check: {bad} stale name(s)")
        return 1
    print("oracle_lists_check: OK")
    return 0


if __name__ == "__main__":
    sys.exit(main())
