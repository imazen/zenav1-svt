# Documentation index — a classification, not a reading list

**You probably do not want this file.** It is an exhaustive table of every
tracked Markdown and metadata document with a one-word role, built by an audit
on 2026-09-08 and useful for one thing: deciding whether a document you have
already found is current, a snapshot, or evidence. To find the document you
need in the first place, use the router in
[CONTEXT-HANDOFF.md](../../CONTEXT-HANDOFF.md) or the read order in
[AGENTS.md](../../AGENTS.md).

The roles below are stable, but individual rows date from 2026-09-08 and a
document's role can have changed since. The first line of the document itself
wins over this table. The C submodule owns its own upstream documentation and
is outside this cleanup.

- **Current**: source-reviewed API, handoff, support, workflow or remaining requirements.
- **Historical reference**: old port maps, campaigns and superseded state. Their new
  headers delimit stale counts, priorities and line references; reproduce before reuse.
- **Measurement record**: preserved benchmark/capture evidence; dates, paths, source
  identities and metrics remain as captured. Unsupported prose is not promoted to fact.
- **Archived original**: exact pre-cleanup text, plus a provenance header. Old relative
  links refer to the original file location and revision, not the archive directory.
- **Generated/shape ledger**: source-generated refusals or C-API-shape inventory, not
  a feature-count scoreboard.
- **Algorithm/technical reference**: source-derived specs, fixture notes and tool
  instructions. Specs originate at older C; current named C source wins on conflicts.

