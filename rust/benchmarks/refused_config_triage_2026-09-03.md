# Refused configs, triaged against the C oracle — 2026-09-03

`docs/REFUSED-CONFIGS.md` answers "what does this encoder refuse, and did the
message say it was unported?". It does **not** answer the question that decides
what the refusal is worth: **can C v4.2.0 encode this configuration at all?**

This file answers that one, with two independent kinds of evidence per row —
a reading of `reference/svt-av1`'s own source, and a RUN of the real library
through `tools/capture_c_trace` (the public API, not a transcription).

Reproduce the run half with:

```
tools/c_envelope_probe.sh benchmarks/c_envelope_2026-09-03.tsv
```

That probe's first two rows are CONTROLS and it exits non-zero if they come out
wrong: `baseline` must be accepted and `bitdepth-12` must be rejected. Without
them a broken driver reports "C rejects everything", which reads exactly like a
finding (`docs/WORKING-ON-THIS.md` §5).

## The three C verdicts, and what each one means for the backlog

| verdict | meaning | can byte-parity ever be the evidence? |
|---|---|---|
| **C-REJECTS** | C's `svt_av1_verify_settings` refuses it too | no — and there is nothing to implement |
| **C-ABSENT** | C has no such mode at all | **no, not even in principle** |
| **C-ACCEPTS** | C encodes it; an oracle exists | yes — this is the only implementable class |

**C-ABSENT is the whole monochrome family, and it is a bigger deal than it
looks.** `verify_settings` rejects any `encoder_color_format` other than
`EB_YUV420` ("Only support 420 now", `Globals/enc_settings.c:473`) and the
string `monochrome` appears nowhere in C's App, its `enc_settings.c` or its
public headers. So every mono refusal is real debt whose evidence can never be
byte-parity. This repo already has the substitute and uses it:
`tools/regression_spotcheck.sh`'s `monoReconEq` asserts the encoder's FINAL
reconstruction equals `aomdec`'s output, plus decodability. Ranking a mono item
by "how close is it to byte-parity" is a category error.

## Ranked triage

Value is judged for this repo's stated product: **AVIF / web stills**. An item
that only matters for video is ranked low here even when it is easy.

| # | refused config | C? | evidence | product value | verdict |
|---|---|---|---|---|---|
| 1 | `use_ref_frame_mvs` at `mfmv_level >= 2` | **C-ACCEPTS** | `mfmv_controls`, `enc_mode_config.c:8853`; run `inter-*` rows | high — it is 12 of the 64 inter-completion cells, i.e. a whole *resolution* axis (568 px and up at preset <= 10) | **IMPLEMENTED** (below) |
| 2 | global motion (no refusal existed) | **C-ACCEPTS** | `svt_aom_derive_gm_level`, `enc_mode_config.c:194`; `inter-preset4` accepted | high — it was SILENT, not refused | **REFUSAL ADDED** (below) |
| 3 | monochrome at non-8-aligned dims | **C-ABSENT** | no mono mode in C | high — AVIF alpha is a mono plane at the image's own (arbitrary) size | **IMPLEMENTED** (below) |
| 4 | QP 0 (coded-lossless) at 10-bit | C-ACCEPTS | `qp0-10bit` = 1977 B | medium — lossless 10-bit AVIF | leave refused; needs a WHT/TX_4X4 arm in a bd10 level producer |
| 5 | QP 0 with screen-content tools | C-ACCEPTS | `qp0-screen-content` = 509 B | medium — lossless AVIF of screenshots is a real PNG-replacement case | leave refused; the palette/IntraBC × lossless product is unverified, not unimplemented |
| 6 | QP 0 on the monochrome path | **C-ABSENT** | no mono mode in C | medium — lossless AVIF alpha | leave refused; needs a mono WHT/TX_4X4 arm AND a non-byte oracle |
| 7 | 10-bit monochrome below preset 9 | **C-ABSENT** | no mono mode in C | medium — 10-bit AVIF alpha | leave refused; needs the bd10 funnel to run without chroma |
| 8 | GOP shapes outside the 4 translated `generate_rps_info` branches | C-ACCEPTS | `inter-randomaccess` = 138 B | low for stills | leave refused; random-access pyramids are a video feature |
| 9 | inter frames on the public API (envelope 89/96) | C-ACCEPTS | the inter campaign | low for stills, high for animation | leave refused — the envelope, not the machinery, is the blocker |
| 10 | superres at 10-bit | C-ACCEPTS | `superres-10bit` 53 B → 42 B with denom 16, 43 B at denom 9 | low — superres is a streaming feature | leave refused; the u16 source downscale is genuinely unported |
| 11 | QP 0 with superres | C-ACCEPTS | `qp0-superres` = 651 B | very low — "lossless, but downscale it first" | leave refused |
| 12 | QP 0 in HDR-fork mode | C-ACCEPTS (fork) | fork knobs | very low | leave refused; and C's own variance boost is internally inconsistent there (SUSPECTED-C-BUGS #1) |
| 13 | QP 0 on inter frames | C-ACCEPTS | `qp0-inter` = 929 B | very low for stills | leave refused |
| 14 | bit depth other than 8 or 10 | **C-REJECTS** | `Globals/enc_settings.c:460`, probe row `bitdepth-12` | n/a | **permanent — reclassified from CAPABILITY to CONTRACT** (below) |

