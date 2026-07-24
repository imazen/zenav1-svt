# Native 10-bit input (real u16 source) — port map (issue #6)

**Status: SCOPED, chunk-1 designed. Not yet landed.** The port already processes at
**true 10-bit internally** (the u16 MD/recon path exists — `leaf_funnel.rs:2130`, `pipeline.rs:4862+`),
but **source enters as `u8` and is widened `<< 2` at ~43 sites**, so the low 2 bits of a real
10-bit source are never seen. This map threads *real* u16 source into the existing u16 path.

Contract consumer: **zenavif#33** (lift the SvtRs `bit_depth Ten` rejection once chunks 1–2 land).

## The u8→u16 boundary (where real u16 must replace the widening)

| site | file | what it widens today |
|---|---|---|
| MD funnel RD source | `leaf_funnel.rs:3496-3521` (`Bd10Rd` build) | `y_src10[i] = u16::from(y_src[..]) << shift` (+ u/v) |
| deblock (DLF) search | `pipeline.rs:~1955` | `widen(&encode_input)`, `widen(su)`, `widen(sv)` |
| CDEF search | `pipeline.rs` `cdef_search_still_hbd` caller | source widen for distortion |
| LR (Wiener) search | `pipeline.rs` restoration caller | source widen for distortion |
| recon distortion / SSE | funnel + `pipeline.rs` `svt_full_distortion_kernel16_bits` sites | source vs recon |
| 20 more in `leaf_funnel`, 23 in `pipeline` | grep `<< sh`, `<< 2`, `widen` | — |

## Chunk 1 — public entry point + funnel real-u16 (SAFE, equivalence-gated)

Threads real u16 into the **MD funnel RD only** (the biggest consumer). Post-filters still
widen u8 → chunk-1 real-10-bit source gets real-u16 MD but 8-bit-precision post-filter
distortion (documented band-limit). The `u8` path is byte-unchanged.

1. **Pipeline field** — `pipeline.rs`: add `hbd_source: Option<(Vec<u16>, Vec<u16>, Vec<u16>)>`
   (whole-frame, in the SAME strided layout as `encode_input`).
2. **Entry points** — `pipeline.rs` next to `try_encode_frame_420`:
   - `pub fn try_encode_frame_420_hbd(&mut self, y: &[u16], u: &[u16], v: &[u16], y_stride) -> EncodeResult<Vec<u8>>`
   - `pub fn try_encode_frame_hbd(&mut self, y: &[u16], y_stride) -> EncodeResult<Vec<u8>>` (mono)
   - Store the u16 planes in `hbd_source`; set `encode_input`/chroma = `(v >> shift) as u8`
     (the truncated high bits, for the not-yet-threaded sites); call the existing core.
3. **`FunnelFrame`** — `leaf_funnel.rs:376`: add `pub hbd_y: Option<Vec<u16>>`,
   `pub hbd_u: Option<Vec<u16>>`, `pub hbd_v: Option<Vec<u16>>` (whole-frame).
4. **Populate** — `pipeline.rs:5689` (the `FunnelFrame { .. }` literal): set the three from
   `self.hbd_source` (clone the frame-strided planes; `None` on every u8 path).
5. **Funnel read** — `leaf_funnel.rs:3496-3521`: when `frame.hbd_y.is_some()`, fill
   `y_src10[r*w+c] = frame.hbd_y[y_src_off + r*y_src_stride + c]` (real u16), else the
   existing `u16::from(y_src[..]) << shift`. Same for `u_src10`/`v_src10` via `frame.hbd_u/v`
   at `c_off`.
6. **Gate** (anti-vacuous) — a new `tools`/test:
   - **Equivalence**: `encode_frame_420_hbd(widen(y8) …)` == `encode_frame_420(y8 …)` for
     `widen = |s| (s as u16) << 2`. Proves the threading is correct (identical when the u16
     is just widened u8). MUST pass.
   - **Witness**: a real-10-bit source (low 2 bits set) makes the funnel RD *differ* from the
     truncated-u8 encode (proves the low bits now reach MD). MUST differ.

## Chunk 2 — full precision (real 10-bit byte-parity vs C)

Thread `hbd_source` into the remaining sites (deblock/CDEF/LR distortion, recon SSE — the
`pipeline.rs` widen sites) so the low 2 bits survive end-to-end. Then:

- **C oracle**: `capture_c_trace.c` currently writes the C input as `(u8 << shift)`
  (`identity_run.rs:288`). Add a real-u16 source mode (write the actual 10-bit samples) so the
  C reference sees the SAME low bits.
- **Byte-parity gate**: real-10-bit source → port `encode_frame_420_hbd` == C at bd10, across
  the bd10 matrix + a photographic 10-bit corpus (native 10-bit PNG/y4m, not 8-bit widened).

## Chunk 3 — HDR static metadata (separate; issue #7, zenavif#33)

Mastering-display (`mdcv`) + content-light (`clli`). Decide port-OBU vs AVIF-box with the
zenavif muxer. CICP (primaries/transfer/matrix) already flows via `with_color_space`.

## Invariants

- `#![forbid(unsafe_code)]`. The `u8` path stays byte-identical (hbd is additive `Option`,
  `None` everywhere today → every existing gate untouched). Chunk 1's equivalence gate is the
  proof. Do NOT truncate the low 2 bits anywhere on the hbd path past chunk 2.
