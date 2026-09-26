#!/usr/bin/env python3
"""Refuse an environment variable a script sets that no program reads.

A gate that sets `SVTAV1_STILL_TUNE=1` after the feature is deleted still
runs, still passes, and reports the default encoder under the feature's name.
Nothing downstream can tell. This check makes that a CI failure.

It collects every `SVTAV1_*` / `SVT_*` name that a script under `tools/`, the
justfile or a workflow ASSIGNS (`NAME=`, or a Python dict key `'NAME':`), and
requires each to appear as a string literal in a reader:
  SVTAV1_*  the Rust sources (crates, facade, examples);
  SVT_*     those, the C capture driver, or the shell tools that read them
            (`${SVT_X:-...}`, `$SVT_X`, `os.environ.get('SVT_X')`).
Names in the allowlist below are set for a reason other than being read.

Usage: tools/env_names_check.py   (exit 1 lists each unread name and where)
"""

import re
import sys
from pathlib import Path

RS = Path(__file__).resolve().parent.parent
REPO = RS.parent

# Set on purpose although nothing reads them (each with its reason).
ALLOW = {
    # The retired inter switch, refused by name in dbgenv if set to a value
    # that would have mattered; old gate env vectors still carry it.
    "SVTAV1_INTER_EXPERIMENTAL",
}

SETTERS = (
    [p for p in (RS / "tools").rglob("*") if p.suffix in (".sh", ".py", "") and p.is_file()]
    + [RS / "justfile"]
    + list((REPO / ".github" / "workflows").glob("*.yml"))
)
ASSIGN = re.compile(r"""(?<![A-Za-z0-9_$])(SVT(?:AV1)?_[A-Z0-9_]+)(?:=|['"]\s*:|['"]\]\s*=)""")


def text(p):
    try:
        return p.read_text(errors="replace")
    except (IsADirectoryError, PermissionError):
        return ""


def code(s):
    """Drop comments: a name mentioned in prose is not an assignment."""
    out = []
    for line in s.splitlines():
        if line.lstrip().startswith("#"):
            out.append("")
            continue
        out.append(line.split(" # ")[0])
    return "\n".join(out)


def main():
    rust = "".join(
        text(p)
        for base in (RS / "crates", RS / "svtav1")
        for p in base.rglob("*.rs")
        if "target" not in p.parts
    )
    c_driver = "".join(text(p) for p in (RS / "tools" / "capture_c_trace").glob("*.c"))
    me = Path(__file__).resolve()
    setters = {p: code(text(p)) for p in SETTERS if p.exists() and p.resolve() != me}
    shell_reads = "".join(setters.values())

    def read_somewhere(name):
        if f'"{name}"' in rust:
            return True
        if name.startswith("SVTAV1_"):
            return False
        if f'"{name}"' in c_driver:
            return True
        # A tool that consumes the name itself (a gate's own knob).
        return re.search(
            rf"\$\{{?{name}\b|environ(?:\.get)?\(?\[?['\"]{name}['\"]|getenv\(['\"]{name}['\"]",
            shell_reads,
        ) is not None

    unread = {}
    for p, s in setters.items():
        for m in ASSIGN.finditer(s):
            name = m.group(1)
            if name in ALLOW or read_somewhere(name):
                continue
            line = s.count("\n", 0, m.start()) + 1
            unread.setdefault(name, []).append(f"{p.relative_to(REPO)}:{line}")
    for name, where in sorted(unread.items()):
        print(f"{name}: set but never read — {', '.join(where[:4])}"
              + (f" (+{len(where) - 4})" if len(where) > 4 else ""))
    if unread:
        print(f"env_names_check: {len(unread)} unread name(s). Delete the assignment, "
              "or wire the reader.", file=sys.stderr)
        return 1
    print(f"env_names_check: OK — every SVT*/SVTAV1_* name a script sets has a reader")
    return 0


if __name__ == "__main__":
    sys.exit(main())
