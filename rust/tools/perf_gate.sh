#!/usr/bin/env bash
# G4 performance gate — interleaved paired-statistics wall-time harness.
#
# Measures the PORT's per-frame still-image encode wall time against the REAL C
# reference library, on the BYTE-IDENTICAL envelope only (so both encoders do
# the same work — apples-to-apples). Per the acceptance criteria (docs/
# ACCEPTANCE-CRITERIA.md, "Performance"):
#   * INTERLEAVED PAIRED statistics — each round runs port and C back-to-back in
#     RANDOMIZED order, so thermal/turbo drift cancels within the pair. NOT
#     back-to-back isolated blocks of runs.
#   * NO -C target-cpu=native — the port release is built with runtime SIMD
#     dispatch (what users get); the C lib ships the same.
#   * total = intercept + slope * pixels FIT across tiny/small/medium/large, so
#     fixed per-call overhead never hides inside a single "ms/MP" figure. Both
#     the intercept AND the slope are reported, per preset, for port and C.
#   * Every measured size is measured — nothing is extrapolated from another.
#
# Both harnesses time ONLY the frame encode; one-time setup (port
# `EncodePipeline::new` / C `svt_av1_enc_init`: table build, alloc, thread
# spawn) is excluded on both sides. See tools/perf_c_encode/perf_c_encode.c and
# svtav1/examples/perf_encode.rs.
#
# Each (content,size,preset) cell is byte-verified (port .obu == C .obu) before
# its ratio is trusted; a non-identical cell is measured but flagged and left
# OUT of the intercept/slope fit — its ratio would compare different work.
#
# Target: <= ~1.2x C. This is the LAST gate; the port has not been perf-tuned,
# so the current baseline is expected to be well over 1.2x. The point is to
# MEASURE honestly, establish the re-runnable harness + baseline, and surface
# the hotspots.
#
# Usage:   tools/perf_gate.sh [date-suffix]         (default: today's date)
# Env (all overridable):
#   PERF_SIZES   square dims to sweep     (default "64 128 256 512 1024")
#   PERF_PRESETS byte-exact presets       (default "6 10 13")
#   PERF_CONTENT synthetic content        (default "gradient"; or "uniform")
#   PERF_QP      CLI qp 0..63             (default 40)
#   PERF_ROUNDS  interleaved paired rounds/cell (default 20)
#   PERF_WARMUP  untimed warmup encodes/spawn    (default 1)
#
# Writes:
#   benchmarks/perf_<suffix>.tsv       per-cell summary (committed)
#   benchmarks/perf_<suffix>.raw.tsv   every paired sample (auditable)
#   benchmarks/perf_<suffix>.meta      provenance + fits + verdict
set -uo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
cd "$RS_ROOT"

SUFFIX="${1:-$(date +%Y-%m-%d)}"
OUT="$RS_ROOT/benchmarks/perf_${SUFFIX}.tsv"
RAW="$RS_ROOT/benchmarks/perf_${SUFFIX}.raw.tsv"
META="$RS_ROOT/benchmarks/perf_${SUFFIX}.meta"
mkdir -p "$RS_ROOT/benchmarks"

read -r -a SIZES <<<"${PERF_SIZES:-64 128 256 512 1024}"
read -r -a PRESETS <<<"${PERF_PRESETS:-6 10 13}"
CONTENT="${PERF_CONTENT:-gradient}"
QP="${PERF_QP:-40}"
ROUNDS="${PERF_ROUNDS:-20}"
WARMUP="${PERF_WARMUP:-1}"

PE="$RS_ROOT/target/release/examples/perf_encode"
CE="$HERE/perf_c_encode/perf_c_encode"
WORK="$RS_ROOT/target/perf"
mkdir -p "$WORK"

echo "=== perf_gate: building port example (release, NO target-cpu=native) ==="
# `env -u RUSTFLAGS` guarantees no ambient -C target-cpu=native leaks in.
if ! nice -n 19 ionice -c 3 env -u RUSTFLAGS \
    cargo build --release -p zenav1-svt --example perf_encode 2>&1 | tail -3; then
    echo "perf_gate: port build failed" >&2
    exit 1
fi
echo "=== perf_gate: building C reference harness ==="
if ! nice -n 19 ionice -c 3 "$HERE/perf_c_encode/build.sh" 2>&1 | tail -3; then
    echo "perf_gate: C harness build failed" >&2
    exit 1
fi
[[ -x "$PE" && -x "$CE" ]] || { echo "perf_gate: harness binaries missing" >&2; exit 1; }

COMMIT=$(git rev-parse --short HEAD 2>/dev/null || echo unknown)
HOST=$(hostname)
NCORES=$(nproc 2>/dev/null || echo "?")
GRID="content=$CONTENT sizes=[${SIZES[*]}] presets=[${PRESETS[*]}] qp=$QP rounds=$ROUNDS warmup=$WARMUP"

