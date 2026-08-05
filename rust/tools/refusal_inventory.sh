#!/usr/bin/env bash
# Inventory every config this encoder REFUSES, split into the two kinds that
# look identical in a gate log and are not remotely the same thing.
#
#   tools/refusal_inventory.sh           # rewrite docs/REFUSED-CONFIGS.md
#   tools/refusal_inventory.sh --check   # exit 1 if that file is stale (CI)
#
# WHY THIS EXISTS
#
# This port has a correct and load-bearing rule: refuse an out-of-envelope
# config rather than emit a plausible-but-wrong bitstream. The rule is right.
# The SIDE EFFECT is what this tool is for.
#
# A refusal makes a gap look handled:
#
#   - `arbitrary_size_robustness.sh` reports "80 / 80 panic-free +
#     aomdec-decodable (48 refused as out-of-envelope)". The 48 refusals are
#     counted as PASSES, because refusing IS the correct behaviour. Nothing in
#     that line distinguishes "genuinely out of scope" from "nobody has done
#     the work yet".
#   - `coverage_matrix.py` prints `--` for an untested axis, which is the right
#     instinct — but a REFUSED config never produces a cell at all, so it cannot
#     even show as `--`. Refusals are invisible to the one tool built to surface
#     gaps.
#   - Nothing ages them. No inventory, no owner, no expiry.
#
# Measured consequence (2026-08-04): 10-bit at non-64-aligned dimensions — the
# actual product case for AVIF — sat refused behind
# `bit_depth_config_error` while every gate stayed green. It was read aloud in a
# status report the same day and moved past, because the surrounding scoreboard
# said everything was fine.
#
# THE TWO KINDS
#
#   CONTRACT  — the caller asked for something incoherent, or did not enable the
#               mode they are using. Permanent and correct; e.g.
#               "encode_frame_420 requires the pipeline to be built with
#               with_chroma_420(true)". These need no tracking.
#   CAPABILITY — the port does not implement it yet. "not implemented",
#               "unported", "so far", "no <x> producer is <y> aware". THESE ARE
#               DEBT. A capability refusal is a feature request the compiler
#               happens to enforce, and it should be as visible as a failing
#               test.
#
# The classification is keyword-based on the refusal message, which means it is
# only as good as the messages. That is deliberate: a capability refusal whose
# text does not say it is unimplemented is ALSO a defect, because the caller
# cannot tell either.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
cd "$RS_ROOT"

DOC=docs/REFUSED-CONFIGS.md
SRC=(crates/svtav1-encoder/src/pipeline.rs svtav1/src/avif.rs)

# A refusal message spanning continuation lines is joined before matching, so a
# wrapped string cannot hide its own keywords.
collect() {
    # Match refusal CONSTRUCTS, not "any string that sounds like a refusal".
    #
    # The first version regexed every long string literal containing
    # must/only/not. That swept in comment prose and `debug_assert!` messages
    # ("HORZ/VERT children must be leaf blocks, not split nodes"), which are not
    # refusals at all — so the ledger was both wrong and, because the noise
    # shifted with unrelated edits, unstable. A refusal is a string handed to
    # `UnsupportedConfig(...)`, or returned as `Some("...")` from a
    # `*_config_error` predicate. Nothing else counts.
    python3 - "${SRC[@]}" <<'PY'
import re, sys

CAP = re.compile(
    r"not implemented|unported|so far|is not yet|no bd10|not partial-sb|"
    r"has no .*(stage|kernels|producer)|requires \d+-aligned|only on the pd0 path|"
    r"needs preset|8-bit only|would be 8-bit-quantized",
    re.I,
)

UNSUP = re.compile(r'UnsupportedConfig\(\s*"((?:[^"\\]|\\.)*)"')
# A third construct: `EncodeError::InvalidDimensions { reason: "..." }`. Missing
# it dropped the two real monochrome-geometry refusals from the ledger.
REASON = re.compile(r'\breason:\s*"((?:[^"\\]|\\.)*)"')
# Trailing comma and `.to_string()` are both common; requiring a bare
# `)` right after the string silently dropped half the real refusals.
SOMES = re.compile(r'\bSome\(\s*"((?:[^"\\]|\\.)*)"\s*(?:\.to_string\(\))?\s*,?\s*\)')
FNDEF = re.compile(r"\n\s*(?:pub(?:\(crate\))?\s+)?fn\s+([a-z_0-9]+)")

for path in sys.argv[1:]:
    src = open(path).read()
    joined = re.sub(r"\\\s*\n\s*", " ", src)   # join Rust string continuations

    found = set(m.group(1) for m in UNSUP.finditer(joined))
    found |= set(m.group(1) for m in REASON.finditer(joined))

    # `Some("...")` counts only inside a fn whose name ends in _config_error.
    bounds = [(m.start(), m.group(1)) for m in FNDEF.finditer(joined)]
    for i, (pos, name) in enumerate(bounds):
        if not name.endswith("_config_error"):
            continue
        stop = bounds[i + 1][0] if i + 1 < len(bounds) else len(joined)
        for m in SOMES.finditer(joined[pos:stop]):
            found.add(m.group(1))

    for msg in found:
        msg = " ".join(msg.split())
        if len(msg) < 20:
            continue
        kind = "CAPABILITY" if CAP.search(msg) else "CONTRACT"
        print(f"{kind}\t{path}\t{msg}")
PY
}

