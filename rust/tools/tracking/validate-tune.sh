#!/usr/bin/env bash
set -euo pipefail
cd /home/lilith/work/zen/zenav1-svt/rust
cargo nextest run --workspace --features zenav1-svt/avif-container > /home/lilith/tmp/svt-tracking/tune-nextest.log 2>&1
RS_AOMDEC=/usr/bin/aomdec tools/regression_spotcheck.sh > /home/lilith/tmp/svt-tracking/tune-spotcheck.log 2>&1
cargo clippy --workspace --all-targets --features zenav1-svt/avif-container -- -D warnings > /home/lilith/tmp/svt-tracking/tune-clippy.log 2>&1