# --- raw per-sample TSV (auditable provenance of every number) --------------
{
    echo "# perf_gate raw samples — one row per interleaved paired round"
    echo "# commit=$COMMIT host=$HOST cores=$NCORES date=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    echo "# $GRID"
    echo "# port=svtav1/examples/perf_encode (release, no native)  C=tools/perf_c_encode (libSvtAv1Enc.a)"
    printf 'content\tsize\tpreset\tqp\tround\tport_ns\tc_ns\tident\n'
} >"$RAW"

# port half — regenerates the .yuv (untimed, deterministic) so C reads identical
# bytes, then prints ENCODE_NS for the timed encode.
run_port() { # <size> <preset>
    "$PE" "$CONTENT" "$1" "$1" "$QP" "$2" "$WORK/cell" "$WARMUP" 2>/dev/null \
        | sed -n 's/.*ENCODE_NS=\([0-9]*\).*/\1/p'
}
run_c() { # <size> <preset>
    "$CE" "$1" "$1" "$QP" "$2" "$WORK/cell.yuv" "$WORK/cell.c.obu" "$WARMUP" 2>/dev/null \
        | sed -n 's/.*ENCODE_NS=\([0-9]*\).*/\1/p'
}

echo
echo "=== perf_gate sweep: $GRID ==="
for sz in "${SIZES[@]}"; do
    for preset in "${PRESETS[@]}"; do
        # Identity pre-pass (runs port FIRST so the .yuv exists for C):
        # confirm this cell is byte-identical before trusting its ratio.
        run_port "$sz" "$preset" >/dev/null
        run_c "$sz" "$preset" >/dev/null
        if cmp -s "$WORK/cell.obu" "$WORK/cell.c.obu"; then
            ident="Y"
        else
            ident="N"
            echo "  WARN ${sz}x${sz} p${preset}: NOT byte-identical — ratio not apples-to-apples (excluded from fit)"
        fi

        # Interleaved paired rounds, order randomized per round.
        for ((r = 1; r <= ROUNDS; r++)); do
            if (( RANDOM % 2 )); then
                pns=$(run_port "$sz" "$preset"); cns=$(run_c "$sz" "$preset")
            else
                cns=$(run_c "$sz" "$preset"); pns=$(run_port "$sz" "$preset")
            fi
            [[ -n "$pns" && -n "$cns" ]] || { echo "  ERR ${sz}x${sz} p${preset} round $r: empty timing" >&2; continue; }
            printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
                "$CONTENT" "$sz" "$preset" "$QP" "$r" "$pns" "$cns" "$ident" >>"$RAW"
        done
        printf '  measured %sx%s p%-2s ident=%s (%d rounds)\n' "$sz" "$sz" "$preset" "$ident" "$ROUNDS"
        # keep the coordination marker fresh across the long sweep
        printf '%s claude-perf-g4 perf-sweep-%sx%s-p%s\n' \
            "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$sz" "$sz" "$preset" > "$RS_ROOT/.workongoing" 2>/dev/null || true
    done
done

# --- analysis: per-cell medians + paired-ratio IQR, then per-preset fits -----
# One gawk pass over the raw samples produces the summary TSV, the fit lines,
# and the printed report. Median/percentile via asort (nearest-rank).
gawk -v commit="$COMMIT" -v host="$HOST" -v cores="$NCORES" -v grid="$GRID" \
     -v out="$OUT" -v meta="$META" -v content="$CONTENT" '
function median(a, n,   b, m) { m = asort(a, b); if (m == 0) return 0;
    return (m % 2) ? b[(m + 1) / 2] : (b[m / 2] + b[m / 2 + 1]) / 2 }
function pct(a, n, p,   b, m, i) { m = asort(a, b); if (m == 0) return 0;
    i = int(p / 100 * m + 0.5); if (i < 1) i = 1; if (i > m) i = m; return b[i] }
