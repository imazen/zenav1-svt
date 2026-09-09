# Shared corpus-path resolution for the gate scripts. Source, don't execute:
#
#     . "$(dirname "$0")/lib_corpus.sh"
#     SCREEN_DIR="${SCREEN_DIR:-$(corpus_dir codec-corpus/gb82-sc)}"
#
# WHY. Fourteen gate scripts hard-coded `/root/work/codec-corpus/...`, the path
# on one CI image. On any other host every image "SKIP-MISSING"s and the gate
# either fails for a reason that has nothing to do with the port
# (`screen_palette_gate.sh`: "ANTI-VACUITY FAIL: no palette block coded") or --
# worse -- reports 0/0 and exits 0, which reads as a pass.
#
# That is the same failure this repo has now hit three times: `ionice` (absent
# off Linux, took the whole command down with it), `-Wl,--wrap` (absent on
# ld64, made every byte-parity gate unbuildable), and now a hard-coded corpus
# root. The pattern is assuming ONE host. The fix is the same each time: probe
# for what you need, degrade loudly, never silently.
#
# Resolution order -- first HIT wins, and an explicit env override always beats
# all of it:
#   1. $ZENAV1_CORPUS_ROOT   (set this if your corpora live somewhere else)
#   2. $HOME/work/zen        (the dev-workstation layout)
#   3. /root/work            (the CI image layout -- the old hard-coded root)
#   4. $HOME/work
#
# If nothing hits, the FIRST candidate is echoed so the caller's existing
# "SKIP-MISSING <path>" message names a plausible path rather than an empty
# string. A caller must still handle a missing corpus; this only stops the
# gates from looking in exactly one place.
corpus_dir() {
    local rel=$1 root
    for root in "${ZENAV1_CORPUS_ROOT:-}" "$HOME/work/zen" /root/work "$HOME/work"; do
        [ -n "$root" ] || continue
        if [ -d "$root/$rel" ]; then
            printf '%s\n' "$root/$rel"
            return 0
        fi
    done
    printf '%s\n' "${ZENAV1_CORPUS_ROOT:-$HOME/work/zen}/$rel"
    return 1
}

# ---------------------------------------------------------------------------
# screen_crop_spec <png-path>
#
# The identity_run content argument for a SCREEN-CONTENT cell.
#
# Defaults to `crop:` (the centre window). gb82-sc/gmessages.png is the one
# exception: its centre 512x512 is a single flat colour (0x0d141c, all 262144
# px), so a palette block cannot exist there and IntraBC cannot beat exact DC.
# 26 cells across screen_ibc_byte_gate, screen_palette_gate and
# bd10_screen_panic_gate were passing while structurally unable to reach the
# code paths those gates exist to guard -- MEASURED at 512x512 q20 p0: the
# centre window codes 46 B / 382 tile ops, a content-bearing window of the same
# image codes 5524 B / 60118 ops. Both are byte-identical to C, so this is a
# coverage fix, not a parity change (issue #23).
#
# Callers should also export SVTAV1_ASSERT_NONFLAT=1 so a future re-aim onto
# flat chrome fails loudly instead of silently passing.
screen_crop_spec() {
    local png="$1"
    case "$(basename "$png")" in
        gmessages.png) printf 'crop@384,384:%s' "$png" ;;
        *)             printf 'crop:%s' "$png" ;;
    esac
}
