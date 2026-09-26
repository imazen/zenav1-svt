#!/usr/bin/env python3
"""Ledger of encoder and dsp items the pipeline never calls (plan T2).

The parity differentials compile into each crate's unit-test binary, so the
C-translation modules only they exercised are `pub(crate)`. Each such module
carries `#[cfg_attr(not(feature = "__dead_code_audit"), allow(dead_code))]` on
its `mod` line in the crate's src/lib.rs. This tool builds each lib with that
feature on, twice, and lists every dead item:

  tests=yes   only the parity tests call it: a C translation verified against
              C that the pipeline does not reach (wire it, or it duplicates
              live code and one copy goes);
  tests=no    nothing calls it at all, not even a test: delete it, or give it
              a test (unless it is a faithful C translation of a feature the
              port has not wired — rust/CLAUDE.md "Keep faithful translations").

    tools/dead_code_ledger.py            # check docs/DEAD-CODE.tsv
    tools/dead_code_ledger.py --write    # regenerate it

The check fails when the ledger differs from the build, and when a module's
allow names a lint nothing in it needs (or misses one that fires). Line
numbers are left out so unrelated edits do not churn the file.
"""
import json
import os
import re
import subprocess
import sys
from collections import Counter
from pathlib import Path

RUST = Path(__file__).resolve().parents[1]
LEDGER = RUST / "docs/DEAD-CODE.tsv"
CRATES = {  # ledger name -> (cargo package, src dir)
    "encoder": ("zenav1-svt-encoder", "crates/svtav1-encoder/src/"),
    "dsp": ("zenav1-svt-dsp", "crates/svtav1-dsp/src/"),
}
ALLOW = re.compile(
    r'#\[cfg_attr\(not\(feature = "__dead_code_audit"\), allow\(([^)]*)\)\)\]\n(?:#\[[^\n]*\]\n)*'
    r'pub(?:\(crate\))? mod (\w+);'
)
LINTS = {"dead_code", "unused_imports"}


def dead_items(package: str, src: str, tests: bool) -> set:
    """(file, lint words, item) for every dead item in one build of a lib.

    `tests=False` is the plain lib; `tests=True` is the lib compiled as its
    own unit-test binary, cfg(test) (`--lib --profile test`, cargo's way to
    check unit tests alone; `--tests` would also build the integration
    targets, which link the PLAIN lib, and its messages would mix in
    indistinguishably). rustc groups items in one message
    ("functions `a` and `b` are never used") and groups them differently in
    the two builds, so the ledger keys on each named item, not the message.
    """
    cmd = ["cargo", "check", "-p", package, "--lib", "--locked",
           "--features", "__dead_code_audit", "--message-format=json"]
    if tests:
        cmd += ["--profile", "test"]
    r = subprocess.run(cmd, cwd=RUST, capture_output=True, text=True,
                       env={**os.environ, "CARGO_TERM_COLOR": "never"})
    if r.returncode != 0:
        sys.exit(f"dead_code_ledger: cargo check failed:\n{r.stderr[-4000:]}")
    out = set()
    for line in r.stdout.splitlines():
        if not line.startswith("{"):
            continue
        msg = json.loads(line)
        if msg.get("reason") != "compiler-message":
            continue
        target = msg.get("target", {})
        # Each invocation builds ONE variant of the lib; messages do not say
        # which (`target.test` is the manifest flag, not the build mode). A
        # dependency's messages are filtered out by the src prefix below.
        if "lib" not in target.get("kind", []):
            continue
        m = msg["message"]
        code = (m.get("code") or {}).get("code")
        if code not in LINTS or not m.get("spans"):
            continue
        span = next((s for s in m["spans"] if s.get("is_primary")), m["spans"][0])
        path = span["file_name"]
        if not path.startswith(src):
            continue
        what = m["message"].split("`", 1)[0].strip().rstrip(":")
        for name in re.findall(r"`([^`]+)`", m["message"]):
            out.add((path[len(src):], what, name))
    return out


def includers(src: str) -> dict:
    """`include!("x.rs")` files, mapped to the module file that includes them."""
    out = {}
    for f in (RUST / src).rglob("*.rs"):
        for inc in re.findall(r'include!\("([^"]+\.rs)"\)', f.read_text()):
            out[str((f.parent / inc).relative_to(RUST / src))] = str(f.relative_to(RUST / src))
    return out


def main() -> int:
    write = "--write" in sys.argv
    rows, stale, unannotated = [], [], []
    for crate, (package, src) in CRATES.items():
        inc = includers(src)

        def module_of(rel: str) -> str:
            return re.split(r"[/.]", inc.get(rel, rel), maxsplit=1)[0]

        non_test = dead_items(package, src, tests=False)
        with_tests = {(f, n) for f, _, n in dead_items(package, src, tests=True)}
        rows += [(crate, module_of(f), f, what, name, "no" if (f, name) in with_tests else "yes")
                 for f, what, name in non_test]
        allowed = {mod: {l.strip() for l in lints.split(",")}
                   for lints, mod in ALLOW.findall((RUST / src / "lib.rs").read_text())}
        needed = {}
        for f, what, _ in non_test:
            needed.setdefault(module_of(f), set()).add("unused_imports" if "import" in what else "dead_code")
        stale += [f"{crate}::{m}: {l}" for m in allowed for l in sorted(allowed[m] - needed.get(m, set()))]
        unannotated += [f"{crate}::{m}: {l}" for m in needed for l in sorted(needed[m] - allowed.get(m, set()))]
    rows.sort()
    body = "".join("\t".join(r) + "\n" for r in rows)
    per_mod = Counter((r[0], r[1]) for r in rows)
    untested = sum(r[5] == "no" for r in rows)
    header = (
        "# encoder and dsp items the pipeline never calls. Generated by\n"
        "# tools/dead_code_ledger.py --write; checked in CI. Columns: crate, module,\n"
        "# file, kind (rustc's words), item, tests (yes = only the parity tests call\n"
        f"# it; no = nothing does). {len(rows)} items in {len(per_mod)} modules, {untested} with no caller at all.\n"
    )
    text = header + body

    if write:
        LEDGER.write_text(text)
        print(f"dead_code_ledger: wrote {len(rows)} items ({untested} with no caller) to {LEDGER.relative_to(RUST)}")
    elif not LEDGER.exists() or LEDGER.read_text() != text:
        old = set(LEDGER.read_text().splitlines()) if LEDGER.exists() else set()
        new = set(text.splitlines())
        for l in sorted(new - old)[:20]:
            print(f"  + {l}")
        for l in sorted(old - new)[:20]:
            print(f"  - {l}")
        print("dead_code_ledger: docs/DEAD-CODE.tsv is stale; run tools/dead_code_ledger.py --write")
        return 1
    bad = 0
    for m in stale:
        print(f"dead_code_ledger: `{m}` is allowed but nothing in the module needs it; narrow the allow in lib.rs")
        bad = 1
    for m in unannotated:
        print(f"dead_code_ledger: `{m}` fires but is not allowed (the build would warn)")
        bad = 1
    return bad


if __name__ == "__main__":
    sys.exit(main())
