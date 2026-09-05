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
#   - `arbitrary_size_robustness.sh` used to report "80 / 80 panic-free +
#     aomdec-decodable (48 refused as out-of-envelope)". Refusals are counted
#     as PASSES there, because refusing IS the correct behaviour, and nothing
#     in that line distinguishes "genuinely out of scope" from "nobody has done
#     the work yet".
#
#     AND THE LINE ITSELF WENT STALE, which is the same failure one level up.
#     MEASURED 2026-09-03: that gate is **128 / 128 with ZERO refusals**. Its 48
#     were exactly the bd10 cells at non-64-aligned dims (26 bd10 cells x 2
#     contents, minus the 4 that are 64-aligned = 48), and that refusal was
#     lifted on 2026-08-04 — a month before this comment was read aloud as a
#     current fact. A scoreboard quoted from prose is a scoreboard nobody
#     re-ran.
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
#
# THE THIRD AXIS (added 2026-09-03): DOES C SUPPORT IT?
#
# CAPABILITY-vs-CONTRACT is a claim about what someone typed. It does not say
# what a CAPABILITY refusal is worth working on, and that is decided by a
# question nothing here asked: can C v4.2.0 encode this configuration at all?
# Three answers, three different backlog items:
#
#   accepts     C encodes it, so a BYTE oracle exists and "implement it and
#               prove byte-parity" is a coherent instruction. The only class a
#               ranked backlog should be drawn from.
#   no mono     C has no monochrome mode AT ALL — `verify_settings` rejects any
#               `encoder_color_format` other than EB_YUV420 ("Only support 420
#               now", Globals/enc_settings.c:473) and the word `monochrome`
#               appears nowhere in its App, settings or public headers. Real
#               debt, but byte-parity can NEVER be its evidence; the substitute
#               this repo already uses is the recon oracle plus decodability
#               (tools/regression_spotcheck.sh's `monoReconEq`).
#   rejects     C refuses it too, so there is nothing to implement and never
#               will be — building it would put this port OUTSIDE the envelope
#               it is measured against.
#
# A refusal declares its answer with a trailing `[C: ...]` marker in the message
# itself, for the same reason the CAPABILITY/CONTRACT split lives there: the
# person who writes the refusal is the one who knows, and a fact kept in a
# separate table rots. Missing marker => `?`, and the summary counts those, so
# an unclassified refusal is visible rather than silently assumed workable.
#
# The markers are checked against the real library by
# `tools/c_envelope_probe.sh`, which runs each configuration through the actual
# encoder (with a positive and a negative control, so a broken driver cannot
# report "C rejects everything"). The ranked triage this feeds is
# benchmarks/refused_config_triage_2026-09-03.md.
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

# `UnsupportedConfig(...)` — every string literal inside the BALANCED call, not
# just one that starts it. A refusal whose message comes out of a `match` on the
# error variant (`UnsupportedConfig(match e { A => "...", B => "..." })`) is a
# refusal like any other, and the anchored regex this replaces silently dropped
# BOTH arms the moment one was introduced (2026-09-05: the mfmv/TPL refusal
# vanished from the ledger, which is exactly the quiet accretion this file
# exists to prevent).
STRLIT = re.compile(r'"((?:[^"\\]|\\.)*)"')


def unsupported_messages(text):
    out = set()
    for m in re.finditer(r"UnsupportedConfig\(", text):
        i = m.end()
        depth, j = 1, i
        while j < len(text) and depth:
            if text[j] == "(":
                depth += 1
            elif text[j] == ")":
                depth -= 1
            j += 1
        for s in STRLIT.finditer(text[i : j - 1]):
            out.add(s.group(1))
    return out
# A third construct: `EncodeError::InvalidDimensions { reason: "..." }`. Missing
# it dropped the two real monochrome-geometry refusals from the ledger.
REASON = re.compile(r'\breason:\s*"((?:[^"\\]|\\.)*)"')
# The C-envelope marker, stripped out of the message into its own column.
CMARK = re.compile(r"\[C:\s*([^\]]*)\]")
# Trailing comma and `.to_string()` are both common; requiring a bare
# `)` right after the string silently dropped half the real refusals.
SOMES = re.compile(r'\bSome\(\s*"((?:[^"\\]|\\.)*)"\s*(?:\.to_string\(\))?\s*,?\s*\)')
FNDEF = re.compile(r"\n\s*(?:pub(?:\(crate\))?\s+)?fn\s+([a-z_0-9]+)")

