# Rescued investigation tooling (`svt-tracking`)

**Status: PRESERVED, NOT RUNNABLE. Read this before trying to use anything here.**

These 25 files were recovered on 2026-09-10 from `~/tmp/svt-tracking` on the
`i265` box, where they were the *only* copy. Nothing here was committed
anywhere, and the directory was 14 GB of scratch on a single machine. The repo's
own history says why that matters: `CLAUDE.md` records a 2026-07-15 `/tmp` wipe
that destroyed a complete zenwebp parity harness including its pre-fix baseline,
which made a before/after comparison impossible. The `imazen26-cache/K300`
corpus was lost the same way, to a torn-down rented box.

## They do not run as-is

**24 of the 25 hardcode absolute scratch paths** — `/home/lilith/tmp/svt-tracking/...`,
`/home/lilith/tmp/av1-imazen26-research-baseline-2026-09-08`, and similar. This
is precisely the failure class that made `decode_diff` (hardcoded `/root/aom-rs`)
and `ifs_join_gate` unbuildable on every host but one, and that
`tools/lib_corpus.sh` exists to document. So:

- Do **not** add any of these to CI or to a gate as they stand.
- Do **not** report a result from one without first repointing its paths and
  saying which host it ran on.
- Treat them as **method and evidence** — what a past investigation actually
  did — rather than as tools. That is still worth far more than deletion, which
  was the alternative.

Making one portable means the `lib_corpus.sh` treatment: probe candidate roots,
take an env override, and fail loudly rather than silently. Do that per script,
when a script is actually needed, and commit the repointed version.

## What is here

| cluster | files | what it was for |
|---|---|---|
| native10 | `native10-drill.py`, `native10-recon.py`, `native10-seeds.py`, `native10-sb3.py` | The four open native10 parity cells — the repo's #1 open parity item. `IDENTITY-STATUS.md` records p1q10's first divergence as block mi(32,36), a partition-size flip; this is the tooling that localisation came from. |
| pristine oracle | `capture-mainline.c`, `decode-rust-mainline.py`, `generate-reference-fixtures.py` | The **default `SvtParity` target**. `capture-mainline.c` is the pristine-v4.2.0 capture driver; the committed `tools/pristine_reference_compare.py` compares against what it produces, and has no CI gate. |
| chroma | `chroma-native-boundary.sh`, `chroma-native-remainder.py`, `replay.py` | Chroma-boundary and parity-replay investigation. |
| research/expand | `expand_compare.py`, `expand_c.py`, `decode-research*.py`, `summarize-imazen26-research.py` | The preset −1 research-mode comparison work and the imazen26 research summary. |
| probes | `tune-probe*.py`, `tune-mutation.py`, `scm-probe*.py`, `validate-*.sh`, `decode-controls.py`, `archive-intra-edge.py` | Tune/SCM ablation probes and their controls. |

## The pristine-v4.2.0 build recipe

Recovered from that tree's `configure.log`, which is the part that was genuinely
at risk — the 2.0 GB build itself is reproducible from the pinned submodule:

```
cmake -S <svt-av1 v4.2.0 source> -B <build> \
  -DCMAKE_BUILD_TYPE=Release -DBUILD_SHARED_LIBS=OFF -DSVT_AV1_LTO=OFF
```
Built with GCC 15.2.0 + nasm on x86-64.

## Not rescued, deliberately

`pristine-v4.2.0/hybrid-ablation/{full_loop,product_coding_loop}.hybrid.c`
(728 KB together) are **byte-identical to the current submodule sources** —
`diff -u` against `reference/svt-av1` is 0 bytes for both. They were plain
copies carrying no ablation, so they encode nothing and were dropped. The rest
of the 14 GB is C build trees, reproducible from the recipe above.