## What was wrong in the ledger, and is now fixed

1. **A `C-REJECTS` constraint was filed as DEBT.** "bit depth must be 8 or 10"
   sat in the CAPABILITY table — the table whose header says "this is DEBT" —
   because `refusal_inventory.sh` classifies on the words `no 12-bit kernels`.
   It is not debt: C rejects the config at `svt_av1_verify_settings`, so there
   is no oracle and implementing it would put the port outside C's envelope.
   The identical constraint in `svtav1/src/avif.rs` was already filed as
   CONTRACT, so the same rule appeared in both halves of the ledger at once.

2. **A refusal that could not fire.** `bit_depth_config_error`'s third arm
   ("this 10-bit configuration has no bd10 stage") is unreachable:
   `bd10_levels_native` returns `preset >= 9 || preset <= 8` for 4:2:0, i.e.
   always true, and the mono case is caught by the arm above it. It was
   counted as one of the CAPABILITY refusals, inflating the debt list by an
   item nobody can hit.

3. **A refusal that was only a comment.** `inter_syntax_state` said
   "`inter_hdr_arm::inter_signal` refuses a non-identity model, so every
   reference is IDENTITY here by the same rule the header is written under —
   not by assumption". `InterHdrError::GlobalMotionNotImplemented` was never
   constructed anywhere in the crate, so it *was* the assumption. C's
   `svt_aom_derive_gm_level` gives an inter frame at preset <= 4 a non-zero
   `gm_level`, and the whole inter campaign measures preset >= 6, where C's own
   level is 0 — so nothing could ever have caught it. Now a real refusal.

4. **A refusal that named the wrong precondition** — item 1 of the ranked
   table, below.

## Item 1 in detail: `mfmv_level >= 2` was never TPL-dependent HERE

The refusal read:

> use_ref_frame_mvs at mfmv_level >= 2 needs the TPL r0 and the references' own
> is_mfmv_used

True of C in general. Not true of any configuration this port can encode:

* C `mfmv_controls` (`enc_mode_config.c:8853`) sets
  `r0_th = ppcs->scs->tpl ? 0.15/0.13/0.10 : 0` for levels 2/3/4 and then
  guards the entire `r0` + `is_mfmv_used` block behind `if (r0_th)`. With TPL
  off the bit is a closed **0** and neither input is read.
* C `get_tpl` (`Globals/enc_handle.c:3657`) returns 0 for all-intra, for
  `aq_mode == 0`, for `LOW_DELAY`, for a fixed superres mode and for resize.
  This port **refuses `aq_mode != 0` outright** (`knob_config_error`, issue #9
  item 8) and builds every inter picture as `LOW_DELAY`. So `scs->tpl` is 0 in
  every encodable configuration, by two independent clauses.
* The port had already ported `mfmv_controls` — in
  `port_enc_mode_config::tail`, tier-1 C-parity-tested through the exported
  `svt_aom_sig_deriv_mode_decision_config_default` shim
  (`tests/c_parity_sig_deriv_md_config.rs:315`), with a doc comment stating the
  `r0_th` argument above **in those words**. `inter_hdr_arm` re-derived the
  rule inline and refused instead of calling it. This is the exact failure
  `docs/WORKING-ON-THIS.md` §4 names: "TWO transcriptions of the same C
  function will diverge — grep before you write the second."

There was a **third** copy: `inter_mvp_env` spelled the value as
`sigs.mfmv_level == 1`. That agreed with C only because every level above 1 was
refused before the tile could reach it. All three now call the one ported
function.

**Why the refusal was resolution-shaped.** `mfmv_level` is 2 when
`enc_mode <= M8` and `input_resolution > 360p` (`md_config.rs:478`, C
`sig_deriv_mode_decision_config_default`). That is why the completion scan's
twelve refusals were all at 568/576/1024/2048 square and none at 512 or below,
and why preset 13 was unaffected (`mfmv_enabled = 0` above M10 → level 0).

