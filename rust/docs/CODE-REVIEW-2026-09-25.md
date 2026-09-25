> **SNAPSHOT — a whole-codebase code review, dated in its own filename.** It
> describes the tree at `388e213d` (2026-09-25), not the current state. Current
> support is in [README.md](../../README.md), current parity in
> [IDENTITY-STATUS.md](IDENTITY-STATUS.md), and current refusals in
> [REFUSED-CONFIGS.md](REFUSED-CONFIGS.md). Line numbers are orientation only;
> re-derive every item against source before acting on it, and strike it here
> when it is fixed. The per-file working ledger behind this summary, with every
> file:line reference, is [CODE-REVIEW-2026-09-25-ledger.md](CODE-REVIEW-2026-09-25-ledger.md).

# Code review — cleanup, structure, testability, performance

## How it was done, and what it did not cover

The product source was read after stripping tests and comments
(`tools/review/prep.py`), with size, complexity and clone metrics from
`tools/review/run.sh`, a transitive dead-function scan (`deadfns.py`) and an
`incant!` tier scan (`incant_tiers.py`). The encoder crate was read in full,
except `port_picstruct` and bundles 14–16, which got targeted scans (env reads,
panics, allocations, duplicate names). Dispatch-less and allocating DSP files
were found by scan. DSP bundles 03/04 and part of 00 were read by reviewer
agents whose claims were spot-checked (one was corrected). DSP bundles 01, 02,
05 and 06 were **not** read line by line.

Three claims were checked by running code on `i265`:

| check | result |
|---|---|
| `identity_run gradient 128 128 40 5`, 4 frames, `SVT_HDR_MODE=1 SVT_FORK_ALT_SSIM_TUNING=1` | **release panic** `leaf_funnel/mds3.rs:3589` "the tune-SSIM parallel full cost has no INTER skip arm"; same cell without alt-SSIM: rc 0; p3, p8 and q20 cells: rc 0 |
| `cargo check -p zenav1-svt-encoder --no-default-features --lib` | **120 errors** (22 `eprintln!`, 38 `std::` paths, 46 unqualified `Vec`/`vec!`, `thread_local!`) |
| 10-bit RDOQ gating asymmetry, C differential: 5 CID22-512 images × presets 6/7/8 × qp {16,20,24,28,36,44,48} | 105/105 byte-identical — the code asymmetry is real but not reachable on these still cells |

## Size against C

Rust product code is 180.9k physical lines (tests and comments excluded);
the C scalar library (`Codec`, `C_DEFAULT`, `Globals`) is 146.7k. The Rust
figure also contains the port's SIMD. C's SIMD is separate and larger than its
scalar code: ~137k lines of x86 (`ASM_SSE2`…`ASM_AVX512`) and ~67k of Arm
(`ASM_NEON*`, `ASM_SVE*`).

Rust lines attributed to C files by the C citations in the Rust source
(`cmp/c_modules.tsv`; 156.6k of the 180.9k attribute), rolled up by subsystem:

| subsystem | C kloc | Rust kloc | Rust/C |
|---|---:|---:|---:|
| Mode decision (MD loop, candidates, RD cost) | 20.5 | 35.6 | 1.74 |
| Picture decision / GOP / TF / TPL / RC | 18.4 | 21.2 | 1.15 |
| Inter prediction (MC, warp, OBMC, compound) | 6.9 | 13.0 | 1.90 |
| Speed/config ladders (`enc_mode_config.c`) | 8.5 | 12.9 | 1.52 |
| Loop filters (DLF, CDEF, LR) | 7.0 | 12.9 | 1.85 |
| Entropy coding + CDF tables | 6.5 | 12.5 | 1.92 |
| Motion estimation / MV prediction | 6.9 | 10.7 | 1.55 |
| Transform + quant | 10.1 | 9.6 | 0.95 |
| Intra / palette / IntraBC | 4.2 | 6.3 | 1.51 |
| Partition / enc-dec driver | 2.7 | 4.6 | 1.74 |
| Distortion / variance / SAD / pic ops | 2.3 | 3.8 | 1.68 |
| other (mostly C threading, queues, result objects) | 23.8 | 13.5 | 0.57 |

