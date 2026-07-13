# Coverage gate — EbSvtAv1EncConfiguration surface

Auto-derived from `Source/API/EbSvtAv1Enc.h` by `gen_coverage` (do not
edit the field list by hand — rerun the generator after baseline
bumps). Statuses ARE hand-maintained and survive regeneration:
`unmapped` -> `mapped` (plumbed through the Rust config) ->
`tested:<test>` (a passing test exercises it against the gates).
The coverage gate is green when every row is `tested`.

**121 fields** — tested: 0, mapped: 0, unmapped: 121

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
