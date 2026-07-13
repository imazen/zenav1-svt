# Coverage gate — EbSvtAv1EncConfiguration surface

Auto-derived from `Source/API/EbSvtAv1Enc.h` by `gen_coverage` (do not
edit the field list by hand — rerun the generator after baseline
bumps). Statuses ARE hand-maintained and survive regeneration:
`unmapped` -> `mapped` (plumbed through the Rust config) ->
`tested:<test>` (a passing test exercises it against the gates).
The coverage gate is green when every row is `tested`.

**121 fields** — tested: 0, mapped: 4, unmapped: 117

| field | type | status | notes |
|---|---|---|---|
| `enc_mode` | `int8_t` | mapped | is stored in pcs->enc_mode. |
| `intra_period_length` | `int32_t` | unmapped | Default is -2. |
| `intra_refresh_type` | `SvtAv1IntraRefreshType` | unmapped | Default is 1. |
| `hierarchical_levels` | `uint32_t` | unmapped | Default is auto |
| `pred_structure` | `PredStructure` | unmapped | Default is RANDOM_ACCESS. |
| `source_width` | `uint32_t` | mapped | Default is 0. |
| `source_height` | `uint32_t` | mapped | Default is 0. |
| `forced_max_frame_width` | `uint32_t` | unmapped | the maximum height between renditions when switch frame feature is on. |
| `forced_max_frame_height` | `uint32_t` | unmapped |  |
| `frame_rate_numerator` | `uint32_t` | unmapped | Default is 0. |
| `frame_rate_denominator` | `uint32_t` | unmapped | Default is 0. |
| `encoder_bit_depth` | `uint32_t` | unmapped | Default is 8. |
| `encoder_color_format` | `EbColorFormat` | unmapped | Default is YUV420. |
| `profile` | `EbAv1SeqProfile` | unmapped | Default is MAIN_PROFILE. |
| `tier` | `uint32_t` | unmapped | Default is 0. |
| `level` | `uint32_t` | unmapped | Default is 0. |
| `color_primaries` | `EbColorPrimaries` | unmapped | values are from EbColorPrimaries |
| `transfer_characteristics` | `EbTransferCharacteristics` | unmapped | values are from EbTransferCharacteristics |
| `matrix_coefficients` | `EbMatrixCoefficients` | unmapped | values are from EbMatrixCoefficients |
| `color_range` | `EbColorRange` | unmapped | 1: full swing. |
| `mastering_display` | `struct EbSvtAv1MasteringDisplayInfo` | unmapped | values are from set using svt_aom_parse_mastering_display() |
| `content_light_level` | `struct EbContentLightLevel` | unmapped | values are from set using svt_aom_parse_content_light_level() |
| `chroma_sample_position` | `EbChromaSamplePosition` | unmapped | EB_CSP_COLOCATED: value 2 from H.273 AKA "top left" |
| `rate_control_mode` | `uint8_t` | unmapped | Default is 0. |
| `qp` | `uint32_t` | mapped | Default is 50. |
| `use_qp_file` | `bool` | unmapped | Default is 0. |
| `target_bit_rate` | `uint32_t` | unmapped | Default is 2000513. |
| `max_bit_rate` | `uint32_t` | unmapped | Default is 0. |
| `max_qp_allowed` | `uint32_t` | unmapped | Default is 63. |
| `min_qp_allowed` | `uint32_t` | unmapped | Default is auto. |
| `vbr_min_section_pct` | `uint32_t` | unmapped | Default is 0. |
| `vbr_max_section_pct` | `uint32_t` | unmapped | Default is 2000. |
| `under_shoot_pct` | `uint32_t` | unmapped | Default is 25 for CBR and 50 for VBR. |
| `over_shoot_pct` | `uint32_t` | unmapped | Default is 25. |
| `mbr_over_shoot_pct` | `uint32_t` | unmapped | Default is 50. |
| `starting_buffer_level_ms` | `int64_t` | unmapped | Default is 600. |
| `optimal_buffer_level_ms` | `int64_t` | unmapped | Default is 600. |
| `maximum_buffer_size_ms` | `int64_t` | unmapped | Default is 1000. |
| `rc_stats_buffer` | `SvtAv1FixedBuf` | unmapped | input / output buffer to be used for multi-pass encoding |
| `pass` | `int` | unmapped |  |
| `use_fixed_qindex_offsets` | `uint8_t` | unmapped | Default is 0. |
| `qindex_offsets` | `int32_t[EB_MAX_TEMPORAL_LAYERS]` | unmapped |  |
| `key_frame_chroma_qindex_offset` | `int32_t` | unmapped |  |
| `key_frame_qindex_offset` | `int32_t` | unmapped |  |
| `chroma_qindex_offsets` | `int32_t[EB_MAX_TEMPORAL_LAYERS]` | unmapped |  |
| `luma_y_dc_qindex_offset` | `int32_t` | unmapped |  |
| `chroma_u_dc_qindex_offset` | `int32_t` | unmapped |  |
| `chroma_u_ac_qindex_offset` | `int32_t` | unmapped |  |
| `chroma_v_dc_qindex_offset` | `int32_t` | unmapped |  |
| `chroma_v_ac_qindex_offset` | `int32_t` | unmapped |  |
| `enable_dlf_flag` | `uint8_t` | unmapped | 2: more accurate (slower) |
| `film_grain_denoise_strength` | `uint32_t` | unmapped | Default is 0. |
| `film_grain_denoise_apply` | `uint8_t` | unmapped | Default is 0. |
| `cdef_level` | `int` | unmapped | Default is -1. |
| `enable_restoration_filtering` | `int` | unmapped | Default is -1. |
| `enable_mfmv` | `int` | unmapped | Default is -1. |
| `scene_change_detection` | `uint32_t` | unmapped | Default is 1. |
| `tile_columns` | `int32_t` | unmapped | Default is 0. |
| `tile_rows` | `int32_t` | unmapped |  |
| `look_ahead_distance` | `uint32_t` | unmapped | Default depends on rate control mode. |
| `recode_loop` | `uint32_t` | unmapped | default is 4 |
| `screen_content_mode` | `uint32_t` | unmapped | Default is 0. |
| `aq_mode` | `uint8_t` | unmapped | 2: CRF (per-frame QPs and per-SB delta-QPs derived using TPL) |
| `enable_tf` | `uint8_t` | unmapped | Default is 1. |
| `enable_overlays` | `bool` | unmapped |  |
| `tune` | `uint8_t` | unmapped | Default is 1. |
| `superres_mode` | `uint8_t` | unmapped | super-resolution parameters |
| `superres_denom` | `uint8_t` | unmapped |  |
| `superres_kf_denom` | `uint8_t` | unmapped |  |
| `superres_qthres` | `uint8_t` | unmapped |  |
| `superres_kf_qthres` | `uint8_t` | unmapped |  |
| `superres_auto_search_type` | `uint8_t` | unmapped |  |
| `fast_decode` | `uint8_t` | unmapped | 2: Level 2 of decoder-targeted speed optimizations (faster decoder-speed than level 1) |
| `sframe_dist` | `int32_t` | unmapped | >0: S-Frame on and indicates the number of frames after which a frame may be coded as an S-Frame |
| `sframe_mode` | `EbSFrameMode` | unmapped | SFRAME_DEC_POSI: if the considered frame in decode order is not an altref frame, modify the mini-GOP structure to promote its previous frame to an altref frame, and set the next altref to an S-Frame |
| `level_of_parallelism` | `uint32_t` | unmapped | will map to the highest level. |
| `use_cpu_flags` | `EbCpuFlags` | unmapped | Default is EB_CPU_FLAGS_ALL. |
| `stat_report` | `uint32_t` | unmapped | Default is 0. |
| `recon_enabled` | `bool` | unmapped | Default is false. |
| `force_key_frames` | `bool` | unmapped | 1.0.0: Any additional fields shall go after here |
| `multiply_keyint` | `bool` | unmapped | multiply by fps_num/fps_den. |
| `resize_mode` | `uint8_t` | unmapped | the available modes are defined in RESIZE_MODE |
| `resize_denom` | `uint8_t` | unmapped | resolution in both width and height |
| `resize_kf_denom` | `uint8_t` | unmapped | resolution in both width and height |
| `enable_qm` | `bool` | unmapped | Default is false. |
| `min_qm_level` | `uint8_t` | unmapped | Default is 8. |
| `max_qm_level` | `uint8_t` | unmapped | Default is 15. |
| `gop_constraint_rc` | `bool` | unmapped | Default is 0. |
| `lambda_scale_factors` | `int32_t[SVT_AV1_FRAME_UPDATE_TYPES]` | unmapped | factor >> 7 (/ 128) is the actual value in float |
| `enable_dg` | `bool` | unmapped | Default is 1. |
| `startup_mg_size` | `uint8_t` | unmapped | Default is 0. |
| `startup_qp_offset` | `int8_t` | unmapped | Default is 0. |
| `frame_scale_evts` | `SvtAv1FrameScaleEvts` | unmapped | resize_denoms:    array of scaling denominators of non-key-frame |
| `enable_roi_map` | `bool` | unmapped | Default is 0. |
| `tf_strength` | `uint8_t` | unmapped | 10 + (4 - 4) = 10 (2x stronger) |
| `fgs_table` | `AomFilmGrain*` | unmapped | Stores the optional film grain synthesis info |
| `enable_variance_boost` | `bool` | unmapped | Default is false. |
| `variance_boost_strength` | `uint8_t` | unmapped | Default is 2 |
| `variance_octile` | `uint8_t` | unmapped | Default is 5 |
| `sharpness` | `int8_t` | unmapped | Default is 0 (medium sharpness). |
| `variance_boost_curve` | `uint8_t` | unmapped | Default is 0. |
| `luminance_qp_bias` | `uint8_t` | unmapped | Default is 0 (disabled). |
| `lossless` | `bool` | unmapped | Default is false. |
| `avif` | `bool` | unmapped | Default is false. |
| `min_chroma_qm_level` | `uint8_t` | unmapped | Default is 8. |
| `max_chroma_qm_level` | `uint8_t` | unmapped | Default is 15. |
| `rtc` | `bool` | unmapped | Default is false. |
| `qp_scale_compress_strength` | `uint8_t` | unmapped | Default is 1 |
| `sframe_posi` | `SvtAv1SFramePositions` | unmapped |  |
| `sframe_qp` | `uint8_t` | unmapped |  |
| `sframe_qp_offset` | `int8_t` | unmapped |  |
| `adaptive_film_grain` | `bool` | unmapped | Default is 1 |
| `max_tx_size` | `uint8_t` | unmapped | aren't exposed as options. |
| `extended_crf_qindex_offset` | `uint8_t` | unmapped | Default is 0 if CRF is an integer |
| `ac_bias` | `double` | unmapped | Default is 0.00. |
| `hbd_mds` | `int` | unmapped | Default is -1 |
| `enable_tf_key` | `bool` | unmapped | Default is 1. |
| `max_intra_bitrate_pct` | `uint32_t` | unmapped | Default is 300. |
| `max_inter_bitrate_pct` | `uint32_t` | unmapped | Default is 0. |
| `enable_intrabc` | `bool` | unmapped | Default is true. |
| `max_managed_refs` | `uint8_t` | unmapped | stack contents and unexpectedly enable the feature. |

