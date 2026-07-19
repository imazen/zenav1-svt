#!/usr/bin/env python3
"""Transcribe SVT-AV1 q_matrices.h (wt_matrix_ref / iwt_matrix_ref) into
crates/svtav1-encoder/src/qm_tables.rs. Values are copied verbatim; the
c_parity_qm test validates them end-to-end through the exported C
quantize kernels (which receive our slices as qm_ptr/iqm_ptr)."""
import re, sys

SRC = sys.argv[1] if len(sys.argv) > 1 else "/root/svt-av1-hdr-on-4.2/Source/Lib/Codec/q_matrices.h"
OUT = "crates/svtav1-encoder/src/qm_tables.rs"
QM_TOTAL_SIZE = 3344

s = open(SRC).read()
s = re.sub(r"/\*.*?\*/", "", s, flags=re.S)
s = re.sub(r"//[^\n]*", "", s)

def grab(name):
    i = s.index(name)
    start = s.index("{", i)
    depth = 0
    for j in range(start, len(s)):
        if s[j] == "{": depth += 1
        elif s[j] == "}":
            depth -= 1
            if depth == 0:
                body = s[start:j+1]
                return [int(x) for x in re.findall(r"\d+", body)]
    raise RuntimeError(name)

wt = grab("wt_matrix_ref")
iwt = grab("iwt_matrix_ref")
per = 2 * QM_TOTAL_SIZE
assert len(wt) % per == 0 and len(iwt) == len(wt), (len(wt), len(iwt))
levels = len(wt) // per
print(f"levels with data: {levels}, ints per table: {len(wt)}")
assert max(wt) <= 255 and max(iwt) <= 255 and min(wt) >= 0 and min(iwt) >= 0

def emit(f, name, vals):
    f.write(f"pub static {name}: [[[u8; QM_TOTAL_SIZE]; 2]; {levels}] = [\n")
    for q in range(levels):
        f.write("    [\n")
        for c in range(2):
            block = vals[(q*2+c)*QM_TOTAL_SIZE:(q*2+c+1)*QM_TOTAL_SIZE]
            f.write("        [")
            f.write(",".join(str(v) for v in block))
            f.write("],\n")
        f.write("    ],\n")
    f.write("];\n")

with open(OUT, "w") as f:
    f.write("""//! AV1 quantization-matrix weight tables, transcribed verbatim from
//! SVT-AV1 `q_matrices.h` (`wt_matrix_ref` / `iwt_matrix_ref`) by
//! `xtask/transcribe_qm.py`. Do not hand-edit. Validated end-to-end by
//! `tests/c_parity_qm.rs`, which feeds these slices as qm/iqm pointers to
//! the exported C quantize kernels and compares against the Rust port.
//!
//! Layout matches C: per level, per {luma, chroma}, the concatenation of
//! one flattened matrix per SELF-ADJUSTED tx size in TX_SIZES_ALL order
//! (see `qm::qm_offset`). Level 15 has no matrices (identity).
#![allow(clippy::all)]

pub const QM_TOTAL_SIZE: usize = 3344;

""")
    emit(f, "WT_MATRIX_REF", wt)
    f.write("\n")
    emit(f, "IWT_MATRIX_REF", iwt)
print(f"wrote {OUT}")