Largest excess by C file: `product_coding_loop.c` +4.8k, `enc_mode_config.c`
+4.4k, `cabac_context_model.c` +3.6k (tables), `mode_decision.c` +3.5k,
`rd_cost.c` ×2.9, `deblocking_common.c` ×3.9, `md_config_process.c` ×2.7,
`rc_process.c` ×2.9, `cdef.c` ×2.8. The excess is mostly the duplication and
8/10-bit twinning itemised below, not extra function: name-matched functions
are ~1:1 (40.0k Rust vs 39.7k C lines). Complexity is where it diverges: the
largest Rust functions are `encode_frame_impl` (4,562 lines, cognitive 976),
`encode_tile_rows` (1,900 / 807), `eval_candidate` (2,483 / 464) and
`inject_candidates` (1,600 / 537); C's worst is cognitive 317.

## Correctness and trust defects

1. **Release panic on a settable config** — the tune-SSIM row above. Refuse
   `hdr.is_fork() && alt_ssim_tuning` on inter frames at config time, or port
   the missing inter-skip arm.
2. **`no_std` does not compile** (120 errors), no CI job checks it, and
   `just test-minimal` (`--workspace --no-default-features`) hides the break
   through feature unification. Once it compiles, `intrabc::libm_exp`
   (`unimplemented!()`, reached at IBC levels 3–5, qp ≥ 46) becomes a runtime
   panic. Either drop the claim or add a per-crate CI check and fix it.
3. **Preset −1 silently skips global motion C performs.** `pipeline.rs:~7261`
   hard-codes `gm_pp_detected = false` under a comment saying `pp_enabled` is
   false at every expressible level; `set_gm_controls(2)` (reached at
   `enc_mode <= MR`) sets it true. If enabled, `compute_global_motion` would
   reach `panic!("unreachable correspondence_method")` on CORNERS.
4. **Stale gates and comments that point the wrong way.** `pipeline.rs:~6105`
   hard-disables a homegrown temporal filter with a comment claiming C's
   driver is unported — `port_tf_driver.rs` is that driver and is live.
   `pipeline.rs:~16098` describes a "live" decoder-desync inter writer that
   was replaced. `port_entropy_inter/primitives.rs` `allow_palette` claims to
   call through to `entropy::context::allow_palette` and does not.
5. **Silent fallbacks**: TPL pushes an empty reference picture in release
   (`run_tpl_stage`, `debug_assert!(false)` then continue); `port_tpl.rs:1957`
   discards a recon `Result` with `let _ =`; `WnFilterCtrls::from` drops
   `use_prev_frame_coeffs` behind a `debug_assert!`.
6. **The 10-bit RDOQ path skips the gates the 8-bit path applies**
   (`dct_dct_only`, `skip_uv`, `eob_th`, `eob_fast_th`; `txt.rs:391` →
   `tx_unit_hbd_screened`). Not reachable on the still cells above; the
   exposure is 10-bit video at presets where light-PD1 raises RDOQ to level
   4/5, which is decoder-verified only. Pass `RdoqCtrls` instead of a bool.

## Structural improvements — code

**S1. Retire or quarantine the second encoder.** A pre-port, non-parity encoder
still lives beside the C port: `encode_loop.rs`, `motion_est.rs` (a
"hierarchical ME" that is not hierarchical, bilinear subpel), the legacy
`partition_search_with_config` (~800 lines, nine copy-pasted partition
blocks, live for mono and 4:4:4 at presets ≤ 5), `rate_control::tpl_sb_qp_offsets`,
`temporal_filter::temporal_filter`, `mode_decision`, `perceptual`,
`multipass`, `lf_levels`, `tile`. It is interleaved with the port, so every
reader has to work out which half a function belongs to. Route mono and 4:4:4
through the funnel and delete it, or move it under one `legacy/` module with
its own tests and no re-exports.

