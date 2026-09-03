#!/usr/bin/env bash
# Does the C ORACLE support the configuration this port REFUSES?
#
# WHY THIS EXISTS. `docs/REFUSED-CONFIGS.md` splits refusals into CONTRACT
# (caller misuse, permanent) and CAPABILITY (unported, debt). That split is
# derived from the WORDING of the refusal message, which makes it a claim about
# what someone typed, not about the world. A third question decides what a
# CAPABILITY refusal is actually worth working on, and nothing asked it:
#
#     can C v4.2.0 encode this configuration at all?
#
# Three answers, three very different backlog items:
#
#   C-REJECTS   C refuses it too (bit depth 12: `svt_av1_verify_settings`,
#               enc_settings.c:460). Our refusal is CORRECT and PERMANENT; the
#               only defect possible is a message that frames an upstream
#               constraint as our debt. There is no work to do and never will
#               be — implementing it would put the port OUTSIDE C's envelope
#               with no oracle to check it against.
#   C-ABSENT    C has no such mode at all, so there is no oracle even in
#               principle. MONOCHROME is the whole of this class: v4.2.0's
#               `verify_settings` rejects any `encoder_color_format` other than
#               EB_YUV420 ("Only support 420 now", enc_settings.c:473) and the
#               string "monochrome" does not appear in its App, its
#               `enc_settings.c` or its public headers. A mono gap is real debt,
#               but byte-parity can NEVER be its evidence — the established
#               substitute in this repo is the recon oracle plus decodability
#               (`tools/regression_spotcheck.sh`'s `monoReconEq`).
#   C-ACCEPTS   C encodes it. This is the only class where "implement it and
#               prove byte-parity" is a coherent instruction, and it is
#               therefore the only class a ranked backlog should be drawn from.
#
# Usage:  tools/c_envelope_probe.sh [out.tsv]
# Env:    PROBE_SIZE (default 64), PROBE_KEEP=1 to keep the .obu files.
#
# The probe drives `capture_c_trace`, i.e. the REAL library through the REAL
# public API, not a reading of `enc_settings.c`. Reading the source is the other
# half of the evidence and is cited per row in the ledger this feeds; a source
# reading alone has been wrong in this repo before (docs/WORKING-ON-THIS.md §5).
#
# POSITIVE AND NEGATIVE CONTROLS ARE MANDATORY AND ARE ROWS 1 AND 2. A probe
# whose harness is silently broken reports "C rejects everything", which reads
# exactly like a useful finding — §5 again. `baseline` must ACCEPT and
# `bitdepth-12` must REJECT; if either control comes out wrong the script exits
# non-zero and says the probe itself is untrustworthy, rather than emitting a
# table of zeros.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
cd "$HERE/.."

OUT=${1:-}
SZ=${PROBE_SIZE:-64}
CT="$HERE/capture_c_trace/capture_c_trace"
W="${TMPDIR:-$HOME/tmp}/c-envelope.$$"
mkdir -p "$W"
[[ -n "${PROBE_KEEP:-}" ]] || trap 'rm -rf "$W"' EXIT