generate() {
    local rows cap con
    # LC_ALL=C: byte collation, not locale collation. Without it macOS and the
    # Linux CI runner order the same rows differently and `--check` fails on a
    # diff that is pure sort order — which is exactly how this gate first went
    # red.
    rows=$(collect | LC_ALL=C sort -u)
    cap=$(printf '%s\n' "$rows" | grep -c '^CAPABILITY' || true)
    con=$(printf '%s\n' "$rows" | grep -c '^CONTRACT' || true)

    cat <<EOF
<!-- generated by tools/refusal_inventory.sh — do not edit by hand -->

# Configs this encoder refuses

**${cap} CAPABILITY refusals** (unimplemented — this is DEBT) and **${con}
CONTRACT refusals** (caller misuse — permanent and correct).

Regenerate with \`tools/refusal_inventory.sh\`; \`--check\` is a CI gate.

## Why this file exists

Refusing beats emitting a wrong bitstream — that rule is correct and stays. But
a refusal also makes a gap look handled: \`arbitrary_size_robustness.sh\` counts
its 48 refusals as PASSES, and \`coverage_matrix.py\` cannot show a refused
config even as \`--\`, because a refused config produces no cell at all. So the
one tool built to surface gaps is structurally blind to this one.

That is not hypothetical. 10-bit at non-64-aligned dimensions — the actual AVIF
product case — sat behind a refusal while every gate stayed green, and was read
aloud in a status report before anyone acted on it.

**Read the CAPABILITY list as a backlog, not as a specification.**

## CAPABILITY — not implemented (debt)

| where | refusal |
|---|---|
EOF
    printf '%s\n' "$rows" | awk -F'\t' '$1=="CAPABILITY"{printf "| `%s` | %s |\n", $2, $3}'

    cat <<EOF

## CONTRACT — caller misuse (permanent, correct)

| where | refusal |
|---|---|
EOF
    printf '%s\n' "$rows" | awk -F'\t' '$1=="CONTRACT"{printf "| `%s` | %s |\n", $2, $3}'
}

tmp=$(mktemp)
generate > "$tmp"

if [[ "${1:-}" == "--check" ]]; then
    if [[ -f "$DOC" ]] && diff -q "$DOC" "$tmp" >/dev/null; then
        echo "refusal_inventory: current ($(grep -c '^| `' "$DOC" || true) refusals catalogued)"
        rm -f "$tmp"
        exit 0
    fi
    echo "refusal_inventory: $DOC is STALE." >&2
    echo "  The set of configs this encoder refuses has changed." >&2
    echo "  Run tools/refusal_inventory.sh and commit the result." >&2
    [[ -f "$DOC" ]] && diff "$DOC" "$tmp" | head -30 >&2
    rm -f "$tmp"
    exit 1
fi

mkdir -p "$(dirname "$DOC")"
mv "$tmp" "$DOC"
echo "refusal_inventory: wrote $DOC"
grep -c '^| `' "$DOC" | sed 's/^/  refusals catalogued: /'