**S2. Split `pipeline.rs` (23.6k lines) along the frame's actual stages.**
`EncodePipeline` holds 70 fields of config, state and diagnostic taps. Stages
with explicit input/output structs — `config`, `ra_queue`, `tf_stage`,
`tpl_stage`, `frame_plan` (signals), `tile_md`, `recon`/`bd10`, `loop_filters`,
`entropy`, `ref_store` — are what make stage-level tests possible (T4).
Inside the tile loop, build `Pd0SbParams` and `FunnelCtx` once per superblock:
today the 25-field `FunnelCtx` literal is written three times and
`pd0_pick_sb_partition_*_eval` is called at six sites with 20–26 positional
arguments.

**S3. Derive signals once, through the ported orchestrators.** The C
`sig_deriv_*` functions in `port_enc_mode_config` (`sig_deriv_enc_dec_common`,
`_enc_dec_default`, `_multi_processes_default`, `_pre_analysis_pcs`) are
ported, tested, and dead: `pipeline.rs` calls the leaf setters directly and
re-assembles the signals by hand, so each signal has two derivations and the
tested one is not the one that runs. Produce one `PictureSignals`/`SbSignals`
through the orchestrators. The same pattern produced same-name type
collisions (`SgFilterCtrls` ×2, one live and one dead).

**S4. Make bit depth a type parameter, not a twin.** The 10-bit path is a
second copy at every layer: `tx_unit_inner`/`tx_unit_hbd_screened`, `hbd.rs`
beside `intra_pred.rs`, TF, CDEF, OBMC blends, warp, convolve, and a separate
leaf re-encode pass (`pipeline/bd10_reencode.rs`). MDS1 and `txt_search` run
every 10-bit candidate at 8 bits too, only to keep an 8-bit twin. A
`Pixel: u8 | u16` sample trait (several TF accumulators already use one) would
collapse the twins; a `BitDepth { B8, B10 }` enum validated once removes the
22 `panic!("unsupported bit depth")` arms.

**S5. One home for AV1 vocabulary and small helpers** (`svtav1-types`).
Definitions today: `round_power_of_two` ×20, `rdcost` ×13, `divide_and_round` ×7,
`MI_SIZE` ×9, `LAST_FRAME` ×9, `INTRA_FRAME` ×8, `REF_FRAMES` ×7; mode
constants as raw `u8` beside `PredictionMode`; C-index transform numbering
beside the `TxType`/`TxSize` enums. Same name, different type: `SbVariance`
×3, `MotionMode` ×3, `CompoundType` ×3, `TransformationType` ×2 plus `u8`
constants, `PartitionType`, `FrameType`, `FilmGrainParams`, `SgrprojInfo`, the
ME search-area and ME control structs (copied field by field in
`apply_me_signals` and `apply_me_tf_signals`).

**S6. One scratch strategy.** Buffers come from three places: `vecpool`,
thread-local `TxScratch`, and ad-hoc `vec!` in hot loops. Converge on `&mut`
scratch owned by the tile context and passed down. The worst per-call
allocations are the Wiener `H` matrix per restoration unit (C uses the
stack), 10-bit OBMC blends (three `Vec`s per neighbour, while 8-bit allocates
none), `enc_make_inter_predictor` (16K scratch per masked/warp block), SGR
buffers, `bd10_reencode` leaves, and the TPL recon helpers.

**S7. Diagnostics out of the library.** There are 87 `SVTAV1_*` environment
variables: 44 are read directly rather than through `dbgenv`'s cache, and one
(`SVTAV1_BLKLAMBDA`) is read per leaf. There are 166 `eprintln!` sites, and
twelve `std::fs` writers, some of which `.expect()` and panic on I/O errors.
`SVTAV1_SC_TOOLS` changes coding decisions. `HdrForkConfig::from_env` is a
library function that reads 28 variables and panics on unparsable values;
only the example harness calls it. Replace this with an
observer/trace trait carried in config behind a `trace` feature, and turn
behaviour knobs into config fields.