for path in sys.argv[1:]:
    src = open(path).read()
    joined = re.sub(r"\\\s*\n\s*", " ", src)   # join Rust string continuations

    found = unsupported_messages(joined)
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
        # `[C: ...]` — does the oracle support this? Split OFF the message so
        # the table has a column instead of a sentence to read.
        m = CMARK.search(msg)
        cverdict = m.group(1).strip() if m else "?"
        msg = " ".join(CMARK.sub("", msg).split()).rstrip()
        print(f"{kind}\t{path}\t{cverdict}\t{msg}")
PY
}

generate() {
    local rows cap con oracle noora
    # LC_ALL=C: byte collation, not locale collation. Without it macOS and the
    # Linux CI runner order the same rows differently and `--check` fails on a
    # diff that is pure sort order — which is exactly how this gate first went
    # red.
    rows=$(collect | LC_ALL=C sort -u)
    cap=$(printf '%s\n' "$rows" | grep -c '^CAPABILITY' || true)
    con=$(printf '%s\n' "$rows" | grep -c '^CONTRACT' || true)
    # How many CAPABILITY refusals could EVER be closed by a byte gate?
    oracle=$(printf '%s\n' "$rows" | awk -F'\t' '$1=="CAPABILITY" && $3=="accepts"' | wc -l | tr -d ' ')
    noora=$(printf '%s\n' "$rows" | awk -F'\t' '$1=="CAPABILITY" && $3=="?"' | wc -l | tr -d ' ')

    cat <<EOF
<!-- generated by tools/refusal_inventory.sh — do not edit by hand -->

# Configs this encoder refuses

**${cap} CAPABILITY refusals** (unimplemented — this is DEBT) and **${con}
CONTRACT refusals** (caller misuse — permanent and correct). Of the CAPABILITY
refusals, **${oracle}** name a configuration C v4.2.0 actually encodes — the
only ones a byte-parity gate could ever close — and **${noora}** carry no
\`[C: ...]\` marker at all.

Regenerate with \`tools/refusal_inventory.sh\`; \`--check\` is a CI gate.

## Why this file exists

Refusing beats emitting a wrong bitstream — that rule is correct and stays. But
a refusal also makes a gap look handled: \`arbitrary_size_robustness.sh\` counts
refusals as PASSES, and \`coverage_matrix.py\` cannot show a refused config even
as \`--\`, because a refused config produces no cell at all. So the one tool
built to surface gaps is structurally blind to this one.

That is not hypothetical. 10-bit at non-64-aligned dimensions — the actual AVIF
product case — sat behind a refusal while every gate stayed green, and was read
aloud in a status report before anyone acted on it. It was 48 of that gate's
128 cells; it was lifted on 2026-08-04, and the gate now reads **128 / 128 with
zero refusals** (re-measured 2026-09-03). The sentence you are reading said "its
48 refusals" for a month after they were gone, which is the same rot one level
up: **re-run the gate, do not quote this file's prose at it.**

**Read the CAPABILITY list as a backlog, not as a specification.**

## CAPABILITY — not implemented (debt)

\`C?\` is what the ORACLE does with this configuration, declared by the refusal
itself and verified by \`tools/c_envelope_probe.sh\`:
\`accepts\` = C encodes it, so byte-parity can close it;
\`no mono mode\` = C cannot encode monochrome at all, so byte-parity NEVER can
(use the recon oracle, as \`regression_spotcheck.sh\` does);
\`rejects\` = C refuses it too, so there is nothing to build;
\`?\` = nobody has said, which is itself a gap.

| where | C? | refusal |
|---|---|---|
EOF
    printf '%s\n' "$rows" | awk -F'\t' '$1=="CAPABILITY"{printf "| `%s` | %s | %s |\n", $2, $3, $4}'

    cat <<EOF

## CONTRACT — caller misuse (permanent, correct)

| where | refusal |
|---|---|
EOF
    printf '%s\n' "$rows" | awk -F'\t' '$1=="CONTRACT"{printf "| `%s` | %s |\n", $2, $4}'
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
