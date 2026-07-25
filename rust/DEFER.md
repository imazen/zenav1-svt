# DEFER.md — SUPERSEDED

Every item this file deferred (the fallible `encode_frame_impl`, the
`try_vec!` / `try_with_capacity!` fallible-allocation sweep, and the in-loop
cooperative stop checks) LANDED in `78c99d767`.

Verify rather than trust this note: `encode_frame_impl` returns
`crate::EncodeResult<Vec<u8>>`, `grep -rn 'try_vec!\|try_with_capacity!'
crates/svtav1-encoder/src` shows the allocation sites, and the stop checks sit
inside the superblock-row loops in `pipeline.rs`.

Kept as a stub so links to it do not 404. Do not add new deferrals here — use
`rust/docs/*-port-map.md` (per-feature plans) or the queue in `rust/CLAUDE.md`.