**S8. Uniform SIMD dispatch, and a gate on it.**
- 27 of 98 `incant!` sites lack a tier (`incant_tiers.py --missing`); 24 of
  them lack x86, including every 8-bit and 10-bit intra predictor and CFL.
- `port_convolve.rs` dispatches through 11 hand-rolled `summon()` cascades and
  has no NEON path.
- `port_convolve_hbd`, `port_convolve_scale`, `port_diffwtd_d16`,
  `port_enc_make_pred`, `pic_operators` and `port_temporal_filtering` have no
  dispatch at all.
- Full-pel ME lacks C's multi-offset (`mpsadbw`) SAD.
- The `fwd_txfm2d_*_dct_dct` tier arms are identical calls to a non-inline
  function, so that dispatch is pure overhead.

Make `incant_tiers.py --missing` a CI gate with an allowlist, so a new
scalar-only site is a visible decision rather than a default.

**S9. A narrow public API.** The facade re-exports whole internal crates
(`pub use svtav1_encoder as encoder`, `dsp`, `types`, `entropy`, `tables`),
which makes all 107 encoder modules public API. The root `Encoder`/`Frame`/
`Packet` is a non-functional scaffold that also accepts 12-bit. The setters
silently clamp (`with_quality`, `with_speed`, `with_variance_boost`,
`with_crf`), which makes `EncodeError::InvalidQuality` unreachable.
`EncodedAvif` holds raw AV1 OBUs, not an AVIF container. The thread count
silently changes the tile layout, and with it the bitstream. `RgbaFrame` has
no stride. Much of the module surface is public only because tests need it —
see T2.

## Structural improvements — testing

**T1. Every aggregated integration test is compiled and run twice.**
`35a246eb3` merged the `tests/` files into one target per crate
(`dsp_parity`, `encoder_parity`, `svtav1_suite`) and measured the incremental
rebuild dropping from 5.34 s to 2.36 s. `d2abc282b` then reported 162 of those
files as "never run" and declared each one as its own `[[test]]` again. They
were running, as modules of the aggregators. `cargo metadata` shows that 50/50
dsp files, 102/103 encoder files and 10/10 facade files are now in both
targets. Delete the standalone entries for aggregated files, and add a check
that every `tests/*.rs` belongs to exactly one target. Before deleting, check
which gates pass `--test <name>`.

**T2. Tests in `tests/` force the public API.** 71 of the encoder's 107
public modules are imported by integration tests, and 8 are imported by
nothing else (`lf_levels`, `port_frame_update`, `port_full_loop_md`,
`port_pass2_gop`, `port_pd_gop`, `port_rc_rtc_cbr`,
`port_superres_decision`, `port_tune_vmaf`). Move function-level `c_parity_*`
differentials into in-crate `#[cfg(test)]` modules, or expose a single
`#[doc(hidden)] pub mod __testing` behind a feature. Then modules can become
`pub(crate)` and the facade stops leaking them.

**T3. One cell harness instead of 39 copies in bash.** There are 100 shell
tools (17.1k lines). 49 run in CI; 51 do not, including `bd10_photo_gate.sh`,
whose 191/191 figure `rust/CLAUDE.md` cites as a ledger number.
39 scripts each re-implement "run `identity_run`, run `capture_c_trace`,
`cmp`", and `identity_run`'s configuration surface is 41 environment
variables. A `CellSpec` struct (content, dims, qp, preset, bit depth, frames,
knobs) with one Rust runner and cell lists as data would turn each gate into a
table. A cell could then not differ from another in how it was invoked, only
in what it asks for.

**T4. Add the missing middle layer.** Tests today are either function-level C
differentials or whole-encode byte/decoder checks. Nothing tests a stage
boundary against C: the picture signals, the per-SB PD0/PD1 levels, the TF
output picture, the TPL stats. That is why the tested `sig_deriv_*`
orchestrators can be dead while the untested hand-assembly runs (S3). The
`*_traced.rs` tests already capture C trace records; extend them to stage
outputs once S2 gives the stages explicit outputs.