| Document | Role |
|---|---|
| [CHANGELOG.md](../../CHANGELOG.md) | Historical change record |
| [CONTEXT-HANDOFF.md](../../CONTEXT-HANDOFF.md) | Current |
| [ENCODER-POLICY-GOAL.md](../../ENCODER-POLICY-GOAL.md) | Current |
| [PORTING.md](../../PORTING.md) | Current |
| [README.md](../../README.md) | Current |
| [benchmarks/dsp_kernel_tiers_aarch64_2026-07-28.md](../../benchmarks/dsp_kernel_tiers_aarch64_2026-07-28.md) | Measurement record |
| [benchmarks/zensim_hdr_target_wave_2026-08-27.md](../../benchmarks/zensim_hdr_target_wave_2026-08-27.md) | Measurement record |
| [rust/CLAUDE.md](../CLAUDE.md) | Current |
| [rust/COVERAGE.md](../COVERAGE.md) | Generated/shape ledger |
| [rust/DEFER.md](../DEFER.md) | Historical reference |
| [rust/README.md](../README.md) | Current |
| [rust/STATUS.md](../STATUS.md) | Current |
| [rust/benchmarks/alignment_gate_teeth_2026-08-14.md](../benchmarks/alignment_gate_teeth_2026-08-14.md) | Measurement record |
| [rust/benchmarks/alloc_bufpool_null_2026-08-13.meta](../benchmarks/alloc_bufpool_null_2026-08-13.meta) | Measurement record |
| [rust/benchmarks/alloc_decisioncopy_ab_2026-08-13.meta](../benchmarks/alloc_decisioncopy_ab_2026-08-13.meta) | Measurement record |
| [rust/benchmarks/alloc_traffic_null_2026-08-07.meta](../benchmarks/alloc_traffic_null_2026-08-07.meta) | Measurement record |
| [rust/benchmarks/animation_hdr_2026-09-07.md](../benchmarks/animation_hdr_2026-09-07.md) | Measurement record |
| [rust/benchmarks/arm_audit_2026-09-06/README.md](../benchmarks/arm_audit_2026-09-06/README.md) | Measurement record |
| [rust/benchmarks/arm_audit_2026-09-06/baseline.pointer.md](../benchmarks/arm_audit_2026-09-06/baseline.pointer.md) | Measurement record |
| [rust/benchmarks/arm_audit_2026-09-06/full_suite.pointer.md](../benchmarks/arm_audit_2026-09-06/full_suite.pointer.md) | Measurement record |
| [rust/benchmarks/arm_audit_2026-09-06/oracle-resolution.md](../benchmarks/arm_audit_2026-09-06/oracle-resolution.md) | Measurement record |
| [rust/benchmarks/arm_audit_2026-09-06/svt-oracle-full.pointer.md](../benchmarks/arm_audit_2026-09-06/svt-oracle-full.pointer.md) | Measurement record |
| [rust/benchmarks/arm_pairwise_2026-09-07.meta](../benchmarks/arm_pairwise_2026-09-07.meta) | Measurement record |
| [rust/benchmarks/arm_pairwise_release_2026-09-08.md](../benchmarks/arm_pairwise_release_2026-09-08.md) | Measurement record |
| [rust/benchmarks/bd10_photo_p0p3_2026-07-23.meta](../benchmarks/bd10_photo_p0p3_2026-07-23.meta) | Measurement record |
| [rust/benchmarks/bd10_pq_2026-08-28.md](../benchmarks/bd10_pq_2026-08-28.md) | Measurement record |
| [rust/benchmarks/callcount_2026-09-04.meta](../benchmarks/callcount_2026-09-04.meta) | Measurement record |
| [rust/benchmarks/callcount_inter_2026-09-05.meta](../benchmarks/callcount_inter_2026-09-05.meta) | Measurement record |
| [rust/benchmarks/callcount_mds1skip_2026-09-04.meta](../benchmarks/callcount_mds1skip_2026-09-04.meta) | Measurement record |
| [rust/benchmarks/callcount_realimg_2026-09-04.meta](../benchmarks/callcount_realimg_2026-09-04.meta) | Measurement record |
| [rust/benchmarks/callcount_txtscreen_2026-09-04.meta](../benchmarks/callcount_txtscreen_2026-09-04.meta) | Measurement record |
| [rust/benchmarks/cancel_latency_2026-09-07_i265.meta](../benchmarks/cancel_latency_2026-09-07_i265.meta) | Measurement record |
| [rust/benchmarks/cdef_find_dir_simd_2026-09-05.meta](../benchmarks/cdef_find_dir_simd_2026-09-05.meta) | Measurement record |
| [rust/benchmarks/cdef_i16_ab_2026-09-03.meta](../benchmarks/cdef_i16_ab_2026-09-03.meta) | Measurement record |
| [rust/benchmarks/cdef_i16_kernel_2026-09-05.meta](../benchmarks/cdef_i16_kernel_2026-09-05.meta) | Measurement record |
| [rust/benchmarks/cdef_neon_4wide_i16_2026-09-05.meta](../benchmarks/cdef_neon_4wide_i16_2026-09-05.meta) | Measurement record |
| [rust/benchmarks/cfl_branchfree_2026-09-05.meta](../benchmarks/cfl_branchfree_2026-09-05.meta) | Measurement record |
| [rust/benchmarks/cfl_simd_kernel_2026-09-05.meta](../benchmarks/cfl_simd_kernel_2026-09-05.meta) | Measurement record |
| [rust/benchmarks/chromadetect_ab_2026-09-03.meta](../benchmarks/chromadetect_ab_2026-09-03.meta) | Measurement record |
| [rust/benchmarks/ci_decoder_routing_2026-09-07.md](../benchmarks/ci_decoder_routing_2026-09-07.md) | Measurement record |
| [rust/benchmarks/coeffctx_ab_2026-09-03.meta](../benchmarks/coeffctx_ab_2026-09-03.meta) | Measurement record |
| [rust/benchmarks/compute_stats_cshape_2026-09-04.meta](../benchmarks/compute_stats_cshape_2026-09-04.meta) | Measurement record |
| [rust/benchmarks/compute_stats_x86_recheck_2026-09-05.meta](../benchmarks/compute_stats_x86_recheck_2026-09-05.meta) | Measurement record |
| [rust/benchmarks/coverage_combos_2026-08-28_arm64_axes12.meta](../benchmarks/coverage_combos_2026-08-28_arm64_axes12.meta) | Measurement record |
| [rust/benchmarks/crf_cqp_equivalence_2026-07-24.md](../benchmarks/crf_cqp_equivalence_2026-07-24.md) | Measurement record |
| [rust/benchmarks/cross_isa_x86_verification_2026-08-31.md](../benchmarks/cross_isa_x86_verification_2026-08-31.md) | Measurement record |
| [rust/benchmarks/dqfull_ab_2026-09-03.meta](../benchmarks/dqfull_ab_2026-09-03.meta) | Measurement record |
| [rust/benchmarks/dqzero_ab_2026-09-03.meta](../benchmarks/dqzero_ab_2026-09-03.meta) | Measurement record |
| [rust/benchmarks/drpred_neon_ab_2026-09-03.meta](../benchmarks/drpred_neon_ab_2026-09-03.meta) | Measurement record |
| [rust/benchmarks/encode_gap_attribution_2026-08-07.md](../benchmarks/encode_gap_attribution_2026-08-07.md) | Measurement record |
| [rust/benchmarks/entropy_coder_cshape_2026-09-05.meta](../benchmarks/entropy_coder_cshape_2026-09-05.meta) | Measurement record |
| [rust/benchmarks/ext_sad8_ab_2026-09-02.meta](../benchmarks/ext_sad8_ab_2026-09-02.meta) | Measurement record |
| [rust/benchmarks/f1diff_q55_localization_2026-09-03.md](../benchmarks/f1diff_q55_localization_2026-09-03.md) | Measurement record |
| [rust/benchmarks/frame2_cdef_skip_2026-09-03.md](../benchmarks/frame2_cdef_skip_2026-09-03.md) | Measurement record |
| [rust/benchmarks/frame2_last_slot_2026-09-03.md](../benchmarks/frame2_last_slot_2026-09-03.md) | Measurement record |
| [rust/benchmarks/frame2_mechanisms_2026-09-03.md](../benchmarks/frame2_mechanisms_2026-09-03.md) | Measurement record |
| [rust/benchmarks/frame2_mfmv_wiring_2026-09-03.md](../benchmarks/frame2_mfmv_wiring_2026-09-03.md) | Measurement record |
| [rust/benchmarks/frame2_skip_mode_2026-09-03.md](../benchmarks/frame2_skip_mode_2026-09-03.md) | Measurement record |
| [rust/benchmarks/frame2_skip_mode_wired_2026-09-03.md](../benchmarks/frame2_skip_mode_wired_2026-09-03.md) | Measurement record |
| [rust/benchmarks/gate_wallclock_ci_2026-08-27.md](../benchmarks/gate_wallclock_ci_2026-08-27.md) | Measurement record |
| [rust/benchmarks/gate_wallclock_ci_2026-09-05.md](../benchmarks/gate_wallclock_ci_2026-09-05.md) | Measurement record |
| [rust/benchmarks/hadamard_neon_ab_2026-08-13.meta](../benchmarks/hadamard_neon_ab_2026-08-13.meta) | Measurement record |
| [rust/benchmarks/hadscratch_null_2026-09-03.meta](../benchmarks/hadscratch_null_2026-09-03.meta) | Measurement record |
| [rust/benchmarks/ibc_bd10_gb82sc_2026-08-03.meta](../benchmarks/ibc_bd10_gb82sc_2026-08-03.meta) | Measurement record |
| [rust/benchmarks/identity_full_8bit_2026-08-03.meta](../benchmarks/identity_full_8bit_2026-08-03.meta) | Measurement record |
| [rust/benchmarks/identity_full_8bit_dims_p0p5_after_2026-08-04.meta](../benchmarks/identity_full_8bit_dims_p0p5_after_2026-08-04.meta) | Measurement record |
| [rust/benchmarks/identity_full_8bit_dims_p0p5_merged_2026-08-04.meta](../benchmarks/identity_full_8bit_dims_p0p5_merged_2026-08-04.meta) | Measurement record |
| [rust/benchmarks/identity_full_8bit_dims_p1235_2026-08-04.meta](../benchmarks/identity_full_8bit_dims_p1235_2026-08-04.meta) | Measurement record |
| [rust/benchmarks/identity_matrix_132_2026-07-16.meta](../benchmarks/identity_matrix_132_2026-07-16.meta) | Measurement record |
| [rust/benchmarks/identity_matrix_132_full_2026-07-16.meta](../benchmarks/identity_matrix_132_full_2026-07-16.meta) | Measurement record |
| [rust/benchmarks/identity_matrix_2026-07-13.meta](../benchmarks/identity_matrix_2026-07-13.meta) | Measurement record |
| [rust/benchmarks/identity_matrix_2026-07-14.meta](../benchmarks/identity_matrix_2026-07-14.meta) | Measurement record |
| [rust/benchmarks/identity_matrix_allpresets.meta](../benchmarks/identity_matrix_allpresets.meta) | Measurement record |
| [rust/benchmarks/identity_matrix_fix85_synth.meta](../benchmarks/identity_matrix_fix85_synth.meta) | Measurement record |
| [rust/benchmarks/ifs_join_2026-09-04.meta](../benchmarks/ifs_join_2026-09-04.meta) | Measurement record |
| [rust/benchmarks/imazen26_sweep_2026-07-24.meta](../benchmarks/imazen26_sweep_2026-07-24.meta) | Measurement record |
| [rust/benchmarks/inter_byte_matrix_2026-09-02.meta](../benchmarks/inter_byte_matrix_2026-09-02.meta) | Measurement record |
| [rust/benchmarks/inter_byte_matrix_2026-09-02b.meta](../benchmarks/inter_byte_matrix_2026-09-02b.meta) | Measurement record |
| [rust/benchmarks/inter_byte_matrix_2026-09-02c.meta](../benchmarks/inter_byte_matrix_2026-09-02c.meta) | Measurement record |
| [rust/benchmarks/inter_byte_matrix_2026-09-02d.meta](../benchmarks/inter_byte_matrix_2026-09-02d.meta) | Measurement record |
| [rust/benchmarks/inter_byte_matrix_2026-09-02e.meta](../benchmarks/inter_byte_matrix_2026-09-02e.meta) | Measurement record |
| [rust/benchmarks/inter_byte_matrix_2026-09-02f.meta](../benchmarks/inter_byte_matrix_2026-09-02f.meta) | Measurement record |
| [rust/benchmarks/inter_byte_matrix_2026-09-02g.meta](../benchmarks/inter_byte_matrix_2026-09-02g.meta) | Measurement record |
| [rust/benchmarks/inter_byte_matrix_2026-09-03-near.meta](../benchmarks/inter_byte_matrix_2026-09-03-near.meta) | Measurement record |
| [rust/benchmarks/inter_byte_matrix_2026-09-03-sblambda.meta](../benchmarks/inter_byte_matrix_2026-09-03-sblambda.meta) | Measurement record |
| [rust/benchmarks/inter_byte_matrix_2026-09-04-nsqmode.meta](../benchmarks/inter_byte_matrix_2026-09-04-nsqmode.meta) | Measurement record |
| [rust/benchmarks/inter_completion_2026-09-02b.meta](../benchmarks/inter_completion_2026-09-02b.meta) | Measurement record |
| [rust/benchmarks/inter_completion_2026-09-03.meta](../benchmarks/inter_completion_2026-09-03.meta) | Measurement record |
| [rust/benchmarks/inter_edge_shape_mode_2026-09-03.md](../benchmarks/inter_edge_shape_mode_2026-09-03.md) | Measurement record |
| [rust/benchmarks/inter_near_candidate_2026-09-03.md](../benchmarks/inter_near_candidate_2026-09-03.md) | Measurement record |
| [rust/benchmarks/intrabc_has_top_right_vert_a_2026-08-31.meta](../benchmarks/intrabc_has_top_right_vert_a_2026-08-31.meta) | Measurement record |
| [rust/benchmarks/issue16_mds1_txt_cdf_2026-08-27.md](../benchmarks/issue16_mds1_txt_cdf_2026-08-27.md) | Measurement record |
| [rust/benchmarks/kernel_tiers_neon_2026-08-07.md](../benchmarks/kernel_tiers_neon_2026-08-07.md) | Measurement record |
| [rust/benchmarks/kernel_tiers_sad_dedup_2026-09-04.meta](../benchmarks/kernel_tiers_sad_dedup_2026-09-04.meta) | Measurement record |
| [rust/benchmarks/levelscratch_ab_2026-09-03.meta](../benchmarks/levelscratch_ab_2026-09-03.meta) | Measurement record |
| [rust/benchmarks/lossless_gate_2026-08-28.md](../benchmarks/lossless_gate_2026-08-28.md) | Measurement record |
| [rust/benchmarks/lossless_partition_2026-09-07.md](../benchmarks/lossless_partition_2026-09-07.md) | Measurement record |
| [rust/benchmarks/lossless_screen_2026-09-07.md](../benchmarks/lossless_screen_2026-09-07.md) | Measurement record |
| [rust/benchmarks/lr_align_extent_ab_2026-08-06.meta](../benchmarks/lr_align_extent_ab_2026-08-06.meta) | Measurement record |
| [rust/benchmarks/main_merge_2026-09-07.md](../benchmarks/main_merge_2026-09-07.md) | Measurement record |
| [rust/benchmarks/masked_blend_cross_isa_2026-08-31.md](../benchmarks/masked_blend_cross_isa_2026-08-31.md) | Measurement record |
| [rust/benchmarks/mds0_variance_ab_2026-08-13.meta](../benchmarks/mds0_variance_ab_2026-08-13.meta) | Measurement record |
| [rust/benchmarks/mds3d0_null_2026-09-03.meta](../benchmarks/mds3d0_null_2026-09-03.meta) | Measurement record |
| [rust/benchmarks/mds3scratch_ab_2026-09-03.meta](../benchmarks/mds3scratch_ab_2026-09-03.meta) | Measurement record |
| [rust/benchmarks/mdscratch_null_2026-09-03.meta](../benchmarks/mdscratch_null_2026-09-03.meta) | Measurement record |
| [rust/benchmarks/me_dist_ab_2026-09-02.meta](../benchmarks/me_dist_ab_2026-09-02.meta) | Measurement record |
| [rust/benchmarks/me_sad_ab_2026-09-02.meta](../benchmarks/me_sad_ab_2026-09-02.meta) | Measurement record |
| [rust/benchmarks/me_simd_cumulative_2026-09-02.meta](../benchmarks/me_simd_cumulative_2026-09-02.meta) | Measurement record |
| [rust/benchmarks/mem_2026-08-16.meta](../benchmarks/mem_2026-08-16.meta) | Measurement record |
| [rust/benchmarks/mem_2026-09-02.meta](../benchmarks/mem_2026-09-02.meta) | Measurement record |
| [rust/benchmarks/mem_aarch64_2026-09-03.meta](../benchmarks/mem_aarch64_2026-09-03.meta) | Measurement record |
| [rust/benchmarks/mem_arms_2026-09-02.meta](../benchmarks/mem_arms_2026-09-02.meta) | Measurement record |
| [rust/benchmarks/mem_churn_rss_2026-09-03.meta](../benchmarks/mem_churn_rss_2026-09-03.meta) | Measurement record |
| [rust/benchmarks/mem_harness_2026-09-03.meta](../benchmarks/mem_harness_2026-09-03.meta) | Measurement record |
| [rust/benchmarks/mem_heaptrack_2026-09-03.meta](../benchmarks/mem_heaptrack_2026-09-03.meta) | Measurement record |
| [rust/benchmarks/mem_heaptrack_arena_2026-09-03.meta](../benchmarks/mem_heaptrack_arena_2026-09-03.meta) | Measurement record |
| [rust/benchmarks/mem_heaptrack_satd_2026-09-03.meta](../benchmarks/mem_heaptrack_satd_2026-09-03.meta) | Measurement record |
| [rust/benchmarks/mem_inter_axis_2026-09-03b.meta](../benchmarks/mem_inter_axis_2026-09-03b.meta) | Measurement record |
| [rust/benchmarks/mem_levelscratch_2026-09-03.meta](../benchmarks/mem_levelscratch_2026-09-03.meta) | Measurement record |
| [rust/benchmarks/mem_massif_2026-09-03.meta](../benchmarks/mem_massif_2026-09-03.meta) | Measurement record |
| [rust/benchmarks/mem_mecand_2026-09-03.meta](../benchmarks/mem_mecand_2026-09-03.meta) | Measurement record |
| [rust/benchmarks/mem_preset_2026-09-03.meta](../benchmarks/mem_preset_2026-09-03.meta) | Measurement record |
| [rust/benchmarks/mem_refclone_2026-09-04.meta](../benchmarks/mem_refclone_2026-09-04.meta) | Measurement record |
| [rust/benchmarks/native_lossless_2026-09-07.md](../benchmarks/native_lossless_2026-09-07.md) | Measurement record |
| [rust/benchmarks/neighbor_scratch_ab_2026-09-03.meta](../benchmarks/neighbor_scratch_ab_2026-09-03.meta) | Measurement record |
| [rust/benchmarks/neon_tier_audit_2026-08-07.md](../benchmarks/neon_tier_audit_2026-08-07.md) | Measurement record |
| [rust/benchmarks/nic_class_prune_2026-09-03.md](../benchmarks/nic_class_prune_2026-09-03.md) | Measurement record |
| [rust/benchmarks/nmvc_avg_byte_neutrality_2026-08-31.md](../benchmarks/nmvc_avg_byte_neutrality_2026-08-31.md) | Measurement record |
| [rust/benchmarks/nsq_inter_reach_census_2026-09-04.meta](../benchmarks/nsq_inter_reach_census_2026-09-04.meta) | Measurement record |
| [rust/benchmarks/pd0_depth_removal_join_2026-09-02.md](../benchmarks/pd0_depth_removal_join_2026-09-02.md) | Measurement record |
| [rust/benchmarks/percall_layout_2026-09-05.meta](../benchmarks/percall_layout_2026-09-05.meta) | Measurement record |
| [rust/benchmarks/perf_2026-07-20.meta](../benchmarks/perf_2026-07-20.meta) | Measurement record |
| [rust/benchmarks/perf_2026-08-13-hadamard.meta](../benchmarks/perf_2026-08-13-hadamard.meta) | Measurement record |
| [rust/benchmarks/perf_2026-08-13-mds0var.meta](../benchmarks/perf_2026-08-13-mds0var.meta) | Measurement record |
| [rust/benchmarks/perf_2026-08-14-induv.meta](../benchmarks/perf_2026-08-14-induv.meta) | Measurement record |
| [rust/benchmarks/perf_2026-09-02-arm-inter-simd.meta](../benchmarks/perf_2026-09-02-arm-inter-simd.meta) | Measurement record |
| [rust/benchmarks/perf_2026-09-02-arm-inter.meta](../benchmarks/perf_2026-09-02-arm-inter.meta) | Measurement record |
| [rust/benchmarks/perf_2026-09-02-arm-still.meta](../benchmarks/perf_2026-09-02-arm-still.meta) | Measurement record |
| [rust/benchmarks/perf_2026-09-02-arm-videokey-simd.meta](../benchmarks/perf_2026-09-02-arm-videokey-simd.meta) | Measurement record |
| [rust/benchmarks/perf_2026-09-02-arm-videokey.meta](../benchmarks/perf_2026-09-02-arm-videokey.meta) | Measurement record |
| [rust/benchmarks/perf_2026-09-02-control-still.meta](../benchmarks/perf_2026-09-02-control-still.meta) | Measurement record |
| [rust/benchmarks/perf_2026-09-03-arm3-inter.meta](../benchmarks/perf_2026-09-03-arm3-inter.meta) | Measurement record |
| [rust/benchmarks/perf_2026-09-03-arm3-still.meta](../benchmarks/perf_2026-09-03-arm3-still.meta) | Measurement record |
| [rust/benchmarks/perf_2026-09-03-arm3-videokey.meta](../benchmarks/perf_2026-09-03-arm3-videokey.meta) | Measurement record |
| [rust/benchmarks/perf_2026-09-03-arm3b-inter.meta](../benchmarks/perf_2026-09-03-arm3b-inter.meta) | Measurement record |
| [rust/benchmarks/perf_2026-09-03-arm3b-still.meta](../benchmarks/perf_2026-09-03-arm3b-still.meta) | Measurement record |
| [rust/benchmarks/perf_2026-09-03-arm3b-videokey.meta](../benchmarks/perf_2026-09-03-arm3b-videokey.meta) | Measurement record |
| [rust/benchmarks/perf_2026-09-03-arm4-inter.meta](../benchmarks/perf_2026-09-03-arm4-inter.meta) | Measurement record |
| [rust/benchmarks/perf_2026-09-03-arm4-still.meta](../benchmarks/perf_2026-09-03-arm4-still.meta) | Measurement record |
| [rust/benchmarks/perf_2026-09-03-arm4-videokey.meta](../benchmarks/perf_2026-09-03-arm4-videokey.meta) | Measurement record |
| [rust/benchmarks/perf_2026-09-03-arm5-inter.meta](../benchmarks/perf_2026-09-03-arm5-inter.meta) | Measurement record |
| [rust/benchmarks/perf_2026-09-03-arm5-still.meta](../benchmarks/perf_2026-09-03-arm5-still.meta) | Measurement record |
| [rust/benchmarks/perf_2026-09-03-arm5-videokey.meta](../benchmarks/perf_2026-09-03-arm5-videokey.meta) | Measurement record |
| [rust/benchmarks/perf_2026-09-03-arm6-inter.meta](../benchmarks/perf_2026-09-03-arm6-inter.meta) | Measurement record |
| [rust/benchmarks/perf_2026-09-03-arm6-still.meta](../benchmarks/perf_2026-09-03-arm6-still.meta) | Measurement record |
| [rust/benchmarks/perf_2026-09-03-arm6-videokey.meta](../benchmarks/perf_2026-09-03-arm6-videokey.meta) | Measurement record |
| [rust/benchmarks/perf_2026-09-03-arm7-inter.meta](../benchmarks/perf_2026-09-03-arm7-inter.meta) | Measurement record |
| [rust/benchmarks/perf_2026-09-03-arm7-still.meta](../benchmarks/perf_2026-09-03-arm7-still.meta) | Measurement record |
| [rust/benchmarks/perf_2026-09-03-arm7-videokey.meta](../benchmarks/perf_2026-09-03-arm7-videokey.meta) | Measurement record |
| [rust/benchmarks/perf_2026-09-03-arm8-still.meta](../benchmarks/perf_2026-09-03-arm8-still.meta) | Measurement record |
| [rust/benchmarks/perf_2026-09-03-arm9-POSITION.meta](../benchmarks/perf_2026-09-03-arm9-POSITION.meta) | Measurement record |
| [rust/benchmarks/perf_2026-09-03-arm9-inter.meta](../benchmarks/perf_2026-09-03-arm9-inter.meta) | Measurement record |
| [rust/benchmarks/perf_2026-09-03-arm9-still.meta](../benchmarks/perf_2026-09-03-arm9-still.meta) | Measurement record |
| [rust/benchmarks/perf_2026-09-03-arm9-videokey.meta](../benchmarks/perf_2026-09-03-arm9-videokey.meta) | Measurement record |
| [rust/benchmarks/perf_2026-09-03-still512.meta](../benchmarks/perf_2026-09-03-still512.meta) | Measurement record |
| [rust/benchmarks/perf_2026-09-05-arm10-POSITION.meta](../benchmarks/perf_2026-09-05-arm10-POSITION.meta) | Measurement record |
| [rust/benchmarks/perf_2026-09-05-arm10-inter.meta](../benchmarks/perf_2026-09-05-arm10-inter.meta) | Measurement record |
| [rust/benchmarks/perf_2026-09-05-arm10-photo-inter.meta](../benchmarks/perf_2026-09-05-arm10-photo-inter.meta) | Measurement record |
| [rust/benchmarks/perf_2026-09-05-arm10-photo-still.meta](../benchmarks/perf_2026-09-05-arm10-photo-still.meta) | Measurement record |
| [rust/benchmarks/perf_2026-09-05-arm10-photo-videokey.meta](../benchmarks/perf_2026-09-05-arm10-photo-videokey.meta) | Measurement record |
| [rust/benchmarks/perf_2026-09-05-arm10-still.meta](../benchmarks/perf_2026-09-05-arm10-still.meta) | Measurement record |
| [rust/benchmarks/perf_2026-09-05-arm10-videokey.meta](../benchmarks/perf_2026-09-05-arm10-videokey.meta) | Measurement record |
| [rust/benchmarks/perf_2026-09-05-gm-photo-p2-inter.meta](../benchmarks/perf_2026-09-05-gm-photo-p2-inter.meta) | Measurement record |
| [rust/benchmarks/perf_2026-09-05-lilith1-POSITION.meta](../benchmarks/perf_2026-09-05-lilith1-POSITION.meta) | Measurement record |
| [rust/benchmarks/perf_2026-09-05-lilith1-inter.meta](../benchmarks/perf_2026-09-05-lilith1-inter.meta) | Measurement record |
| [rust/benchmarks/perf_2026-09-05-lilith1-p26-inter.meta](../benchmarks/perf_2026-09-05-lilith1-p26-inter.meta) | Measurement record |
| [rust/benchmarks/perf_2026-09-05-lilith1-p26-still.meta](../benchmarks/perf_2026-09-05-lilith1-p26-still.meta) | Measurement record |
| [rust/benchmarks/perf_2026-09-05-lilith1-p26-videokey.meta](../benchmarks/perf_2026-09-05-lilith1-p26-videokey.meta) | Measurement record |
| [rust/benchmarks/perf_2026-09-05-lilith1-photo-inter.meta](../benchmarks/perf_2026-09-05-lilith1-photo-inter.meta) | Measurement record |
| [rust/benchmarks/perf_2026-09-05-lilith1-photo-still.meta](../benchmarks/perf_2026-09-05-lilith1-photo-still.meta) | Measurement record |
| [rust/benchmarks/perf_2026-09-05-lilith1-photo-videokey.meta](../benchmarks/perf_2026-09-05-lilith1-photo-videokey.meta) | Measurement record |
| [rust/benchmarks/perf_2026-09-05-lilith1-still.meta](../benchmarks/perf_2026-09-05-lilith1-still.meta) | Measurement record |
| [rust/benchmarks/perf_2026-09-05-lilith1-videokey.meta](../benchmarks/perf_2026-09-05-lilith1-videokey.meta) | Measurement record |
| [rust/benchmarks/perf_class_attrib_2026-08-13.meta](../benchmarks/perf_class_attrib_2026-08-13.meta) | Measurement record |
| [rust/benchmarks/perf_cs_after.meta](../benchmarks/perf_cs_after.meta) | Measurement record |
| [rust/benchmarks/perf_cs_before.meta](../benchmarks/perf_cs_before.meta) | Measurement record |
| [rust/benchmarks/perf_gap_2026-08-07-postopt.md](../benchmarks/perf_gap_2026-08-07-postopt.md) | Measurement record |
| [rust/benchmarks/perf_gap_2026-08-07-postopt.meta](../benchmarks/perf_gap_2026-08-07-postopt.meta) | Measurement record |
| [rust/benchmarks/perf_gap_2026-08-07.meta](../benchmarks/perf_gap_2026-08-07.meta) | Measurement record |
| [rust/benchmarks/perf_gap_2026-08-07b.meta](../benchmarks/perf_gap_2026-08-07b.meta) | Measurement record |
| [rust/benchmarks/perf_gap_2026-08-11.meta](../benchmarks/perf_gap_2026-08-11.meta) | Measurement record |
| [rust/benchmarks/perf_gap_2026-08-13-final.meta](../benchmarks/perf_gap_2026-08-13-final.meta) | Measurement record |
| [rust/benchmarks/perf_gap_2026-08-13-r1r2.meta](../benchmarks/perf_gap_2026-08-13-r1r2.meta) | Measurement record |
| [rust/benchmarks/perf_gap_2026-08-13.meta](../benchmarks/perf_gap_2026-08-13.meta) | Measurement record |
| [rust/benchmarks/perf_inter_attrib_2026-09-02.meta](../benchmarks/perf_inter_attrib_2026-09-02.meta) | Measurement record |
| [rust/benchmarks/perf_nzmap-after.meta](../benchmarks/perf_nzmap-after.meta) | Measurement record |
| [rust/benchmarks/perf_nzmap-before-master.meta](../benchmarks/perf_nzmap-before-master.meta) | Measurement record |
| [rust/benchmarks/perf_nzmap_ab_2026-07-23.meta](../benchmarks/perf_nzmap_ab_2026-07-23.meta) | Measurement record |
| [rust/benchmarks/perf_p6_4size.meta](../benchmarks/perf_p6_4size.meta) | Measurement record |
| [rust/benchmarks/perf_p6_computestats_simd.meta](../benchmarks/perf_p6_computestats_simd.meta) | Measurement record |
| [rust/benchmarks/perf_postfilter_2026-08-11.meta](../benchmarks/perf_postfilter_2026-08-11.meta) | Measurement record |
| [rust/benchmarks/perf_still_attrib_2026-09-03.meta](../benchmarks/perf_still_attrib_2026-09-03.meta) | Measurement record |
| [rust/benchmarks/perf_videokey_attrib_2026-09-03.meta](../benchmarks/perf_videokey_attrib_2026-09-03.meta) | Measurement record |
| [rust/benchmarks/perf_vs_c_2026-07-13.meta](../benchmarks/perf_vs_c_2026-07-13.meta) | Measurement record |
| [rust/benchmarks/perf_vs_c_2026-07-13_cdef.meta](../benchmarks/perf_vs_c_2026-07-13_cdef.meta) | Measurement record |
| [rust/benchmarks/perf_vs_c_2026-07-13_deblock.meta](../benchmarks/perf_vs_c_2026-07-13_deblock.meta) | Measurement record |
| [rust/benchmarks/perf_vs_c_2026-07-13_qindex.meta](../benchmarks/perf_vs_c_2026-07-13_qindex.meta) | Measurement record |
| [rust/benchmarks/perf_vs_c_2026-07-16.meta](../benchmarks/perf_vs_c_2026-07-16.meta) | Measurement record |
| [rust/benchmarks/photo_p0_bd8_sortfix_2026-07-23.meta](../benchmarks/photo_p0_bd8_sortfix_2026-07-23.meta) | Measurement record |
| [rust/benchmarks/qcoeff_dpb_ab_2026-08-13.meta](../benchmarks/qcoeff_dpb_ab_2026-08-13.meta) | Measurement record |
| [rust/benchmarks/ratemode_r2_ab_2026-08-13.meta](../benchmarks/ratemode_r2_ab_2026-08-13.meta) | Measurement record |
| [rust/benchmarks/rdoq_txclass_ab_2026-09-03.meta](../benchmarks/rdoq_txclass_ab_2026-09-03.meta) | Measurement record |
| [rust/benchmarks/real_image_identity_2026-07-14.meta](../benchmarks/real_image_identity_2026-07-14.meta) | Measurement record |
| [rust/benchmarks/real_image_identity_2026-07-15.meta](../benchmarks/real_image_identity_2026-07-15.meta) | Measurement record |
| [rust/benchmarks/real_image_identity_cflfix_2026-07-15.meta](../benchmarks/real_image_identity_cflfix_2026-07-15.meta) | Measurement record |
| [rust/benchmarks/real_image_identity_fix84_m2m5_2026-07-15.meta](../benchmarks/real_image_identity_fix84_m2m5_2026-07-15.meta) | Measurement record |
| [rust/benchmarks/real_image_identity_fix85_m2m5.meta](../benchmarks/real_image_identity_fix85_m2m5.meta) | Measurement record |
| [rust/benchmarks/real_image_identity_fix85_m6m10.meta](../benchmarks/real_image_identity_fix85_m6m10.meta) | Measurement record |
| [rust/benchmarks/real_image_identity_m2m5_indcfl_2026-07-15.meta](../benchmarks/real_image_identity_m2m5_indcfl_2026-07-15.meta) | Measurement record |
| [rust/benchmarks/real_image_identity_m2m5_pd0tx4_2026-07-15.meta](../benchmarks/real_image_identity_m2m5_pd0tx4_2026-07-15.meta) | Measurement record |
| [rust/benchmarks/real_image_identity_m2m5_sub8cfl_2026-07-15.meta](../benchmarks/real_image_identity_m2m5_sub8cfl_2026-07-15.meta) | Measurement record |
| [rust/benchmarks/real_image_identity_m8cap_2026-07-16.meta](../benchmarks/real_image_identity_m8cap_2026-07-16.meta) | Measurement record |
| [rust/benchmarks/real_image_identity_poolfix_2026-07-16.meta](../benchmarks/real_image_identity_poolfix_2026-07-16.meta) | Measurement record |
| [rust/benchmarks/real_image_identity_postfix_2026-07-16.meta](../benchmarks/real_image_identity_postfix_2026-07-16.meta) | Measurement record |
| [rust/benchmarks/real_image_identity_txs_m10_2026-07-14.meta](../benchmarks/real_image_identity_txs_m10_2026-07-14.meta) | Measurement record |
| [rust/benchmarks/recon_gate_r1_ab_2026-08-13.meta](../benchmarks/recon_gate_r1_ab_2026-08-13.meta) | Measurement record |
| [rust/benchmarks/recon_lazy_refuted_2026-08-07.meta](../benchmarks/recon_lazy_refuted_2026-08-07.meta) | Measurement record |
| [rust/benchmarks/ref_coded_area_stats_2026-09-02.md](../benchmarks/ref_coded_area_stats_2026-09-02.md) | Measurement record |
| [rust/benchmarks/refused_config_triage_2026-09-03.md](../benchmarks/refused_config_triage_2026-09-03.md) | Measurement record |
| [rust/benchmarks/residual_hoist_2026-09-05.meta](../benchmarks/residual_hoist_2026-09-05.meta) | Measurement record |
| [rust/benchmarks/residual_simd_ab_2026-09-03.meta](../benchmarks/residual_simd_ab_2026-09-03.meta) | Measurement record |
| [rust/benchmarks/satdscratch_ab_2026-09-03.meta](../benchmarks/satdscratch_ab_2026-09-03.meta) | Measurement record |
| [rust/benchmarks/screen_controls_2026-09-08.md](../benchmarks/screen_controls_2026-09-08.md) | Measurement record |
| [rust/benchmarks/sse_i32_width_2026-08-11.meta](../benchmarks/sse_i32_width_2026-08-11.meta) | Measurement record |
| [rust/benchmarks/sse_madd_2026-09-05.meta](../benchmarks/sse_madd_2026-09-05.meta) | Measurement record |
| [rust/benchmarks/stall_attrib_2026-09-05.meta](../benchmarks/stall_attrib_2026-09-05.meta) | Measurement record |
| [rust/benchmarks/subpel_simd_ab_2026-09-02.meta](../benchmarks/subpel_simd_ab_2026-09-02.meta) | Measurement record |
| [rust/benchmarks/subpel_stream_ab_2026-09-02.meta](../benchmarks/subpel_stream_ab_2026-09-02.meta) | Measurement record |
| [rust/benchmarks/tile_reserve_ab_2026-09-03.meta](../benchmarks/tile_reserve_ab_2026-09-03.meta) | Measurement record |
| [rust/benchmarks/tune_sharpness_2026-09-08.md](../benchmarks/tune_sharpness_2026-09-08.md) | Measurement record |
| [rust/benchmarks/txout_cshape_2026-09-05.meta](../benchmarks/txout_cshape_2026-09-05.meta) | Measurement record |
| [rust/benchmarks/txscratch_dqcoeff_ab_2026-09-03.meta](../benchmarks/txscratch_dqcoeff_ab_2026-09-03.meta) | Measurement record |
| [rust/benchmarks/txsize_tables_2026-09-05.meta](../benchmarks/txsize_tables_2026-09-05.meta) | Measurement record |
| [rust/benchmarks/unaligned_real_identity_2026-08-11.meta](../benchmarks/unaligned_real_identity_2026-08-11.meta) | Measurement record |
| [rust/benchmarks/unaligned_real_identity_2026-08-13.meta](../benchmarks/unaligned_real_identity_2026-08-13.meta) | Measurement record |
| [rust/benchmarks/unaligned_real_identity_2026-08-14-induv.meta](../benchmarks/unaligned_real_identity_2026-08-14-induv.meta) | Measurement record |
| [rust/benchmarks/unaligned_real_identity_2026-08-14.meta](../benchmarks/unaligned_real_identity_2026-08-14.meta) | Measurement record |
| [rust/benchmarks/video_key_matrix_72x88_2026-09-01-sgr.meta](../benchmarks/video_key_matrix_72x88_2026-09-01-sgr.meta) | Measurement record |
| [rust/benchmarks/video_key_matrix_72x88_2026-09-01-ssse.meta](../benchmarks/video_key_matrix_72x88_2026-09-01-ssse.meta) | Measurement record |
| [rust/benchmarks/video_key_matrix_72x88_2026-09-01.meta](../benchmarks/video_key_matrix_72x88_2026-09-01.meta) | Measurement record |
| [rust/benchmarks/wider_corpus_2026-07-22.meta](../benchmarks/wider_corpus_2026-07-22.meta) | Measurement record |
| [rust/benchmarks/wiener_avx512_tier_2026-09-05.meta](../benchmarks/wiener_avx512_tier_2026-09-05.meta) | Measurement record |
| [rust/benchmarks/wiener_stream_ab_2026-09-02.meta](../benchmarks/wiener_stream_ab_2026-09-02.meta) | Measurement record |
| [rust/benchmarks/z2neon_ab_2026-09-03.meta](../benchmarks/z2neon_ab_2026-09-03.meta) | Measurement record |
| [rust/docs/ACCEPTANCE-CRITERIA.md](ACCEPTANCE-CRITERIA.md) | Historical reference |
| [rust/docs/ANIMATED-AVIF-PLAN.md](ANIMATED-AVIF-PLAN.md) | Current |
| [rust/docs/API-SUPPORT-AUDIT-2026-09-08.md](API-SUPPORT-AUDIT-2026-09-08.md) | Current |
| [rust/docs/C-TEST-PORTING-AUDIT.md](C-TEST-PORTING-AUDIT.md) | Historical reference |
| [rust/docs/C-VS-PORT-CODE-REVIEW-2026-08-13.md](C-VS-PORT-CODE-REVIEW-2026-08-13.md) | Historical reference |
| [rust/docs/DOCUMENTATION-INDEX.md](DOCUMENTATION-INDEX.md) | Current |
| [rust/docs/ENCODER-POLICY-API.md](ENCODER-POLICY-API.md) | Current |
| [rust/docs/HANDOFF-2026-09-08-PARITY.md](HANDOFF-2026-09-08-PARITY.md) | Current |
| [rust/docs/HDR-ON-4.2.md](HDR-ON-4.2.md) | Historical reference |
| [rust/docs/IDENTITY-STATUS.md](IDENTITY-STATUS.md) | Current |
| [rust/docs/INTER-ENCODE-PLAN.md](INTER-ENCODE-PLAN.md) | Historical reference |
| [rust/docs/OPEN-ISSUES-AUDIT-2026-09-07.md](OPEN-ISSUES-AUDIT-2026-09-07.md) | Historical reference |
| [rust/docs/OPEN-ISSUES-AUDIT-2026-09-08.md](OPEN-ISSUES-AUDIT-2026-09-08.md) | Current |
| [rust/docs/PARITY-REFERENCE-AUDIT-2026-09-08.md](PARITY-REFERENCE-AUDIT-2026-09-08.md) | Historical reference |
| [rust/docs/PORT-coeff-writer.md](PORT-coeff-writer.md) | Historical reference |
| [rust/docs/REFUSED-CONFIGS.md](REFUSED-CONFIGS.md) | Generated/shape ledger |
| [rust/docs/STILL-PERF-2026-09-06-history-1.md](STILL-PERF-2026-09-06-history-1.md) | Historical reference |
| [rust/docs/STILL-PERF-2026-09-06-history-2.md](STILL-PERF-2026-09-06-history-2.md) | Historical reference |
| [rust/docs/STILL-PERF-2026-09-06-history-3.md](STILL-PERF-2026-09-06-history-3.md) | Historical reference |
| [rust/docs/STILL-PERF-2026-09-06.md](STILL-PERF-2026-09-06.md) | Historical reference |
| [rust/docs/PARITY-FIDELITY-COSTS.md](PARITY-FIDELITY-COSTS.md) | Current |
| [rust/docs/SUSPECTED-C-BUGS.md](SUSPECTED-C-BUGS.md) | Historical reference |
| [rust/docs/UNWIRED-PORTED-CODE-2026-09-04-dsp-types-cref.md](UNWIRED-PORTED-CODE-2026-09-04-dsp-types-cref.md) | Historical reference |
| [rust/docs/UNWIRED-PORTED-CODE-2026-09-04.md](UNWIRED-PORTED-CODE-2026-09-04.md) | Historical reference |
| [rust/docs/V4.2-AUDIT-AND-HDR-PLAN.md](V4.2-AUDIT-AND-HDR-PLAN.md) | Historical reference |
| [rust/docs/WORKING-ON-THIS.md](WORKING-ON-THIS.md) | Current |
| [rust/docs/arbitrary-dims-port-map.md](arbitrary-dims-port-map.md) | Historical reference |
| [rust/docs/bd10-port-map.md](bd10-port-map.md) | Historical reference |
| [rust/docs/coverage-combos-map.md](coverage-combos-map.md) | Historical reference |
| [rust/docs/entropy-coding-port-map.md](entropy-coding-port-map.md) | Historical reference |
| [rust/docs/film-grain-port-map.md](film-grain-port-map.md) | Historical reference |
| [rust/docs/finishing-survey.md](finishing-survey.md) | Historical reference |
| [rust/docs/hbd-input-port-map.md](hbd-input-port-map.md) | Historical reference |
| [rust/docs/history/2026-09-08/CONTEXT-HANDOFF.md](history/2026-09-08/CONTEXT-HANDOFF.md) | Archived original |
| [rust/docs/history/2026-09-08/ENCODER-POLICY-GOAL.md](history/2026-09-08/ENCODER-POLICY-GOAL.md) | Archived original |
| [rust/docs/history/2026-09-08/README.md](history/2026-09-08/README.md) | Archived original |
| [rust/docs/history/2026-09-08/github/issue-11.md](history/2026-09-08/github/issue-11.md) | Archived original |
| [rust/docs/history/2026-09-08/github/issue-13.md](history/2026-09-08/github/issue-13.md) | Archived original |
| [rust/docs/history/2026-09-08/github/issue-15.md](history/2026-09-08/github/issue-15.md) | Archived original |
| [rust/docs/history/2026-09-08/github/issue-16.md](history/2026-09-08/github/issue-16.md) | Archived original |
| [rust/docs/history/2026-09-08/github/issue-17.md](history/2026-09-08/github/issue-17.md) | Archived original |
| [rust/docs/history/2026-09-08/github/issue-18.md](history/2026-09-08/github/issue-18.md) | Archived original |
| [rust/docs/history/2026-09-08/github/issue-19.md](history/2026-09-08/github/issue-19.md) | Archived original |
| [rust/docs/history/2026-09-08/github/issue-21.md](history/2026-09-08/github/issue-21.md) | Archived original |
| [rust/docs/history/2026-09-08/github/issue-3.md](history/2026-09-08/github/issue-3.md) | Archived original |
| [rust/docs/history/2026-09-08/github/issue-4.md](history/2026-09-08/github/issue-4.md) | Archived original |
| [rust/docs/history/2026-09-08/github/issue-5.md](history/2026-09-08/github/issue-5.md) | Archived original |
| [rust/docs/history/2026-09-08/github/issue-6.md](history/2026-09-08/github/issue-6.md) | Archived original |
| [rust/docs/history/2026-09-08/github/issue-7.md](history/2026-09-08/github/issue-7.md) | Archived original |
| [rust/docs/history/2026-09-08/github/issue-8.md](history/2026-09-08/github/issue-8.md) | Archived original |
| [rust/docs/history/2026-09-08/github/issue-9.md](history/2026-09-08/github/issue-9.md) | Archived original |
| [rust/docs/history/2026-09-08/rust/CLAUDE.md](history/2026-09-08/rust/CLAUDE.md) | Archived original |
| [rust/docs/history/2026-09-08/rust/README.md](history/2026-09-08/rust/README.md) | Archived original |
| [rust/docs/history/2026-09-08/rust/STATUS.md](history/2026-09-08/rust/STATUS.md) | Archived original |
| [rust/docs/history/2026-09-08/rust/docs/ANIMATED-AVIF-PLAN.md](history/2026-09-08/rust/docs/ANIMATED-AVIF-PLAN.md) | Archived original |
| [rust/docs/history/2026-09-08/rust/docs/IDENTITY-STATUS.md](history/2026-09-08/rust/docs/IDENTITY-STATUS.md) | Archived original |
| [rust/docs/history/2026-09-08/rust/docs/OPEN-ISSUES-AUDIT-2026-09-07.md](history/2026-09-08/rust/docs/OPEN-ISSUES-AUDIT-2026-09-07.md) | Archived original |
| [rust/docs/history/2026-09-08/rust/docs/WORKING-ON-THIS.md](history/2026-09-08/rust/docs/WORKING-ON-THIS.md) | Archived original |
| [rust/docs/ibc-port-map.md](ibc-port-map.md) | Historical reference |
| [rust/docs/inter-mvp-port-map.md](inter-mvp-port-map.md) | Historical reference |
| [rust/docs/md-subpel-port-map.md](md-subpel-port-map.md) | Historical reference |
| [rust/docs/nsq-port-map.md](nsq-port-map.md) | Historical reference |
| [rust/docs/palette-port-map.md](palette-port-map.md) | Historical reference |
| [rust/docs/pcl-md-port-map.md](pcl-md-port-map.md) | Historical reference |
| [rust/docs/pd-pcs-resize-lr-coverage.md](pd-pcs-resize-lr-coverage.md) | Historical reference |
| [rust/docs/perf-status.md](perf-status.md) | Historical reference |
| [rust/docs/picstruct-port-map.md](picstruct-port-map.md) | Historical reference |
| [rust/docs/practical-usage-plan.md](practical-usage-plan.md) | Historical reference |
| [rust/docs/quality-program-audit-2026-09-07.md](quality-program-audit-2026-09-07.md) | Historical reference |
| [rust/docs/rate-arm-port-map.md](rate-arm-port-map.md) | Historical reference |
| [rust/docs/research-preset-port-map.md](research-preset-port-map.md) | Historical reference |
| [rust/docs/sb128-port-map.md](sb128-port-map.md) | Historical reference |
| [rust/docs/sc-detection-port-map.md](sc-detection-port-map.md) | Historical reference |
| [rust/docs/superres-port-map.md](superres-port-map.md) | Historical reference |
| [rust/docs/transforms-port-map.md](transforms-port-map.md) | Historical reference |
| [rust/docs/tune-iq-port-map.md](tune-iq-port-map.md) | Historical reference |
| [rust/svtav1/tests/fixtures/reference_chroma/README.md](../svtav1/tests/fixtures/reference_chroma/README.md) | Algorithm/technical reference |
| [rust/tools/perf_profile/PGO.md](../tools/perf_profile/PGO.md) | Algorithm/technical reference |
| [rust/tools/perf_profile/README.md](../tools/perf_profile/README.md) | Algorithm/technical reference |
| [rust/tools/perf_profile/dct64_probe/README.md](../tools/perf_profile/dct64_probe/README.md) | Algorithm/technical reference |
| [rust/tools/perf_profile/dr_zone2_probe/README.md](../tools/perf_profile/dr_zone2_probe/README.md) | Algorithm/technical reference |
| [rust/tools/perf_profile/eob_probe/README.md](../tools/perf_profile/eob_probe/README.md) | Algorithm/technical reference |
| [rust/tools/perf_profile/hadamard_compose_probe/README.md](../tools/perf_profile/hadamard_compose_probe/README.md) | Algorithm/technical reference |
| [rust/tools/perf_profile/hadamard_probe/README.md](../tools/perf_profile/hadamard_probe/README.md) | Algorithm/technical reference |
| [rust/tools/perf_profile/hadamard_transpose_probe/README.md](../tools/perf_profile/hadamard_transpose_probe/README.md) | Algorithm/technical reference |
| [rust/tools/perf_profile/satd_probe/README.md](../tools/perf_profile/satd_probe/README.md) | Algorithm/technical reference |
| [specs/00-architecture.md](../../specs/00-architecture.md) | Algorithm/technical reference |
| [specs/01-api.md](../../specs/01-api.md) | Algorithm/technical reference |
| [specs/02-motion-estimation.md](../../specs/02-motion-estimation.md) | Algorithm/technical reference |
| [specs/03-mode-decision.md](../../specs/03-mode-decision.md) | Algorithm/technical reference |
| [specs/04-transforms.md](../../specs/04-transforms.md) | Algorithm/technical reference |
| [specs/05-intra-prediction.md](../../specs/05-intra-prediction.md) | Algorithm/technical reference |
| [specs/06-inter-prediction.md](../../specs/06-inter-prediction.md) | Algorithm/technical reference |
| [specs/07-entropy-coding.md](../../specs/07-entropy-coding.md) | Algorithm/technical reference |
| [specs/08-loop-filters.md](../../specs/08-loop-filters.md) | Algorithm/technical reference |
| [specs/09-rate-control.md](../../specs/09-rate-control.md) | Algorithm/technical reference |
| [specs/10-encoding-loop.md](../../specs/10-encoding-loop.md) | Algorithm/technical reference |
| [specs/11-picture-management.md](../../specs/11-picture-management.md) | Algorithm/technical reference |
| [specs/12-film-grain.md](../../specs/12-film-grain.md) | Algorithm/technical reference |
| [specs/13-segmentation.md](../../specs/13-segmentation.md) | Algorithm/technical reference |
| [specs/14-utilities.md](../../specs/14-utilities.md) | Algorithm/technical reference |
| [specs/15-rtcd.md](../../specs/15-rtcd.md) | Algorithm/technical reference |
| [specs/16-data-structures.md](../../specs/16-data-structures.md) | Algorithm/technical reference |
| [specs/17-temporal-filtering.md](../../specs/17-temporal-filtering.md) | Algorithm/technical reference |
| [specs/18-testing.md](../../specs/18-testing.md) | Algorithm/technical reference |
| [specs/README.md](../../specs/README.md) | Algorithm/technical reference |
