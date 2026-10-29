# Issue #19 revalidation — q15→q16 quality-step inversion (2026-10-29)

Verdict: **reproduced on current main, and it is upstream-inherent — not a
porting defect.** The port is byte-identical to C mainline-4.2.0 on both QP
cells of the registered witness pair, so the inversion is SVT-AV1's own RD
outcome on this content. The "adjacent distinct pair" the ladder measured is
the quality→QP quantization surfacing one QP step, not a mapping or
calibration bug in zenavif or the dial.

## Builds measured

- `zenav1-svt` `34219bd8e` (main, post-#26/#27 fixes)
- `zenavif` `a7638f6e` (main) + the documented dev-only
  `[patch."https://github.com/imazen/zenav1-svt"]` → local checkout, so the
  consumer path exercised *current* zenav1-svt rather than the pinned
  `0f63ca13` rev
- Probe: `zenavif/examples/issue19_probe.rs` (the exact consumer path:
  `encode_rgb8` + `Av1Backend::Zenav1Svt`, Yuv420, default speed 4 →
  preset 4, `quality_to_qp_gated(q).max(1)`), decoded by `zenavif::decode`
  (rav1d-safe), scored `fast-ssim2` 0.8 + `butteraugli` 0.9.3
- Source: canonical dial corpus
  `/mnt/tower/output/zensim/dial-grid-pixels-2026-07-27/sources/`
  (`090d19695a8b43c2_512sq.png`, the registered witness)

## Witness pair reproduces exactly

| q | qp | bytes | ssim2 | butteraugli max | butteraugli 3-norm |
|---|----|-------|-------|-----------------|---------------------|
| 13 | 55 | 586 | 19.43 | 13.09 | 4.184 |
| 14 | 55 | 586 | 19.43 | 13.09 | 4.184 | (byte-identical to q13)
| **15** | **54** | **641** | **22.71** | 13.38 | **3.882** |
| **16** | **53** | **627** | **22.10** | 15.09 | **4.202** |
| 17 | 53 | 627 | 22.10 | 15.09 | 4.202 | (byte-identical to q16)
| 18 | 52 | 657 | 25.88 | 13.20 | 3.826 |

q16 is strictly dominated: fewer bytes AND worse on all three metrics. Both
metric values match the issue's 2026-09-05 recording to every printed
decimal (ssim2 22.71→22.10; butteraugli-3n 3.882→4.202). Byte sizes moved
with encoder development (621→641, 607→627) — the QP pair and the
inversion direction did not.

## C parity at both QPs

`tools/cellrun.py` still cells (`crop:` the witness image, 512×512, preset
4, bd 8, `SVT_ORACLE=mainline-4.2.0`):

| cell | verdict |
|------|---------|
| qp 53 | IDENTICAL to C |
| qp 54 | IDENTICAL to C |

Port bytes == upstream bytes at both QPs ⇒ identical recon ⇒ identical
distortion ⇒ the inversion is C's own RD decision at qp53 on this content.

## Scope: content-specific, not systematic

Two other canonical sources at the same grid show monotone ladders
(ssim2, butteraugli-3n, and bytes all improve together from q10→q20;
artifacts `out-14ab4af2…`, `out-0e53ea75…`). The inversion is a
content-dependent single-QP-step RD reversal, a known class of upstream
SVT-AV1 behavior — not a floor-wide axis defect.

## Distinct-setting enumeration

`quality_to_qp_static` maps 99 quality steps onto QP 0..63, so adjacent
quality values alias to identical streams (observed: q13=q14, q16=q17,
q18=q19 byte-for-byte). zenavif additionally clamps `.max(1)` (QP 0 /
lossless is unwired on the svt-rs backend). For the dial/ladder: dedup by
encoded hash — as the instrument already does — is the correct
distinct-setting definition; "adjacent q" pairs may be the same setting.

## Artifacts

`/mnt/tower/output/zenav1-svt/issue19-quality-step-2026-10-29/` —
`.avif` per cell (sha256 in `probe.tsv`), `cells.result.tsv`.
Probe: `cargo run --release --features encode-svt-rs --example
issue19_probe -- <png> <out_dir> [q0 q1]` in `~/work/zen/zenavif`.
