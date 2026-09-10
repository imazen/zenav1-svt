#!/usr/bin/env bash
set -euo pipefail
cd /home/lilith/work/zen/zenav1-svt/rust
python3 /home/lilith/tmp/svt-tracking/scm-probe10.py scm-native10 > /home/lilith/tmp/svt-tracking/scm-native10.log 2>&1
cargo nextest run --workspace --features zenav1-svt/avif-container > /home/lilith/tmp/svt-tracking/controls-nextest.log 2>&1
RS_AOMDEC=/usr/bin/aomdec tools/regression_spotcheck.sh > /home/lilith/tmp/svt-tracking/controls-spotcheck.log 2>&1
cargo test --workspace --features zenav1-svt/avif-container --doc > /home/lilith/tmp/svt-tracking/controls-doctests.log 2>&1