BEGIN { FS = OFS = "\t" }
/^#/ || /^content\t/ { next }
{
    key = $2 SUBSEP $3                 # size, preset
    n = ++cnt[key]
    pns[key, n] = $6; cns[key, n] = $7
    ratio[key, n] = $6 / $7
    ident[key] = $8
    sizes[$2] = 1; presets[$3] = 1
}
END {
    # provenance header on the committed summary TSV
    print "# perf_gate summary — port vs C per-frame encode wall time (byte-identical envelope)" > out
    print "# commit=" commit "  host=" host "  cores=" cores "  date=" strftime("%Y-%m-%dT%H:%M:%SZ", systime(), 1) > out
    print "# " grid > out
    print "# ratio = port/C (median of per-round paired ratios); target <= ~1.2x. ident=Y means byte-identical (apples-to-apples)." > out
    print "size\tpreset\tident\tn\tport_ms\tc_ms\tratio\tratio_p25\tratio_p75" > out

    ns = 0; for (s in sizes) sz_list[++ns] = s + 0
    np = 0; for (p in presets) pr_list[++np] = p + 0
    asort(sz_list); asort(pr_list)

    # per-cell summary rows (sorted size major, preset minor)
    for (i = 1; i <= ns; i++) for (j = 1; j <= np; j++) {
        s = sz_list[i]; p = pr_list[j]; key = s SUBSEP p
        if (!(key in cnt)) continue
        n = cnt[key]
        delete pa; delete ca; delete ra
        for (k = 1; k <= n; k++) { pa[k] = pns[key, k]; ca[k] = cns[key, k]; ra[k] = ratio[key, k] }
        pmed = median(pa, n) / 1e6; cmed = median(ca, n) / 1e6
        rmed = median(ra, n); r25 = pct(ra, n, 25); r75 = pct(ra, n, 75)
        printf "%d\t%d\t%s\t%d\t%.3f\t%.3f\t%.3f\t%.3f\t%.3f\n",
            s, p, ident[key], n, pmed, cmed, rmed, r25, r75 > out
        # stash medians for the fit (identical cells only)
        if (ident[key] == "Y") { PM[p, s] = pmed; CM[p, s] = cmed; HAVE[p, s] = 1 }
    }
    close(out)

    # per-preset linear fit  ms = intercept + slope * pixels  (pixels = size^2)
    # least squares over the byte-identical sizes. Report both coefficients.
    fitcount = 0
    for (j = 1; j <= np; j++) {
        p = pr_list[j]
        n = 0; sx = sy_p = sxx = sxy_p = sy_c = sxy_c = 0
        for (i = 1; i <= ns; i++) {
            s = sz_list[i]; if (!((p, s) in HAVE)) continue
            x = s * s; n++
            sx += x; sxx += x * x
            sy_p += PM[p, s]; sxy_p += x * PM[p, s]
            sy_c += CM[p, s]; sxy_c += x * CM[p, s]
        }
        if (n < 2) continue
        den = n * sxx - sx * sx
        bp = (n * sxy_p - sx * sy_p) / den; ap = (sy_p - bp * sx) / n   # port ms = ap + bp*px
        bc = (n * sxy_c - sx * sy_c) / den; ac = (sy_c - bc * sx) / n   # C   ms = ac + bc*px
        # slope in ms per megapixel for readability
        FIT[++fitcount] = sprintf("p%-2d  port: intercept=%.3f ms  slope=%.4f ms/MP   |   C: intercept=%.3f ms  slope=%.4f ms/MP   |   slope-ratio=%.2fx  intercept-ratio=%.2fx",
            p, ap, bp * 1e6, ac, bc * 1e6, (bc != 0 ? bp / bc : 0), (ac != 0 ? ap / ac : 0))
    }

    # printed report + META sidecar
    print "" ; print "==== PER-CELL RATIOS (port/C, median of paired rounds) ===="
    printf "%-6s %-7s %-6s %-10s %-10s %-8s %-14s\n", "size", "preset", "ident", "port_ms", "C_ms", "ratio", "[p25,p75]"
    for (i = 1; i <= ns; i++) for (j = 1; j <= np; j++) {
        s = sz_list[i]; p = pr_list[j]; key = s SUBSEP p; if (!(key in cnt)) continue
        n = cnt[key]; delete pa; delete ca; delete ra
        for (k = 1; k <= n; k++) { pa[k] = pns[key, k]; ca[k] = cns[key, k]; ra[k] = ratio[key, k] }
        printf "%-6d %-7d %-6s %-10.3f %-10.3f %-8.2f [%.2f, %.2f]\n",
            s, p, ident[key], median(pa,n)/1e6, median(ca,n)/1e6, median(ra,n), pct(ra,n,25), pct(ra,n,75)
    }
    print "" ; print "==== INTERCEPT + SLOPE FIT  (ms = intercept + slope * pixels, byte-identical sizes) ===="
    for (f = 1; f <= fitcount; f++) print FIT[f]

    print "# perf_gate fit + verdict" > meta
    print "date: " strftime("%Y-%m-%dT%H:%M:%SZ", systime(), 1) > meta
    print "commit: " commit > meta
    print "host: " host " (" cores " cores)" > meta
    print "content: " content > meta
    print "grid: " grid > meta
    print "harness: interleaved paired (randomized per-round order); no target-cpu=native; setup excluded both sides" > meta
    print "target: <= ~1.2x C wall clock (matched preset+qp, byte-identical envelope)" > meta
    print "fits (ms = intercept + slope*pixels):" > meta
    for (f = 1; f <= fitcount; f++) print "  " FIT[f] > meta
    close(meta)
}
' "$RAW"

echo
echo "summary : $OUT"
echo "raw     : $RAW"
echo "meta    : $META"
