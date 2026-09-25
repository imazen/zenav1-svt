#!/usr/bin/env bash
# Build the code-review inputs: test-stripped reading bundles, duplication and
# complexity metrics for the Rust port, and the same metrics for the C reference.
#
#   tools/review/run.sh [OUT_DIR]        (default ~/tmp/zenav1-review)
#
# Needs, all installable without root (see the header of each .py for details):
#   ~/.local/share/venvs/review  python venv with tree-sitter, tree-sitter-rust,
#                                tree-sitter-c  (uv venv + uv pip install)
#   rust-code-analysis-cli       github.com/mozilla/rust-code-analysis releases
#   jscpd                        npm install -g jscpd (exact-copy detector)
# It refuses to run with a tool missing rather than producing a partial report.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
rust_root="$(cd "$here/../.." && pwd)"
c_lib="$(cd "$rust_root/../reference/svt-av1/Source/Lib" && pwd)"
out="${1:-$HOME/tmp/zenav1-review}"
py="${REVIEW_PY:-$HOME/.local/share/venvs/review/bin/python}"

for t in "$py" rust-code-analysis-cli jscpd; do
  command -v "$t" >/dev/null || { echo "missing tool: $t" >&2; exit 2; }
done
"$py" -c 'import tree_sitter, tree_sitter_rust, tree_sitter_c' || { echo "venv lacks tree-sitter grammars" >&2; exit 2; }

mkdir -p "$out"
cd "$out"
nice_=(nice -n19 ionice -c3)

echo "== reading bundles (test code stripped; tables elided)"
"${nice_[@]}" "$py" "$here/prep.py" --root "$rust_root" --out "$out/keep"
"${nice_[@]}" "$py" "$here/prep.py" --root "$rust_root" --out "$out/drop" --comments drop

echo "== complexity (rust-code-analysis)"
rm -rf rca-rs rca-c && mkdir -p rca-rs rca-c
"${nice_[@]}" rust-code-analysis-cli -m -O json -o rca-rs -p keep/mirror -j 8 >/dev/null
"${nice_[@]}" rust-code-analysis-cli -m -O json -o rca-c -p "$c_lib/Codec" -p "$c_lib/C_DEFAULT" -p "$c_lib/Globals" -j 8 >/dev/null

echo "== Rust vs C size and complexity"
"${nice_[@]}" "$py" "$here/c_compare.py" --rust-rca rca-rs --rust-prefix keep/mirror/ --rust-src keep/mirror \
  --c-rca rca-c --c-prefix "$c_lib/" --out cmp > cmp.txt
head -12 cmp.txt

echo "== near-duplicates, same normalization on both sides"
"${nice_[@]}" "$py" "$here/clones.py" --src keep/mirror --out clones-rs
"${nice_[@]}" "$py" "$here/clones.py" --lang c --src "$c_lib/Codec" --src "$c_lib/C_DEFAULT" --src "$c_lib/Globals" --out clones-c

echo "== exact copies (jscpd)"
"${nice_[@]}" jscpd --min-tokens 80 --min-lines 8 --format rust --reporters json --output jscpd-rs --silent keep/mirror | tail -1


echo "== functions no product code reaches (transitive, comments + tests stripped)"
"${nice_[@]}" "$py" "$here/deadfns.py" --src drop/mirror --transitive > dead.tsv
echo "outputs in $out: drop/bundle/*.rs (read these), cmp/*.tsv + cmp.txt, clones-{rs,c}/*.tsv, jscpd-rs/, dead.tsv"
