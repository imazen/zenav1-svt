#!/usr/bin/env python3
"""Every tests/*.rs file belongs to exactly one test target.

Three crates set `autotests = false` and aggregate most integration tests as
modules of one target (dsp_parity, svtav1_suite) so they link once. The
encoder's aggregator, `tests/encoder_parity.rs`, is compiled INTO the lib's
unit-test binary instead (`#[cfg(test)] #[path = "../tests/..."] mod` in
src/lib.rs, plan T2); a file the lib includes that way counts as owned. A file that is ALSO declared as its own [[test]] compiles into two
binaries and runs twice; a file in NEITHER never runs at all. Both happened
(35a246eb3 merged; d2abc282b re-declared 162 merged files as standalone). This
check fails on either, and prints the fix.

    tools/test_targets_check.py            # check
    tools/test_targets_check.py --fix      # drop standalone entries for aggregated files
"""
import pathlib
import re
import sys

RUST = pathlib.Path(__file__).resolve().parents[1]
CRATES = [RUST / "crates/svtav1-dsp", RUST / "crates/svtav1-encoder", RUST / "svtav1", RUST / "crates/svtav1-types"]
TEST_BLOCK = re.compile(
    r'\[\[test\]\]\n(?:#[^\n]*\n)*name = "([^"]+)"\npath = "([^"]+)"\n(?:[a-z_-]+ = [^\n]*\n)*', re.M
)


def modules_of(target: pathlib.Path) -> set:
    """Files a target pulls in as `mod x;` (or `#[path = "..."] mod x;`)."""
    out = set()
    text = target.read_text()
    for m in re.finditer(r'^\s*(?:#\[path\s*=\s*"([^"]+)"\]\s*)?mod\s+(\w+);', text, re.M):
        out.add((m.group(1) or m.group(2) + ".rs").split("/")[-1])
    return out


def main() -> int:
    fix = "--fix" in sys.argv
    bad = 0
    for crate in CRATES:
        toml_path = crate / "Cargo.toml"
        toml = toml_path.read_text()
        tests_dir = crate / "tests"
        if not tests_dir.is_dir():
            continue
        if "autotests = false" not in toml:
            continue  # cargo discovers every file itself: one target each
        declared = {m.group(2).split("/")[-1]: m for m in TEST_BLOCK.finditer(toml)}
        lib = crate / "src/lib.rs"
        in_lib = set()
        if lib.exists():
            for m in re.finditer(r'#\[path\s*=\s*"\.\./tests/([^"/]+)"\]\s*mod\s+\w+;', lib.read_text()):
                in_lib.add(m.group(1))
        aggregated = {f: ["src/lib.rs"] for f in in_lib}
        for fname in list(declared) + sorted(in_lib):
            for mod in modules_of(tests_dir / fname):
                aggregated.setdefault(mod, []).append(fname)
        files = sorted(p.name for p in tests_dir.glob("*.rs"))
        drop = []
        for f in files:
            owners = ([f] if f in declared else []) + aggregated.get(f, [])
            if not owners:
                print(f"{crate.name}: tests/{f} is in NO target — it never runs")
                bad += 1
            elif len(owners) > 1:
                bad += 1
                if f in declared and aggregated.get(f):
                    drop.append(f)
                print(f"{crate.name}: tests/{f} is in {len(owners)} targets ({', '.join(owners)})")
        if fix and drop:
            for f in drop:
                toml = toml.replace(declared[f].group(0), "", 1)
            toml = re.sub(r"\n{3,}", "\n\n", toml)
            toml_path.write_text(toml)
            print(f"{crate.name}: removed {len(drop)} standalone [[test]] entries for aggregated files")
    if bad and not fix:
        print(f"\n{bad} problem(s). Run tools/test_targets_check.py --fix to drop duplicate standalone entries.")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