## Results

| what | before | after |
|---|---|---|
| `inter_completion_scan.sh` (64 cells) | 52 OK / 12 REFUSED / 0 CRASH | **64 OK / 0 REFUSED / 0 CRASH** |
| …byte-identical on both frames | 5 of 64 | **8 of 64** (2 attributable here) |
| `inter_byte_gate.sh` | 89 PASS + 6 open | **91 PASS** + 6 open (first cells above 360p) |
| `regression_spotcheck.sh` | 83 / 83 | **93 / 93** |
| `docs/REFUSED-CONFIGS.md` | 17 CAPABILITY / 28 CONTRACT, no oracle axis | 17 CAPABILITY / 29 CONTRACT, **11 with a byte oracle, 3 unclassified** |
| `decode_conformance.sh avif` | 240 mono streams, 0 refused | 224 encode + **16 typed refusals** (the padded-size mismatch, now named) |

Every one of the three lifts was mutation-proved — the fix reverted, the new
cells observed to FAIL, the fix restored, the cells observed to pass:

* width-1 sequence header: 4 cells fail, **at identical byte counts to C**
  (24 B vs 24 B), which is why they are byte cells and not size cells. The 2x2
  control passes in both states.
* monochrome arbitrary dims: all 5 cells fail as `rs-err`. This mutation also
  caught a defect in my OWN cells — two of them were missing `SVTAV1_MONO=1`
  and so exercised the 4:2:0 path; they passed under the mutation and were
  fixed. A cell added without a mutation check is a cell that may be testing
  nothing.
* mfmv: `identity_diff_inter.sh 576 576 32 8` returns exit 3 (REFUSED) with the
  mutation and "frame 0: IDENTICAL / frame 1: IDENTICAL" without it.

## What was NOT done, and why

* **QP 0 at 10-bit, with screen content, with superres, on inter frames** — C
  accepts all four (measured), so each is a legitimate backlog item with an
  oracle. None is implemented here: the first needs a WHT/TX_4X4 arm in a bd10
  level producer, and the rest are ranked below the three that shipped.
* **Superres at 10-bit** — C accepts it (53 B → 42 B at denom 16, 43 B at denom
  9, so superres is demonstrably live at bd10 and not a no-op). The u16 source
  downscale is genuinely unported. Low product value for still AVIF.
* **GOP shapes beyond the four translated `generate_rps_info` branches, and
  inter on the public API** — C accepts both; both are video features.
* **Every monochrome refusal that is not the arbitrary-dims one** (mono at
  preset < 6 partial SBs, 10-bit mono below preset 9, mono lossless) — real
  debt, no possible byte oracle, and each needs a different missing piece.
* **QP 0 in HDR-fork mode** — the one row this triage could not answer. It
  needs the HDR-fork oracle build, which was out of scope
  (`SVT_CREF_SKIP_HDR=1`). It is the only `?` in the ledger's `C?` column that
  is a gap in the TRIAGE rather than in the port.

## Corrections to the brief that commissioned this

1. **The bit-depth refusal did NOT frame an upstream constraint as port debt in
   the way described.** Its text already led with "C v4.2.0 rejects every other
   depth at encoder init (svt_av1_verify_settings,
   Globals/enc_settings.c:460)". The real defect was subtler and is fixed here:
   the trailing "and this port has no 12-bit kernels" matched
   `refusal_inventory.sh`'s CAPABILITY keyword list, so a permanent upstream
   constraint was filed in the table whose header reads "this is DEBT" — while
   the identical rule in `svtav1/src/avif.rs` was filed as CONTRACT. Same rule,
   both halves of the ledger.
2. **The brief says 18 CAPABILITY refusals; the file said 17.** It is 17 now
   too — one was added (global motion) and one removed (mono arbitrary dims).
3. **"10-bit at non-64-aligned dimensions" is already fixed**, as the brief
   suspected: `bd10_levels_native` has carried "NO GEOMETRY TERM (2026-08-04)"
   since then and `tools/bd10_partial_sb_gate.sh` covers 157 cells. The
   `REFUSED-CONFIGS.md` preamble still cites it, correctly, in the PAST tense as
   the incident that motivated the file.
4. **`InterHdrError::GlobalMotionNotImplemented` was dead, not merely
   unreached** — never constructed anywhere in the crate — and the comment that
   relied on it claimed the opposite in as many words.