# --- inputs -----------------------------------------------------------------
# Generated here rather than reused from identity_run so the probe has no
# dependency on a Rust build: it must be runnable when the port does not
# compile, which is exactly when "what does C do?" matters most.
gen() { # <bit_depth> <frames> <shift> <mode> <path>
    python3 - "$SZ" "$1" "$2" "$3" "$4" "$5" <<'PY'
import sys
n, bd, frames, shift, mode, out = (int(sys.argv[1]), int(sys.argv[2]),
                                   int(sys.argv[3]), int(sys.argv[4]),
                                   sys.argv[5], sys.argv[6])
w = h = n
cw, chh = (w + 1) // 2, (h + 1) // 2
maxv = (1 << bd) - 1
mid = 1 << (bd - 1)
def lum(x, y):
    if mode == "screen":
        # Hard-edged blocks with a tiny palette: what arms C's screen-content
        # detector. A gradient never does (docs/WORKING-ON-THIS.md §5).
        return 0 if ((x // 8) + (y // 8)) % 2 == 0 else maxv
    return ((x + y) * maxv) // max(1, w + h - 2)
def put(f, vals):
    f.write(bytes(vals) if bd == 8 else b"".join(v.to_bytes(2, "little") for v in vals))
with open(out, "wb") as f:
    for fr in range(frames):
        s = fr * shift
        for y in range(h):
            put(f, [lum((x + s) % w, y) for x in range(w)])
        for _ in range(2):
            for _ in range(chh):
                put(f, [mid] * cw)
PY
}
gen 8  1 0 gradient "$W/g8.yuv"
gen 10 1 0 gradient "$W/g10.yuv"
gen 8  2 3 gradient "$W/g8f2.yuv"
gen 8  1 0 screen   "$W/s8.yuv"

# --- the probe ---------------------------------------------------------------
rows=()
probe() { # <label> <qp> <preset> <bd> <yuv> [env...]
    local label=$1 qp=$2 preset=$3 bd=$4 yuv=$5; shift 5
    local log="$W/$label.log" obu="$W/$label.obu"
    rm -f "$obu"
    env "$@" SVT_TRACE_OUT=/dev/null "$CT" "$SZ" "$SZ" "$qp" "$preset" \
        "$yuv" "$obu" "$bd" >"$log" 2>&1
    local rc=$? bytes=0 verdict
    [[ -f "$obu" ]] && bytes=$(wc -c <"$obu" | tr -d ' ')
    if grep -q 'Svt\[error\]' "$log"; then
        verdict=C-REJECTS
    elif ((rc == 0 && bytes > 0)); then
        verdict=C-ACCEPTS
    else
        verdict="C-FAILED(rc=$rc)"
    fi
    rows+=("$label	$verdict	$bytes")
    printf '%-26s %-16s %8s B\n' "$label" "$verdict" "$bytes"
}

echo "C ENVELOPE PROBE — ${SZ}x${SZ}, library $(cd "$HERE/../../reference/svt-av1" && git rev-parse --short HEAD 2>/dev/null || echo '?')"
echo

# Controls first (see header).
probe baseline              32 8 8  "$W/g8.yuv"
probe bitdepth-12           32 8 12 "$W/g10.yuv"

# The CAPABILITY refusals, one row each.
probe qp0-8bit-still        0  8 8  "$W/g8.yuv"
probe qp0-10bit             0  8 10 "$W/g10.yuv"
probe qp0-screen-content    0  8 8  "$W/s8.yuv"
probe qp0-superres          0  8 8  "$W/g8.yuv"  SVT_SUPERRES_KF_DENOM=16
probe qp0-inter             0  8 8  "$W/g8f2.yuv" SVT_FRAMES=2 SVT_AVIF=0 \
                                                  SVT_INTRA_PERIOD=-1 SVT_HIER_LEVELS=0 \
                                                  SVT_PRED_STRUCT=1
probe superres-10bit        32 8 10 "$W/g10.yuv" SVT_SUPERRES_KF_DENOM=16
probe superres-8bit         32 8 8  "$W/g8.yuv"  SVT_SUPERRES_KF_DENOM=16
probe inter-preset4         32 4 8  "$W/g8f2.yuv" SVT_FRAMES=2 SVT_AVIF=0 \
                                                  SVT_INTRA_PERIOD=-1 SVT_HIER_LEVELS=0 \
                                                  SVT_PRED_STRUCT=1
probe inter-intraperiod-1   32 8 8  "$W/g8f2.yuv" SVT_FRAMES=2 SVT_AVIF=0 \
                                                  SVT_INTRA_PERIOD=1 SVT_HIER_LEVELS=0 \
                                                  SVT_PRED_STRUCT=1
probe inter-randomaccess    32 8 8  "$W/g8f2.yuv" SVT_FRAMES=2 SVT_AVIF=0 \
                                                  SVT_INTRA_PERIOD=-1 SVT_HIER_LEVELS=1 \
                                                  SVT_PRED_STRUCT=2

# MONOCHROME cannot be probed: there is no parameter to ask for it.
# `verify_settings` rejects any `encoder_color_format != EB_YUV420`
# (enc_settings.c:473) and no mono mode exists in the App, the settings or the
# public headers. Recorded as a row so the ledger cannot silently omit the
# largest class.
rows+=("monochrome	C-ABSENT	0")
printf '%-26s %-16s %8s\n' "monochrome" "C-ABSENT" "(no such mode)"

# --- controls ----------------------------------------------------------------
base=$(printf '%s\n' "${rows[@]}" | awk -F'\t' '$1=="baseline"{print $2}')
bd12=$(printf '%s\n' "${rows[@]}" | awk -F'\t' '$1=="bitdepth-12"{print $2}')
echo
if [[ "$base" != C-ACCEPTS || "$bd12" != C-REJECTS ]]; then
    echo "c_envelope_probe: CONTROLS FAILED (baseline=$base bitdepth-12=$bd12)." >&2
    echo "  The probe cannot distinguish 'C rejects this' from 'the harness is broken'," >&2
    echo "  so every row above is untrustworthy. Fix the driver before reading them." >&2
    exit 1
fi
echo "controls OK: baseline accepts, bit depth 12 rejects."

if [[ -n "$OUT" ]]; then
    {
        echo "# does C v4.2.0 support the configuration this port refuses?"
        echo "# host=$(hostname)  date=$(date -u +%Y-%m-%dT%H:%M:%SZ)  size=${SZ}x${SZ}"
        echo "# driver=tools/capture_c_trace (the real library, real public API)"
        echo "# C-ACCEPTS: an oracle exists.  C-REJECTS: upstream constraint, permanent."
        echo "# C-ABSENT:  no such mode in C at all — byte-parity can never be the evidence."
        printf 'config\tc_verdict\tbytes\n'
        printf '%s\n' "${rows[@]}"
    } >"$OUT"
    echo "wrote $OUT"
fi