**T5. A never-panics sweep over the public configuration.** The alt-SSIM panic
and the GM/CORNERS panic sit behind options that the byte-identity matrix does
not sample densely. A property sweep over `AvifEncoder` and `EncodePipeline`
options at tiny sizes (16×16 to 64×64, 1–3 frames) should assert "`Ok` or an
explicit `Err`, never a panic". It is cheap and would have caught both.

**T6. Gates for the regressions this review found by hand:** a per-crate
`--no-default-features` check (defect 2); `incant_tiers.py --missing` against an
allowlist (S8); `deadfns.py` against an allowlist of faithful-but-unreachable
ports. The `deadfns.py` gate needs one fix first: it reports `Deref`/`Index`
impls as dead.

**T7. Move inline tests out of product files.** There are 28.6k lines of
`#[cfg(test)]` inside encoder `src/`: `pipeline.rs` 2,359, `md_search.rs`
1,519, `pd0.rs` 1,303, `obu.rs` 905. Sibling `tests.rs` modules
(`leaf_funnel/tests.rs` is the existing pattern) keep grep and reading on
product code.

## Performance, by expected size

1. **x86 intra and 10-bit kernels are scalar** (S8). All-intra stills are the
   primary product, and intra prediction runs for every intra candidate at
   MDS0/1/3.
2. **Inter prediction and TF have no SIMD on any arch** for 10-bit convolve,
   scaled convolve, the diff-weighted masks and `enc_make_inter_predictor`
   (which TF uses). The temporal-filter kernels are scalar too, and aarch64
   convolve is scalar.
3. **ME without multi-offset SAD** (`inter_me/sad.rs:376-500` calls a single
   8×8 SAD 8× per offset).
4. **10-bit mode decision does 8-bit work too** (S4). `tx_unit_hbd_screened`
   makes 8 allocations per transform block and uses scalar
   residual/recon/SSE.
5. **Discarded per-frame work in `pipeline.rs`.**
   - A per-SB full-pel search fills `mv_map` after the tile encode, but its
     only reader, the legacy partition search, runs before the fill and always
     sees zeros.
   - `tpl_sb_qp_offsets` runs a full-frame pass per inter frame and is
     consumed by `let _ =`.
   - The symbol-writing walk runs three times per frame; the `recon_only`
     walk exists to produce deblock geometry and chroma recon that MD already
     has.
   - When TPL or SSIM lambdas apply, each leaf deep-clones the whole
     `FunnelFrame` twice (~256 KB with IntraBC on) to override three lambdas.
6. **Per-call allocations** (S6). Also: `sb_qindex` reads 4,096 pixels per SB
   through `&dyn Fn`, and the tile threads are fresh OS threads per frame,
   joined in barrier batches.

## Cleanup, smaller

- Dead or unwired modules still public: `perceptual`, `lf_levels`,
  `multipass`, `tile` (a non-conformant tile model), `segmentation`, and in
  `sb128_geom` more than 180 of 264 lines.
  - `segmentation`'s AQ/ROI is a feature gap, not dead weight: it is fully
    C-parity-tested and never wired. Document it or wire it.
- Test-only modules: `port_pass2_gop`, `port_pd_gop`, `port_rc_rtc_cbr`.
- Split and duplicated adaptive-context lists: `FrameCdfs::reset_symbol_counters`
  and `for_each_field_mut` each hand-list ~70 CDF fields; nothing ties them to
  the struct.
- Writer/counter twins by sink type (`gm.rs` `BitWriter` primitives vs
  `entropy/lr.rs` `AomWriter` primitives vs the `count_*` variants). A small
  `BitSink` trait with a counting sink gives one implementation.
- `inter_mvp` and `intrabc_mvp` duplicate the full ref-MV scan (Rust-introduced).
- The lossless WHT block appears 6×.
- The partition-child offset walk appears 3× in `bd10_reencode` alone.
- `Pd0Level` enum beside `pd0_level` `u8` constants.
- `InputCoeffLvl` beside `quant::CoeffLvl`.
- The C resolution breakpoints (`0x28500`…) are open-coded in four places
  beside `ResolutionRange`.
