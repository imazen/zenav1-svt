#!/usr/bin/env python3
"""Generate vectorized *_x8 DCT kernels from the scalar butterfly source.

Mechanical, uniform translation of the scalar 1D kernels (idct32/64, fdct32/64)
into archmage AVX2 form, mirroring the hand-written+proven idct16_x8/fdct16_x8.
Every scalar op maps 1:1 to a vector op, so output is bit-identical by
construction; the c_parity differential is the ground-truth check.
"""
import re
import sys

def extract_body(src_path, fn_name):
    src = open(src_path).read()
    m = re.search(r'pub fn %s\(' % re.escape(fn_name), src)
    if not m:
        raise SystemExit("fn %s not found" % fn_name)
    # brace-match the function body
    i = src.index('{', m.start())
    depth = 0
    for j in range(i, len(src)):
        if src[j] == '{': depth += 1
        elif src[j] == '}':
            depth -= 1
            if depth == 0:
                return src[i+1:j]
    raise SystemExit("unbalanced braces")

def split_top_commas(s):
    out, depth, cur = [], 0, ''
    for ch in s:
        if ch in '([': depth += 1
        elif ch in ')]': depth -= 1
        if ch == ',' and depth == 0:
            out.append(cur.strip()); cur = ''
        else:
            cur += ch
    if cur.strip(): out.append(cur.strip())
    return out

def ref(tok, prev):
    """Map a scalar value reference to the vector SSA array element."""
    tok = tok.strip()
    m = re.fullmatch(r'input\[(\d+)\]', tok)
    if m: return 'inp[%s]' % m.group(1)
    m = re.fullmatch(r'(?:o|s)\((\d+)\)', tok)
    if m: return '%s[%s]' % (prev, m.group(1))
    m = re.fullmatch(r'(?:output|step)\[(\d+)\]', tok)
    if m: return '%s[%s]' % (prev, m.group(1))
    raise SystemExit("unref: %r" % tok)

def wt(tok):
    tok = tok.strip()
    m = re.fullmatch(r'-cospi\[(\d+)\]', tok)
    if m: return 'cn!(t, cospi, %s)' % m.group(1)
    m = re.fullmatch(r'cospi\[(\d+)\]', tok)
    if m: return 'c!(t, cospi, %s)' % m.group(1)
    raise SystemExit("unwt: %r" % tok)

def vec(expr, prev):
    """Vectorize an add/sub/copy RHS (no half_btf, no clamp here)."""
    expr = expr.strip()
    # -A + B
    m = re.fullmatch(r'-\s*(.+?)\s*\+\s*(.+)', expr)
    if m: return 'sub!(%s, %s)' % (ref(m.group(2), prev), ref(m.group(1), prev))
    # A + B
    m = re.fullmatch(r'(.+?)\s*\+\s*(.+)', expr)
    if m: return 'add!(%s, %s)' % (ref(m.group(1), prev), ref(m.group(2), prev))
    # A - B
    m = re.fullmatch(r'(.+?)\s*-\s*(.+)', expr)
    if m: return 'sub!(%s, %s)' % (ref(m.group(1), prev), ref(m.group(2), prev))
    # bare copy
    return ref(expr, prev)

def rhs(expr, prev):
    expr = expr.strip().rstrip(';')
    m = re.fullmatch(r'half_btf\((.*)\)', expr, re.S)
    if m:
        a = split_top_commas(m.group(1))
        assert len(a) == 5 and a[4] == 'cos_bit', a
        return 'hbtf(t, %s, %s, %s, %s, rnd, sh)' % (
            wt(a[0]), ref(a[1], prev), wt(a[2]), ref(a[3], prev))
    m = re.fullmatch(r'clamp_value\((.*),\s*range\)', expr, re.S)
    if m:
        return 'cl(%s)' % vec(m.group(1), prev)
    return vec(expr, prev)

def parse_stages(body):
    """Return list of stages; each is dict{index:int -> rhs_str}."""
    stages = []
    cur = None
    for line in body.splitlines():
        s = line.strip()
        if s.startswith('// stage'):
            if cur is not None: stages.append(cur)
            cur = {}
            continue
        if cur is None:
            continue
        m = re.fullmatch(r'(?:output|step)\[(\d+)\]\s*=\s*(.+;)', s)
        if m:
            cur[int(m.group(1))] = m.group(2)
    if cur is not None: stages.append(cur)
    return stages

def gen(kind, n, fn_name, src_path):
    body = extract_body(src_path, fn_name)
    stages = parse_stages(body)
    ns = 'idct' if kind == 'inv' else 'fdct'
    out = []
    if kind == 'inv':
        out.append("#[rite]")
        out.append("pub(super) fn %s%d_x8(" % (ns, n))
        out.append("    t: Desktop64,")
        out.append("    inp: &[__m256i; %d]," % n)
        out.append("    out: &mut [__m256i; %d]," % n)
        out.append("    rnd: __m256i,")
        out.append("    sh: __m128i,")
        out.append("    lo: __m256i,")
        out.append("    hi: __m256i,")
        out.append(") {")
        out.append("    let cospi = &COSPI;")
        out.append("    let cl = |v| clampv(t, v, lo, hi);")
    else:
        out.append("#[rite]")
        out.append("pub(super) fn %s%d_x8(t: Desktop64, inp: &[__m256i; %d], out: &mut [__m256i; %d], cos_bit: i8) {" % (ns, n, n, n))
        out.append("    let cospi = cospi_arr(cos_bit);")
        out.append("    let rnd = splat(t, 1 << (cos_bit as u32 - 1));")
        out.append("    let sh = _mm_cvtsi32_si128(cos_bit as i32);")
    nstages = len(stages)
    for si, st in enumerate(stages):
        assert set(st.keys()) == set(range(n)), (si, sorted(st.keys()))
        prev = 'inp' if si == 0 else ('s%d' % si)
        elems = [rhs(st[i], prev) for i in range(n)]
        last = (si == nstages - 1)
        target = 'out' if last else ('let s%d' % (si + 1))
        out.append("    // stage %d" % (si + 1))
        if last:
            out.append("    *out = [")
        else:
            out.append("    %s: [__m256i; %d] = [" % (target, n))
        for i in range(0, n, 4):
            out.append("        " + ", ".join(elems[i:i+4]) + ",")
        out.append("    ];")
    out.append("}")
    return "\n".join(out)

if __name__ == '__main__':
    kind, n, fn, path = sys.argv[1], int(sys.argv[2]), sys.argv[3], sys.argv[4]
    print(gen(kind, n, fn, path))
