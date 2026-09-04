#!/usr/bin/env python3
"""Total per-function CALL COUNTS from a callgrind output file.

`callgrind_annotate` reports Ir (instruction) cost per function; it does not
summarize invocation counts. This script parses the raw callgrind ASCII
format directly and sums every `calls=N <line>` edge that targets a given
callee, keyed by the callee's resolved symbol name — i.e. "how many times was
this function entered, from anywhere, during the whole trace".

callgrind's format interns each function under a numeric id the first time it
is named (`fn=(123) some_name` or `cfn=(123) some_name`) and refers to it by
bare id (`fn=(123)`) afterward, sometimes with a compilation-unit-qualified
form (`cfl=(N)`) preceding it. This walks the file once, keeps an id->name
table, and accumulates `calls=` totals per resolved callee name.

Usage: callcount.py <callgrind-out-file> [--demangle]

Prints, sorted by count descending: "<count>\t<name>"

--demangle pipes C++/Rust-style mangled names (anything starting with `_Z` or
`_R`) through `rustfilt` if available, else leaves them as-is (rustfilt
demangles Rust `_R...`; it passes C symbols through unchanged, which is what
we want here since C symbols in this codebase are not mangled).
"""
import re
import subprocess
import sys

FN_DEF = re.compile(r'^fn=\((\d+)\)(?:\s+(.*))?$')
FN_REF = re.compile(r'^fn=\((\d+)\)$')
CFN_DEF = re.compile(r'^cfn=\((\d+)\)(?:\s+(.*))?$')
CALLS = re.compile(r'^calls=(\d+)\s')


def parse(path):
    names = {}          # id -> raw name (as first seen)
    counts = {}          # id -> total calls into this fn
    cur_cfn_id = None

    with open(path, errors='replace') as f:
        for line in f:
            line = line.rstrip('\n')
            m = FN_DEF.match(line)
            if m:
                fid, nm = m.group(1), m.group(2)
                if nm:
                    names.setdefault(fid, nm)
                cur_cfn_id = None
                continue
            m = CFN_DEF.match(line)
            if m:
                fid, nm = m.group(1), m.group(2)
                if nm:
                    names.setdefault(fid, nm)
                cur_cfn_id = fid
                continue
            m = CALLS.match(line)
            if m and cur_cfn_id is not None:
                counts[cur_cfn_id] = counts.get(cur_cfn_id, 0) + int(m.group(1))
                # cfn applies only to the immediately following calls= line;
                # subsequent cost lines belong to the caller again.
                cur_cfn_id = None
                continue
    return names, counts


def demangle(names_list):
    try:
        p = subprocess.run(['rustfilt'], input='\n'.join(names_list),
                            capture_output=True, text=True, timeout=30)
        if p.returncode == 0:
            return p.stdout.split('\n')
    except Exception:
        pass
    return names_list


if __name__ == '__main__':
    if len(sys.argv) < 2:
        print(__doc__)
        sys.exit(2)
    path = sys.argv[1]
    do_demangle = '--demangle' in sys.argv[2:]

    names, counts = parse(path)
    ids = list(counts.keys())
    raw_names = [names.get(i, f'0x?{i}') for i in ids]
    final_names = demangle(raw_names) if do_demangle else raw_names

    rows = sorted(zip(ids, final_names, [counts[i] for i in ids]),
                   key=lambda t: -t[2])
    for _id, nm, c in rows:
        print(f"{c}\t{nm}")
