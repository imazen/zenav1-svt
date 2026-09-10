#!/usr/bin/env bash
# fetch_r2_assets.sh <prefix> <dest-dir>
#
# Pull one public codec-corpus R2 prefix into <dest-dir>, verifying every byte
# against the SHA-256 in the prefix's `.list` index.
#
# WHY THIS EXISTS, AND WHAT IT IS *NOT* FOR.
#
# The still corpora this repo's gates use -- gb82-sc, CID22, clic2025 -- are
# PLAIN GIT-TRACKED FILES in `imazen/codec-corpus` (not LFS). CI fetches those
# with a blob-filtered sparse clone (2.2 s / 6.9 MB for gb82-sc), which is
# versioned, reviewable and already correct. Do NOT route them through here:
# mirroring git-versioned content into object storage gives two canonical
# copies of the same bytes and no way to tell which one a run actually used.
#
# This script is for supporting assets that exist NOWHERE in git because they
# do not belong in an image-corpus repo -- video sequences for the inter/video
# gates, generated caches, prebuilt reference artifacts. Those have no home
# today, which is why every inter gate in this repo currently encodes synthetic
# `gradient`/`diag`/`screen`/`uniform` content.
#
# TRANSPORT. `https://codec-corpus.r2.imazen.org` is a public custom domain on
# the `codec-corpus` R2 bucket: objects are readable anonymously, so CI needs
# NO SECRETS. Nothing here signs a request or reads a credential.
#
# FORMAT. The index is the `codec_corpus::r2::ListIndex` document that
# `r2-corpus push` generates, at `<base>/<prefix-without-trailing-slash>.list`:
#
#     { "version": 1, "prefix": "...", "files": { "<rel>": { "size": N,
#       "sha256": "...", "in_bundle": bool } }, "children": ["..."] }
#
# We deliberately implement the pull rather than depending on the crate: a CI
# step that needs `cargo build` before it can fetch its inputs cannot run
# before the toolchain is set up, and `curl` + `python3` are on every runner.
# Bundles (`<prefix>.tar.zst`) are ignored -- per-file fetch is simpler and the
# asset sets here are small. If that stops being true, add bundle support
# rather than silently fetching 500 files one at a time.
#
# FAILURE MODE. Loud, always. A missing `.list`, a short fetch, a hash
# mismatch, or an empty result all exit non-zero. This repo has been bitten
# three times by a gate that "passed" while its inputs were absent
# (see `lib_corpus.sh` for the ledger); a fetch that half-works must not be
# allowed to become the fourth.
set -euo pipefail

BASE="${ZENAV1_R2_BASE:-https://codec-corpus.r2.imazen.org}"

usage() { echo "usage: $0 <prefix/> <dest-dir>" >&2; exit 2; }
[ $# -eq 2 ] || usage
PREFIX="${1%/}"
DEST="$2"

command -v curl >/dev/null || { echo "FETCH FAIL: curl is required" >&2; exit 1; }
command -v python3 >/dev/null || { echo "FETCH FAIL: python3 is required" >&2; exit 1; }

LIST_URL="$BASE/$PREFIX.list"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

if ! curl -fsS --retry 3 --retry-delay 2 --max-time 120 -o "$tmp/index.json" "$LIST_URL"; then
    echo "FETCH FAIL: no .list at $LIST_URL" >&2
    echo "  The prefix does not exist, or was never published with \`r2-corpus push\`." >&2
    exit 1
fi

mkdir -p "$DEST"

# Emit "<rel>\t<size>\t<sha256>" per file, and fail on a schema we do not speak.
python3 - "$tmp/index.json" > "$tmp/files.tsv" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
v = d.get("version")
if v != 1:
    sys.exit(f"FETCH FAIL: .list version {v}, this script speaks 1")
files = d.get("files") or {}
if not files:
    sys.exit("FETCH FAIL: .list names zero files")
for rel, meta in sorted(files.items()):
    if rel.startswith("/") or ".." in rel.split("/"):
        sys.exit(f"FETCH FAIL: unsafe path in .list: {rel!r}")
    print(f"{rel}\t{meta['size']}\t{meta['sha256']}")
PY

want=$(wc -l < "$tmp/files.tsv")
got=0
while IFS=$'\t' read -r rel size sha; do
    out="$DEST/$rel"
    # Already present and correct? Leave it: reruns on a warm workspace are free.
    if [ -f "$out" ] && [ "$(stat -c %s "$out" 2>/dev/null || echo -1)" = "$size" ] \
       && [ "$(sha256sum "$out" | cut -d' ' -f1)" = "$sha" ]; then
        got=$((got + 1)); continue
    fi
    mkdir -p "$(dirname "$out")"
    if ! curl -fsS --retry 3 --retry-delay 2 --max-time 600 -o "$out.part" "$BASE/$PREFIX/$rel"; then
        echo "FETCH FAIL: $PREFIX/$rel did not download" >&2; exit 1
    fi
    actual=$(sha256sum "$out.part" | cut -d' ' -f1)
    if [ "$actual" != "$sha" ]; then
        echo "FETCH FAIL: $PREFIX/$rel sha256 $actual, .list says $sha" >&2; exit 1
    fi
    mv "$out.part" "$out"
    got=$((got + 1))
done < "$tmp/files.tsv"

if [ "$got" -ne "$want" ]; then
    echo "FETCH FAIL: placed $got of $want files from $PREFIX" >&2; exit 1
fi
echo "FETCH OK: $got file(s) from $PREFIX -> $DEST"