## CLI flag surface (SvtAv1EncApp)

         **134 flags** — tested: 0, mapped: 0, unmapped: 134

| field | type | status | notes |
|---|---|---|---|
| `--help` | `flag` | unmapped | Shows the command line options currently available |
| `--color-help` | `flag` | unmapped | Extra help for adding AV1 metadata to the bitstream |
| `--version` | `flag` | unmapped | Shows the version of the library that's linked to the library |
| `-i` | `flag` | unmapped | Input raw video (y4m and yuv) file path, use `stdin` or `-` to read from pipe |
| `--input` | `flag` | unmapped | Input raw video (y4m and yuv) file path, use `stdin` or `-` to read from pipe |
| `--allow-mmap-file` | `flag` | unmapped | Allow memory mapping for regular input file. Performance is platform dependent |
| `-b` | `flag` | unmapped |  |
| `--output` | `flag` | unmapped |  |
| `--ivf` | `flag` | unmapped | Output bitstream in IVF container format (default) |
| `--obu` | `flag` | unmapped | Output bitstream as raw OBU (Open Bitstream Units) without IVF container |
| `-c` | `flag` | unmapped | Configuration file path |
| `--config` | `flag` | unmapped | Configuration file path |
| `--errlog` | `flag` | unmapped | Error file path, defaults to stderr |
| `-o` | `flag` | unmapped | Reconstructed yuv file path |
| `--recon` | `flag` | unmapped | Reconstructed yuv file path |
| `--stat-file` | `flag` | unmapped | PSNR / SSIM per picture stat output file path, requires `--enable-stat-report 1` |
| `--progress` | `flag` | unmapped | Verbosity of the output, default is 1 [0: no progress is printed, 2: detailed progress] |
| `--no-progress" // tbd if it should be removed` | `flag` | unmapped |  |
| `--preset` | `flag` | unmapped |  |
| `-w` | `flag` | unmapped | Frame width in pixels, inferred if y4m, default is 0 [4-16384] |
| `--width` | `flag` | unmapped | Frame width in pixels, inferred if y4m, default is 0 [4-16384] |
| `-h` | `flag` | unmapped | Frame height in pixels, inferred if y4m, default is 0 [4-8704] |
| `--height` | `flag` | unmapped | Frame height in pixels, inferred if y4m, default is 0 [4-8704] |
| `--forced-max-frame-width` | `flag` | unmapped | Maximum frame width value to force, default is 0 [4-16384] |
| `--forced-max-frame-height` | `flag` | unmapped | Maximum frame height value to force, default is 0 [4-8704] |
| `-n` | `flag` | unmapped |  |
| `--frames` | `flag` | unmapped |  |
| `--nb` | `flag` | unmapped |  |
| `--profile` | `flag` | unmapped | Bitstream profile, default is 0 [0: main, 1: high, 2: professional] |
| `--level` | `flag` | unmapped |  |
| `--fps-num` | `flag` | unmapped | Input video frame rate numerator, default is 60000 [0-2^32-1] |
| `--fps-denom` | `flag` | unmapped | Input video frame rate denominator, default is 1000 [0-2^32-1] |
| `--input-depth` | `flag` | unmapped | Input video file and output bitstream bit-depth, default is 8 [8, 10] |
| `--inj" // no Eval` | `flag` | unmapped | Inject pictures to the library at defined frame rate, default is 0 [0-1] |
| `--inj-frm-rt" // no Eval` | `flag` | unmapped | Set injector frame rate, only applicable with `--inj 1`, default is 60 [0-240] |
| `--enable-stat-report` | `flag` | unmapped | Calculates and outputs PSNR SSIM metrics at the end of encoding, default is 0 [0-1] |
| `--asm` | `flag` | unmapped |  |
| `--rc` | `flag` | unmapped |  |
| `-q` | `flag` | unmapped | Initial QP level value, default is 35 [1-63] |
| `--qp` | `flag` | unmapped | Initial QP level value, default is 35 [1-63] |
| `--crf` | `flag` | unmapped |  |
| `--cqp` | `flag` | unmapped |  |
| `--tbr` | `flag` | unmapped |  |
| `--mbr` | `flag` | unmapped | Maximum Bitrate (kbps) only applicable for CRF encoding, default is 0 [1-100000] |
| `--use-q-file` | `flag` | unmapped |  |
| `--qpfile` | `flag` | unmapped | Path to a file containing per picture QP value separated by newlines |
| `--max-qp` | `flag` | unmapped | Maximum (highest) quantizer, only applicable for VBR and CBR, default is 63 [1-63] |
| `--min-qp` | `flag` | unmapped | Minimum (lowest) quantizer, only applicable for VBR and CBR, default is 1 [1-63] |
| `--aq-mode` | `flag` | unmapped |  |
| `--use-fixed-qindex-offsets` | `flag` | unmapped |  |
| `--key-frame-qindex-offset` | `flag` | unmapped |  |
| `--key-frame-chroma-qindex-offset` | `flag` | unmapped |  |
| `--qindex-offsets` | `flag` | unmapped |  |
| `--chroma-qindex-offsets` | `flag` | unmapped |  |
| `--luma-y-dc-qindex-offset` | `flag` | unmapped | Luma Y DC Qindex Offset |
| `--chroma-u-dc-qindex-offset` | `flag` | unmapped | Chroma U DC Qindex Offset |
| `--chroma-u-ac-qindex-offset` | `flag` | unmapped | Chroma U AC Qindex Offset |
| `--chroma-v-dc-qindex-offset` | `flag` | unmapped | Chroma V DC Qindex Offset |
| `--chroma-v-ac-qindex-offset` | `flag` | unmapped | Chroma V AC Qindex Offset |
| `--lambda-scale-factors` | `flag` | unmapped |  |
| `--undershoot-pct` | `flag` | unmapped |  |
| `--overshoot-pct` | `flag` | unmapped |  |
| `--mbr-overshoot-pct` | `flag` | unmapped |  |
| `--max-intra-bitrate-pct` | `flag` | unmapped |  |
| `--max-inter-bitrate-pct` | `flag` | unmapped |  |
| `--gop-constraint-rc` | `flag` | unmapped |  |
| `--buf-sz` | `flag` | unmapped | Client buffer size (ms), only applicable for CBR, default is 6000 [0-10000] |
| `--buf-initial-sz` | `flag` | unmapped | Client initial buffer size (ms), only applicable for CBR, default is 4000 [0-10000] |
| `--buf-optimal-sz` | `flag` | unmapped | Client optimal buffer size (ms), only applicable for CBR, default is 5000 [0-10000] |
| `--recode-loop` | `flag` | unmapped |  |
| `--minsection-pct` | `flag` | unmapped | GOP min bitrate (expressed as a percentage of the target rate), default is 0 [0-100] |
| `--maxsection-pct` | `flag` | unmapped |  |
| `--enable-qm` | `flag` | unmapped | Enable quantisation matrices, default is 0 [0-1] |
| `--qm-min` | `flag` | unmapped | Min quant matrix flatness, default is 8 [0-15] |
| `--qm-max` | `flag` | unmapped | Max quant matrix flatness, default is 15 [0-15] |
| `--chroma-qm-min` | `flag` | unmapped | Min chroma quant matrix flatness, default is 8 [0-15] |
| `--chroma-qm-max` | `flag` | unmapped | Max chroma quant matrix flatness, default is 15 [0-15] |
| `--roi-map-file` | `flag` | unmapped | Enable Region Of Interest and specify a picture based QP Offset map file, default is off |
| `--tf-strength` | `flag` | unmapped | Adjust temporal filtering strength, default is 3 [0-4] |
| `--luminance-qp-bias` | `flag` | unmapped | Adjusts a frame's QP based on its average luma value, default is 0 [0-100] |
| `--sharpness` | `flag` | unmapped | Bias towards decreased/increased sharpness, default is 0 [-7 to 7] |
| `--pass` | `flag` | unmapped |  |
| `--stats` | `flag` | unmapped | Filename for multi-pass encoding, default is \ |
| `--passes` | `flag` | unmapped |  |
| `--keyint` | `flag` | unmapped |  |
| `--irefresh-type" // no Eval` | `flag` | unmapped | Intra refresh type, default is 2 [1: FWD Frame (Open GOP), 2: KEY Frame (Closed GOP)] |
| `--scd` | `flag` | unmapped | Scene change detection control, default is 0 [0-1] |
| `--lookahead` | `flag` | unmapped |  |
| `--hierarchical-levels" // no Eval` | `flag` | unmapped |  |
| `--pred-struct` | `flag` | unmapped | Set prediction structure, default is 2 [1: low delay frames, 2: random access] |
| `--rtc` | `flag` | unmapped |  |
| `--force-key-frames` | `flag` | unmapped | Force key frames at the comma separated specifiers. `#f` for frames, `#.#s` for seconds |
| `--startup-mg-size` | `flag` | unmapped |  |
| `--startup-qp-offset` | `flag` | unmapped |  |
| `--tile-rows` | `flag` | unmapped | Number of tile rows to use, `TileRow == log2(x)`, default changes per resolution but is 1 [0-6] |
| `--tile-columns` | `flag` | unmapped |  |
| `--enable-cdef` | `flag` | unmapped | Enable Constrained Directional Enhancement Filter, default is 1 [0-1] |
| `--enable-restoration` | `flag` | unmapped | Enable loop restoration filter, default is 1 [0-1] |
| `--enable-mfmv` | `flag` | unmapped | Motion Field Motion Vector control, default is -1 [-1: auto, 0-1] |
| `--enable-dg` | `flag` | unmapped | Dynamic GoP control, default is 1 [0-1] |
| `--fast-decode` | `flag` | unmapped | Fast Decoder levels, default is 0 [0-2] |
| `--enable-tf` | `flag` | unmapped | Enable ALT-REF (temporally filtered) frames, default is 1 [0-2] |
| `--enable-kf-tf` | `flag` | unmapped | Enable MCTF for key frames, default is 1 [0-1] |
| `--tune` | `flag` | unmapped |  |
| `--scm` | `flag` | unmapped |  |
| `--enable-intrabc` | `flag` | unmapped | Enable Intra Block Copy, default is 1 [0: off, 1: on] |
| `--film-grain` | `flag` | unmapped | Enable film grain, default is 0 [0: off, 1-50: level of denoising for film grain] |
| `--film-grain-denoise` | `flag` | unmapped |  |
| `--fgs-table` | `flag` | unmapped | Set the film grain model table path |
| `--sframe-dist` | `flag` | unmapped | S-Frame interval (frames) (0: OFF[default], > 0: ON) |
| `--sframe-mode` | `flag` | unmapped |  |
| `--sframe-posi` | `flag` | unmapped |  |
| `--sframe-qp` | `flag` | unmapped | S-Frame setup qp, a list separated by ',', QP value(s) set with S-Frame insertion |
| `--sframe-qp-offset` | `flag` | unmapped |  |
| `--lossless` | `flag` | unmapped | Enable lossless coding, default is 0 [0-1] |
| `--avif` | `flag` | unmapped | Enable still-picture coding, default is 0 [0-1] |
| `--color-primaries` | `flag` | unmapped | Color primaries, refer to --color-help. Default is 2 [0-12, 22] |
| `--transfer-characteristics` | `flag` | unmapped | Transfer characteristics, refer to --color-help. Default is 2 [0-22] |
| `--matrix-coefficients` | `flag` | unmapped | Matrix coefficients, refer to --color-help. Default is 2 [0-14] |
| `--color-range` | `flag` | unmapped | Color range, default is 0 [0: Studio, 1: Full] |
| `--chroma-sample-position` | `flag` | unmapped |  |
| `--mastering-display` | `flag` | unmapped |  |
| `--content-light` | `flag` | unmapped |  |
| `--enable-variance-boost` | `flag` | unmapped | Enable Variance Boost, default is 0 [0-1] |
| `--variance-boost-strength` | `flag` | unmapped | Variance Boost strength, default is 2 [1-4] |
| `--variance-octile` | `flag` | unmapped | Octile for Variance Boost, default is 5 [1-8] |
| `--variance-boost-curve` | `flag` | unmapped | Curve for Variance Boost, default is 0 [0-2] |
| `--qp-scale-compress-strength` | `flag` | unmapped | QP scale compress strength, default is 0 [0-3] |
| `--adaptive-film-grain` | `flag` | unmapped | Adapts film grain blocksize based on video resolution, default is 1 [0-1] |
| `--max-tx-size` | `flag` | unmapped | Limits the allowed transform sizes to the specified, default is 64 [32,64] |
| `--ac-bias` | `flag` | unmapped | Strength of AC bias in rate distortion, default is 0.0 [0.0-8.0] |
| `--hbd-mds` | `flag` | unmapped |  |
| `--tier` | `flag` | unmapped | Tier |
| `--intra-period` | `flag` | unmapped | IntraPeriod |
