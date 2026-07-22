# IntraBC (IBC / intra block copy) — port-ready map (IBC vertical scoping, 2026-07-22)

Port-ready map of the SVT-AV1 v4.2.0 intra-block-copy path for the **allintra KEY
bd8 4:2:0 screen-content** envelope. C reference read READ-ONLY at
`/root/svtav1--ibc-scope/Source/Lib/Codec/` (v4.2.0, base `ca96121d7`). This maps
the LARGEST remaining byte-parity gap: screen images (`/root/work/codec-corpus/
gb82-sc/*.png`, sc_class5) use real intrabc DV blocks on every preset 0–4 bd8 cell
(measured: graph 1–170 blocks/cell, gui 10–307). IBC turns OFF at M5+ so p5+ never
need it.

## TL;DR — the situation is NOT "port from scratch"

**A large, plausible, but ENTIRELY UNVERIFIED IBC translation already exists in the
port, and it is uninvoked.** Prior-Claude bulk-ported IBC on 2026-07-17 into
`crates/svtav1-encoder/src/intrabc.rs` (2379 lines) — the search skeleton, DV
validity, ref-DV, rate tables, gates, and the writer are all translated. It is
`pub mod intrabc;` (compiles) but has **ZERO callers** and its ~17 tests are
self-consistency unit tests (hand-computed expected values), **not C-parity
differentials**. So none of it is proven byte-exact against C. A *second*, naive,
non-C-faithful placeholder exists in `crates/svtav1-dsp/src/intrabc.rs` (197 lines,
sum-of-pixels hash, hand-rolled validity) that CONFLICTS with the encoder version.

The work is therefore: **(1) reconcile the two files; (2) add C-parity
differentials to verify the existing translation; (3) fill the genuine stubs (hash
table CRC, ref-mv-stack DRL, RD integration, prediction/block-copy); (4) wire it
into MD injection + pack; (5) end-to-end byte-match a screen cell.** The FH/SH bits
are already landed-dormant and spec-correct.

The port's cited C anchors were spot-checked and are ACCURATE (see §A) — which
raises confidence in the translation's fidelity, but does NOT substitute for
runtime differential verification.

---

## §A. C source inventory (SVT v4.2.0, `Source/Lib/Codec/`)

All line numbers verified against the tree at base `ca96121d7`.

### A.1 Gate + level derivation (allintra)

| What | file:line | signature / value |
|---|---|---|
| allintra sig-deriv (owns the whole IBC gate) | `enc_mode_config.c:2337` | `void svt_aom_sig_deriv_multi_processes_allintra(SequenceControlSet*, PictureParentControlSet*)` |
| intrabc level table | `enc_mode_config.c:2344-2371` | `enable_intrabc && sc_class5`: MR→1, M0→3, M1→4, M2→5, M3→6, M4→`MAX_INTRABC_LEVEL`(=7), M5+→0; else 0 |
| per-level search ctrls | `enc_mode_config.c:1657` | `static void set_intrabc_level(PictureParentControlSet*, uint8_t ibc_level)` — fills `pcs->intrabc_ctrls` (`IntrabcCtrls`, pcs.h:624-640) |
| FH gate | `enc_mode_config.c:2370-2371` | `set_intrabc_level(pcs, intrabc_level); frm_hdr->allow_intrabc = pcs->intrabc_ctrls.enabled;` |
| `allow_screen_content_tools` | `enc_mode_config.c` (sc-detect map §6) | `(palette_level \|\| allow_intrabc) ? 1 : 0` |
| MD-ctx propagation | `enc_mode_config.c:7888, 8005` | `ctx->md_allow_intrabc = pcs->ppcs->frm_hdr.allow_intrabc;` |
| frame-level allow predicate | `entropy_coding.c:4401` | `int svt_aom_allow_intrabc(frm_hdr, slice_type)` = `slice_type==I_SLICE && frm_hdr->allow_screen_content_tools && frm_hdr->allow_intrabc` (decl also rd_cost.c:45, md_rate_estimation.c:623) |

### A.2 Injection chain (candidate generation)

| What | file:line | note |
|---|---|---|
| candidate-gen gate | `mode_decision.c:3597` | `if (ctx->md_allow_intrabc)` |
| `do_intra_bc` sub-gating | `mode_decision.c:3598-3616` | palette_hint/`eval_intrabc`; PART_N + `b4_parent_gating` + sq_size==4 → skip if parent 8×8 `use_intrabc==0`; else `nsq_parent_gating` → skip if parent square `use_intrabc==0` |
| injection | `mode_decision.c:3127` | `static void inject_intra_bc_candidates(pcs, ctx, scs, blk_ptr, &cand_total_cnt)` (called :3617) |
| **DV search entry** | `mode_decision.c:2976` | `static void intra_bc_search(pcs, ctx, scs, blk_ptr, dv_cand, &num_dv_cand)` (called from inject at :3133) |
| ref-mv-stack (DRL) | `mode_decision.c:3019` | `svt_av1_find_best_ref_mvs_from_stack(0, ctx->ref_mv_stack, xd, ref_frame, &nearestmv, &nearmv, 0)` → dv_ref |
| dv_ref composition | `mode_decision.c:3026-3033` | `dv_ref = nearestmv==0 ? nearmv : nearestmv; if(dv_ref==0) svt_aom_find_ref_dv(&dv_ref, tile, sb_mi_size, mi_row, mi_col); assert dv_ref & 7 == 0` |
| search-range setup | `mode_decision.c:3075` | `svt_av1_set_mv_search_range(&x->mv_limits, &dv_ref)`; `mvp_full = dv_ref` (:3081) |
| hash-search call | `mode_decision.c:3092` | `svt_av1_intrabc_hash_search(...)` |

### A.3 DV search internals (`av1me.c` / `hash_motion.c`) — algorithm depth (agent-extracted, line-verified)

**Call graph:** `inject_intra_bc_candidates` (mode_decision.c:3127) → `intra_bc_search`
(:2976, called :3133) → per-direction { `svt_av1_set_mv_search_range` (av1me.c:97) →
`svt_av1_intrabc_hash_search` (av1me.c:1056) → **else** `svt_av1_full_pixel_search`
(av1me.c:1115) → `full_pixel_diamond` (av1me.c:489, static) → `svt_av1_diamond_search_
sad_c` (av1me.c:291) + `svt_av1_refining_search_sad` (av1me.c:424, static) →
`intrabc_full_pixel_exhaustive` (av1me.c:566, static) → `exhaustive_mesh_search`
(av1me.c:212, static) } → `svt_aom_is_dv_valid` (adaptive_mv_pred.c:1908). Frame-level:
`generate_ibc_data` (md_config_process.c:585, called :950).

**Load-bearing facts (each is a byte-exactness constraint):**

1. **Search reads SOURCE pixels, 8-bit forced** — both the frame hash table and the
   pixel search derive from `pcs->ppcs->enhanced_pic` via `svt_aom_link_eb_to_aom_
   buffer_desc_8bit` (pic_buffer_desc.c:510, "Forces an 8 bit version");
   `x->plane[0].src = x->xdplane[0].pre[0]` = the SAME buffer (mode_decision.c:
   3030-3044); hashing passes `use_highbitdepth=0` hardcoded (av1me.c:1071). Never recon.
2. **Two directions, different limits** (mode_decision.c:3046-3080): ABOVE then LEFT.
   ABOVE: `row_max = (sb_row*sb_mi_size - mi_row)*MI_SIZE - h` (stop at current-SB-row
   top). LEFT: `col_max = (sb_col*sb_mi_size - mi_col)*MI_SIZE - w`, `row_max` to
   `min((sb_row+1)*sb_mi_size, tile->mi_row_end)`. Skip direction if box empty.
   `search_dir!=0 → max_dir=IBC_MOTION_LEFT(1)` = only ABOVE runs (L7).
3. **Hash STRICTLY SHORT-CIRCUITS** (mode_decision.c:3086-3117): if
   `best_hash_cost < INT_MAX` the hash DV is taken UNCONDITIONALLY and the pixel search
   NEVER RUNS for that direction. Only a total hash miss falls through to diamond+mesh.
   (NOT a best-of-two — unlike libaom, which runs both. And NOT "hash optional": see §B.2.)
   Up to 2 DV candidates total (`dv_cand[2]`, one per direction).
4. **Hash = CRC-32C (Castagnoli, poly 0x82f63b78 reversed)** (hash.c:15,55-76; RTCD
   sse4.2/ARM-crc32/table variants all bit-identical). Base 2×2 = raw pixel pack
   `p0<<24|p1<<16|p2<<8|p3` (hash_motion.c:78-82, no hashing). Pyramid: each N×N hash
   = CRC-32C over the four (N/2)-hashes as 16 raw bytes (hash_motion.c:192-216).
   Bucket key `hash_value1 = (size_index<<16) | (crc&0xffff)`, `1<<19` buckets;
   `hash_value2` = full CRC verified per candidate (av1me.c:1088).
5. **Bucket iteration order IS the tie-break** (sharpest determinism risk): insertion
   is append-only in a hierarchical coarse-to-fine raster (state machine, hash_motion.c:
   244-303: stride=`block_size` offset (0,0), then (step/2,0), (0,step/2),
   (step/2,step/2), halve, repeat to step==2); candidate compare is strict `<`
   (av1me.c:1108) so FIRST-INSERTED wins ties. Buckets silently truncate at
   `max_cand_per_bucket` (drop later, never replace; hash_motion.c:127-136).
   For `intra=1`, `count <= 1` treated as empty (self-match; av1me.c guard).
   Per-candidate gates applied INDEPENDENTLY: `hash_value2` match + `is_dv_valid`
   (1/8-pel) + `is_mv_in(x->mv_limits)` (full-pel) (av1me.c:1088-1104).
6. **Hash guard:** square blocks only, `bw==bh && bw <= max_block_size_hash`
   (av1me.c:1062-1064). `hash_value_buffer` ping-pong scratch rtime-alloc'd per block
   (mode_decision.c:3014-3016, freed :3122-3124).
7. **Two-metric asymmetry:** WITHIN diamond/mesh stages candidates compare by SAD
   (`fn_ptr->sdf`/`sdx4df` + `mvsad_err_cost`); ACROSS stages (diamond pass vs pass,
   diamond vs mesh, refine adopt, hash candidates) by **variance**
   `svt_av1_get_mvpred_var` (av1me.c:195: `fn_ptr->vf` + `svt_aom_mv_err_cost[_light]`).
   Reusing SAD cross-stage diverges.
8. **Diamond** (`svt_av1_diamond_search_sad_c` av1me.c:291-422): site table
   `pcs->ss_cfg` built ONCE PER FRAME by `svt_av1_init3smotion_compensation`
   (av1me.c:162, md_config_process.c:954; stride baked in) — `ss[0]={0,0}` + 11
   halvings of `len=MAX_FIRST_STEP(1024)` × 8 sites `{(0,±len),(±len,0),(±len,±len)}`
   = 89 sites. `all_in` fast path uses `sdx4df` 4-way batches; boundary path scalar
   `is_mv_in`-checked, same strict-`<` per-site order. `num00` counts no-move steps and
   SKIPS that many subsequent finer passes (full_pixel_diamond :511-537). Refine pass:
   ±1 N/S/E/W, `search_range=8`, only if `do_refine` survives (`n > further_steps`
   kills it). **`NEW_DIAMOND_SEARCH` block (av1me.c:397-416) is DEAD CODE** (never
   defined) — do not port. `x->second_best_mv` written as side effect. Return value of
   `svt_av1_full_pixel_search` is literally always 0 (:1158) — winner is `x->best_mv`.
9. **Mesh** (`intrabc_full_pixel_exhaustive` av1me.c:566-619): trigger =
   `var > (intrabc_ctrls.exhaustive_mesh_thresh >> (10 - (mi_size_wide_log2[bsize] +
   mi_size_high_log2[bsize])))` (:1134-1141), then OVERRIDDEN OFF if
   `max(|Δx|,|Δy|) <= mesh_search_mv_diff_threshold` (:1142-1145). Pattern[0]
   validated `range∈[7,256], interval∈[1,range]` else `return INT_MAX`. Range adapted
   by center magnitude: `range = clamp(max(range, 5*mv_mag/4), .., 256)`, `interval =
   max(interval, range/base_interval_div)` (integer div BEFORE adapt). Refinement:
   patterns[1..3], stop on `range==0` or `interval==1`. `exhaustive_mesh_search`
   (av1me.c:212-289): row-major ascending, `col_step = step>1 ? step : 4` (finest pass
   walks 4-wide `sdx4df` batches + scalar tail), double strict-`<` gate (raw SAD
   early-out, then +cost). Mesh winner adopted over diamond only on strict `<`
   (:1153-1155). Self-referential center: each pass re-centers on the previous pass's
   winner (tie-break divergence propagates).
10. **Per-frame precomputes** (all in `md_config_process.c:946-969`, gated
   `allow_intrabc && max_block_size_hash != 0`): hash table build (`generate_ibc_data`
   :585-617; sizes 4..max, 4×4 skipped if `pic_disallow_4x4` :604), `ss_cfg`, and the
   **mesh QP rescale mutates `mesh_patterns[].range` IN PLACE once per frame**
   (:956-969) — never per block.
11. **Search-time MV cost**: `mvsad_err_cost` full mode =
   `ROUND_POWER_OF_TWO(svt_mv_cost(diff*8)*sad_per_bit, 9)` over `x->nmv_vec_cost`/
   `x->mv_cost_stack` = `ctx->md_rate_est_ctx` tables (ENTROPY-ADAPTED MD-side chain,
   not frame defaults; mode_decision.c:2984-2985); light mode `1296+50*(|Δx|+|Δy|)*8`
   selected by `x->approx_inter_rate` (from `ctx->approx_inter_rate`) — **preset-
   dependent branch, check its value on the M0-M4 allintra path at wiring time**
   (cf. rd_cost.c:1488). `sadperbit16 = svt_aom_get_sad_per_bit(base_q_idx, 0)`
   (:3010); `errorperbit = full_lambda >> 6`, min 1 (:3011-3012) — **verify lambda
   granularity per-block vs per-frame (aom-rs Root 9 shape, §C)**.

| What | file:line | signature |
|---|---|---|
| hash search (public) | `av1me.c:1056` (decl av1me.h:80) | `void svt_av1_intrabc_hash_search(PictureControlSet*, IntraBcContext* x, BlockSize, int x_pos, int y_pos, const Mv* ref_mv, int intra, fn_ptr, int* best_hash_cost, Mv* best_hash_mv)` |
| full-pixel search (diamond+mesh driver) | `av1me.c:1115` | `int svt_av1_full_pixel_search(PictureControlSet*, IntraBcContext*, BlockSize, Mv* mvp_full, int step_param, int error_per_bit, int* cost_list, const Mv* ref_mv)` — always returns 0; winner in `x->best_mv` |
| diamond driver | `av1me.c:489` (static) | `full_pixel_diamond(...)` — passes + num00 skip + refine |
| diamond core | `av1me.c:291` (EXPORTED `svt_av1_diamond_search_sad_c`) | 89-site table, 8 sites/step, 11 steps |
| refine | `av1me.c:424` (static) | `svt_av1_refining_search_sad`, ±1 cross, range 8 |
| mesh driver | `av1me.c:566` (static) | `intrabc_full_pixel_exhaustive(pcs, x, center_mv, sadpb, fn_ptr, ref_mv, &mv_ex)` (called :1151) |
| mesh core | `av1me.c:212` (static) | `exhaustive_mesh_search(...)` row-major raster |
| mvpred var/cost | `av1me.c:195` | `svt_av1_get_mvpred_var(const IntraBcContext*, const Mv* best_mv, const Mv* center_mv, fn_ptr, use_mvcost)` — the CROSS-STAGE metric |
| search-range narrow | `av1me.c:97` | `svt_av1_set_mv_search_range(MvLimits*, const Mv*)` |
| site table init | `av1me.c:162` | `svt_av1_init3smotion_compensation` (per-frame, stride baked) |
| `IntraBcContext` | `coding_unit.h:120-145` | rdmult, xdplane/plane bufs, mv_limits, sadperbit16, errorperbit, best_mv, second_best_mv, xd, nmv_vec_cost, mv_cost_stack, hash_value_buffer[2], approx_inter_rate |
| **hash table type** | `hash_motion.h:27-35` | `BlockHash{int16 x,y; u32 hash_value2}`; `HashTable{Vector** p_lookup_table}` — `1<<19` buckets |
| hash table create | `hash_motion.c:101` | `svt_aom_rtime_alloc_svt_av1_hash_table_create` (idempotent: clears if exists); bucket cap note :127-136 |
| CRC-32C | `hash.c:55` (`svt_av1_get_crc32c_value_c`) + table init `hash.c:28` | Castagnoli 0x82f63b78 reversed; RTCD variants bit-identical |
| block hash query | `hash_motion.c:309` | `svt_av1_get_block_hash_value(y_src, stride, block_size, *hv1, *hv2, use_hbd, IntraBcContext*)` — in-block pyramid via ping-pong buffers |
| generate 2×2 hash (frame) | `hash_motion.c:153` | `svt_av1_generate_block_2x2_hash_value(picture, *pic_block_hash)` — identity pack |
| generate N×N hash (frame) | `hash_motion.c:192` | `svt_av1_generate_block_hash_value(picture, block_size, *src_hash, *dst_hash)` — CRC over 4 sub-hashes |
| add to hash map | `hash_motion.c:218` | `svt_aom_rtime_alloc_svt_av1_add_to_hash_map_by_row_with_precal_data` — hierarchical insertion order (:244-303) |
| hash lookup | `hash_motion.c:140/148` | `svt_av1_hash_table_count` + `svt_av1_hash_get_first_iterator` |
| **frame-level hash build** | `md_config_process.c:585` (static, called :950) | `generate_ibc_data`: create (:595) + per-size hash (:601-603, 4×4 skipped if `pic_disallow_4x4` :604) + add (:605); gated `allow_intrabc && max_block_size_hash!=0` (:946-951) |
| **DV validity** | `adaptive_mv_pred.c:1908-2000` | `int svt_aom_is_dv_valid(const Mv dv, const MacroBlockD* xd, int mi_row, int mi_col, BlockSize bsize, int mib_size_log2)` — subpel reject; 4 tile-edge checks; sub-8×8 chroma-ref margin (ss hardcoded 1,1 — 4:2:0 only, :1945-1961); already-coded-SB64 + `INTRABC_DELAY_SB64=4` + wavefront + 64/128 SW constraint |
| **ref DV default** | `inter_prediction.c:2390-2401` | `svt_aom_find_ref_dv` — first-SB-row → `(x=-4*mib-256, y=0)` else `(x=0, y=-4*mib)`, ×8; mi_col ignored |
| **MVP stack (dv_ref source)** | `adaptive_mv_pred.c:1329/651/450/469/2002/2030` | `svt_aom_generate_av1_mvp_table` → `setup_ref_mv_list` (spatial scan restricted to intrabc neighbours via `is_inter_block` = `use_intrabc \|\| ref_frame[0]>INTRA_FRAME`, block_structures.h:115-121) → `sort_mvp_table` (STABLE descending-weight bubble sort, strict-`<` swap) → `scan_row_col_light` pad → `clamp_mv_ref`; read out by `svt_av1_find_best_ref_mvs_from_stack` |
| QP mesh scaling apply site | `md_config_process.c:956-969` | in-place `mesh_patterns[].range` rescale, ONCE per frame |

### A.4 Rate / cost (`rd_cost.c` / `md_rate_estimation.c`) — depth (agent-extracted, line-verified)

| What | file:line | note |
|---|---|---|
| fast-cost use_intrabc arm | `rd_cost.c:531-545` | if/else halves of ONE fn (`svt_aom_intra_fast_cost` :526-640): intrabc branch = `rate = svt_av1_mv_bit_cost(mv, pred_mv, dv_joint_cost, dvcost, MV_COST_WEIGHT_SUB) + intrabc_fac_bits[1]`; `fast_luma_rate=rate; fast_chroma_rate=0`; early `return RDCOST(lambda, rate, luma_distortion)` — NO chroma estimate, NO intra mode bits |
| every-intra-candidate charge | `rd_cost.c:629-631` | else-branch: on IBC frame every ordinary intra cand pays `intrabc_fac_bits[0]` (asserts use_intrabc==0). uv fast rate (`svt_aom_get_intra_uv_fast_rate` :476-524) structurally unreachable for intrabc (early return at :545) |
| **`intrabc_fac_bits[2]` VALUES** | `md_rate_estimation.h:93`, fill `md_rate_estimation.c:253-255` (only if allow_intrabc) via `svt_aom_get_syntax_rate_from_cdf` (:48-67, EC_MIN_PROB=4) + `av1_cost_symbol` (:33-43) | **`[0] = 51`, `[1] = 1982`** (1/512-bit units; hand-verified from AOM_CDF2(30531)). Port already has `INTRABC_CDF=[2237,0,0]` in default_cdfs.rs:4455 ✓ |
| **DV RD cost** | `svt_av1_mv_bit_cost` (`rd_cost.c:70-71`, call :538-539) | diff clipped to `[MV_LOW,MV_UPP]=[±16384]`; `cost = dv_joint_cost[joint] + dv_cost[0][MV_MAX+dy] + dv_cost[1][MV_MAX+dx]`; `return ROUND_POWER_OF_TWO(cost * weight, 7)` with **`weight = MV_COST_WEIGHT_SUB = 120`** (md_rate_estimation.h:24) — NOT ordinary inter's `MV_COST_WEIGHT = 108` (rd_cost.c:26). Copy-paste trap. |
| **dv cost tables** | `md_rate_estimation.h:67-72`; build `svt_aom_estimate_mv_rate` (`md_rate_estimation.c:458-488`, dv arm :484-486) | `dv_cost[2][MV_VALS]` + `dv_joint_cost[MV_JOINTS]` are SEPARATE storage from `nmv_*`; built from **`fc->ndvc`** with fixed `MV_SUBPEL_NONE` (no fractional/hp cost terms), gated `allow_intrabc`. **HAZARD: the fn early-returns at :459-465 under `approx_inter_rate` BEFORE the dv arm — dv tables left stale under that speed setting.** |
| **refresh cadence** | genesis `md_config_process.c:293-327`; tile-group memcpy `copy_mv_rate` (`enc_dec_process.c:36-56`, copies dv only if allow_intrabc); **per-SB rebuild** `enc_dec_process.c:2866-2919` | NOT a flat per-frame table: per-SB rebuild from the adapted CDF snapshot `pcs->ec_ctx_array[sb_index]` (left×3 / top-right×1 `avg_cdf_symbols`) — costs differ SB-to-SB. Matches the port's existing MD-side ectx chain concept (palette map "TWO independent CDF tracks"). |
| CDF adaptation | `md_rate_estimation.c:854-855` in `svt_aom_update_stats` (called coding_loop.c:1728) | `update_cdf(fc->intrabc_cdf, is_intrabc_block(..), 2)` once per finally-committed block, gated `svt_aom_allow_intrabc` |
| finalization | `rd_cost.c:1800` | `mbmi->block_mi.use_intrabc = cand->block_mi.use_intrabc` |
| search-time MV cost (distinct fn family) | `svt_aom_mv_err_cost` (av1me.c:142) / `mvsad_err_cost` (av1me.c:152) | see A.3 fact 11 — search domain, not the RD-time `svt_av1_mv_bit_cost` |

### A.5 Writer / coding (`entropy_coding.c`) — depth (agent-extracted, line-verified)

**Exact per-block write order for an intrabc block** (`write_modes_b` :4935-5421, I_SLICE
arm :4980-5116): (1) segment_id pre-skip :4983-4986 → (2) **skip flag**
`encode_skip_coeff_av1` :4988 (always) → (3) segment_id post-skip :4990-4993 →
(4) `write_cdef` :4995 (body :3986-4017 — **RUNS for every block but early-returns with
ZERO bits when `coded_lossless || allow_intrabc`** (:3991-3998), also force-zeroing
`cdef_params` as a side effect; frame-wide gate, NOT per-block use_intrabc) →
(5) `av1_write_delta_q_index` :5006 (not intrabc-gated) → (6) **`write_intrabc_info`**
:5021-5023 wrapped in `if (svt_aom_allow_intrabc(..))`: `aom_write_symbol(w,
use_intrabc, ec_ctx->intrabc_cdf, 2)` :4407 → (7) **DV** `svt_av1_encode_dv(w, &mv,
&dv_ref, &ec_ctx->ndvc)` :4412-4414 iff use_intrabc → (8-12) y_mode+angle / uv_mode+
CfL+angle / palette info / filter-intra / palette map tokens ALL SUPPRESSED, each gated
`use_intrabc == 0` (:5024-5089) → (13) **tx_size via the INTER var-tx path**
`code_tx_size` :5090-5100 → `av1_code_tx_size` :4649-4679 → **`write_tx_size_vartx`
:4513-4557 over `txfm_partition_cdf`** (NOT the intra depth `tx_size_cdf`) because
`is_inter_block()` = `use_intrabc || ref_frame[0]>INTRA_FRAME` (block_structures.h:
115-121) → (14) coefficients `av1_encode_coeff_1d` :5101-5115 iff !skip.

| What | file:line | note |
|---|---|---|
| `svt_av1_encode_dv` | `entropy_coding.c:4381-4399` (EXPORTED, no header decl) | asserts dv AND ref are whole-pel; `joint = svt_av1_get_mv_joint(&diff)` (rd_cost.c:47, EXPORTED — a DIFFERENT fn from `svt_av1_encode_mv`'s static `av1_get_mv_joint_diff` :1482-1490); shared static `encode_mv_component` (:1442-1478) with literal `MV_SUBPEL_NONE` → only sign+class+integer bits, never fractional/HP |
| CDF state | `cabac_context_model.h:325-327` | `nmvc` / `ndvc` / `intrabc_cdf` = THREE separate FRAME_CONTEXT fields, independently adapted. `ndvc` default = `default_nmv_context` (cabac_context_model.c:795); `intrabc_cdf` default AOM_CDF2(30531) (:610-612, installed :792) |
| FH `allow_intrabc` bit | `entropy_coding.c:3478` (KEY) / `:3487` (INTRA_ONLY) | gate `allow_screen_content_tools && av1_superres_unscaled`; bit omitted (inferred 0) otherwise |
| FH `delta_lf_present` suppression | `entropy_coding.c:3577-3583` | `if (allow_intrabc) assert(==0) else write_bit` |
| FH `tx_mode` bit | `entropy_coding.c:3608-3612` | gated ONLY on coded_lossless — NO intrabc dependence |
| **Writer port risks** | — | (a) `write_cdef` must execute (and 0-bit-return) for EVERY block on an IBC frame; (b) `svt_aom_get_kf_y_mode_ctx` (:1004-1021) reads neighbour `block_mi.mode` UNCONDITIONALLY — intrabc blocks must stamp `DC_PRED` (the injection value) or the NEXT block's y-mode CDF row desyncs; (c) angle/CfL syntax have no standalone use_intrabc guard — suppressed only by being nested in the skipped mode writers |

### A.6 Prediction + candidate + coeff/residual arm — depth (agent-extracted, line-verified)

**Candidate injection** (`inject_intra_bc_candidates`, mode_decision.c:3127-3162, ≤2
cands): verbatim field list :3139-3159 — `palette_info=NULL; use_intrabc=1;
angle_delta[Y]=angle_delta[UV]=0; uv_mode=UV_DC_PRED; cfl signs/idx=0;
transform_type[0]=transform_type_uv=DCT_DCT; ref_frame={INTRA_FRAME, NONE_FRAME};
mode=DC_PRED; filter_intra_mode=FILTER_INTRA_MODES; motion_mode=SIMPLE_TRANSLATION;
is_interintra_used=0; skip_mode_allowed=false; mv[0]=dv; pred_mv[0]=
ctx->ref_mv_stack[INTRA_FRAME][0].this_mv; drl_index=0;
interp_filters=broadcast(BILINEAR)`. No other field touched. The DV reuses the ordinary
`mv[0]` slot — no separate DV field. Class: **`CAND_CLASS_4`** (dedicated IBC class,
mode_decision.c:3646-3673) → its own NIC budget/pruning lane in the MD funnel.
MVP stack for INTRA_FRAME is built earlier in `md_encode_block`
(product_coding_loop.c:9388, gated `allow_intrabc`) via `svt_aom_generate_av1_mvp_table`
with `ref_frame={INTRA_FRAME}` — same builder as inter MVP. PD0 NEVER injects IBC
(`generate_md_stage_0_cand_pd0` mode_decision.c:3494-3521 has no call — structural, not
a filter).

**Prediction:** dispatch via `product_prediction_fun_table[is_inter_mode(mode) ||
use_intrabc]` = `svt_aom_inter_pu_prediction_av1` (product_coding_loop.c:53-62,
:1270, :6862). Reference substitution at enc_inter_prediction.c:3809-3816:
`if use_intrabc { svt_aom_get_recon_pic(pcs, &ref_pic_list0, hbd_md); ref_pic_list1 =
NULL; }` — **the CURRENT frame's in-progress RECON** (`svt_aom_get_recon_pic` :25-44:
`reference_picture` if this frame is a future ref else the scratch `recon_pic`).
Search=SOURCE (A.3.1) vs predict=RECON — both must be replicated. Scale factors forced
identity (`sf_identity` ternaries at :3296 luma / :3400 chroma — the similar-looking
ternaries at :3077/:3106 inside `inter_chroma_4xn_pred` are DEAD for intrabc, early
return :3047-3049; do not port them). Interp-filter search SKIPPED for intrabc
(:3818-3823); the deep branch is `svt_inter_predictor`/`svt_highbd_inter_predictor`
(inter_prediction.c:1386-1442/:1444-1511): `assert(IMPLIES(is_intrabc, !is_scaled))`;
**full-pel DV → ordinary convolve dispatch (copy); nonzero subpel (= chroma half-pel at
4:2:0 from an odd luma DV) → `convolve_2d_for_intrabc` (:1195-1236, hbd :1238-1252)**
which hardcodes `BILINEAR` filter params AND a **literal subpel value `8`** (exact
half-pel) into the kernel row-select — the real Q4 fraction is used only as a
nonzero/zero gate. Correct only under the luma-integer-DV + 4:2:0 invariant; the
sharpest prediction-side port risk. Generic `clamp_mv_to_umv_border_sb`
(enc_inter_prediction.c:55-75) applies as for any non-scaled inter; NO intrabc-specific
legality check exists at predict time (all legality is injection-time).

**Coeff/residual/TX arm — RESOLVED (was an open question): intrabc reuses the ordinary
INTER path with ZERO special-casing.** In `full_loop_core` (product_coding_loop.c:
6827-7015), `tx_type_search` (:4582-5099), `perform_tx_partitioning` (:5282-5434),
`perform_dct_dct_tx` (:5582-5920): every `use_intrabc` occurrence folds into
`is_inter` at definition; tx-type search runs over the **inter ext-tx set**
(`get_ext_tx_set_type(tx_size, is_inter, reduced_tx_set)`) — the injection-time
DCT_DCT is only the initial value, MDS does re-search. `full_loop.c:2219` same
classification. Chroma: both uv searches SKIP intrabc (`search_best_mds3_uv_mode`
:7335, `search_best_independent_uv_mode` :7463; synthetic uv probes stamp
use_intrabc=0 at :7362/:7568) — uv stays UV_DC_PRED from injection; CfL excluded the
same way inter is (:6932). **Consequence: the port's coeff-arm work = making its
SHARED tx path accept an "inter-classified" block on an I-frame (tx-set row, tx_size
var-tx coding, contexts), not building a parallel IBC tx module.**

### A.7 Filter suppression (CDEF / DLF / LR) — full picture

Header writers: `encode_loopfilter` (:2281-2283), `encode_cdef` (:2345-2347),
`encode_restoration_mode` (:2190-2192) each start `if (allow_intrabc) return;` — zero
bits (spec 5.9.11/5.9.19/5.9.20). Port already mirrors these (obu.rs:1146/1160/1185/
1217). Per-block `write_cdef` ALSO 0-bit-returns (A.5 item 4).

Search/derivation side, per tool (asymmetric — do not treat by analogy):
- **CDEF:** killed at SIGNAL-DERIVATION: `if (!cdef_level || allow_intrabc)
  cdef_search_level = 0` in all 3 preset fns (allintra: enc_mode_config.c:2397-2399)
  → `cdef_ctrls.enabled=0`; `cdef_process.c:692-697` re-zeroes cdef_params.
- **DLF:** killed at SIGNAL-DERIVATION: `dlf_level` stays 0 unless
  `enable_dlf_flag && allow_intrabc==0` (allintra: enc_mode_config.c:10118-10127);
  `dlf_process.c:94-123` then skips BOTH pick and apply — recon never deblocked.
- **LR — DIFFERENT:** NO `allow_intrabc` term at signal-derivation
  (enc_mode_config.c:2450-2470 is purely `enable_restoration`-driven; zero grep hits in
  restoration*.c). Suppression happens at PIPELINE EXECUTION: `rest_process.c:262`
  (search) and `:325` (apply, else-arm forces all 3 planes `RESTORE_NONE`), plus
  `cdef_process.c:705` (`is_lr = enable_restoration && allow_intrabc==0` boundary-line
  prep). **A port that folds allow_intrabc into its enable_restoration equivalent at
  derivation time is WRONG** — downstream logic (e.g. filter-ctrl flags) still sees
  restoration as enabled.

---

## §B. Port state audit (exists vs missing) — precise

### B.1 EXISTS

| Piece | location | state |
|---|---|---|
| **FH/SH bits** | `svtav1-entropy/src/obu.rs` | LANDED DORMANT, spec-correct. `FrameHeaderScreenContent{allow_screen_content_tools, allow_intrabc}` (:843-844); FH bit :1060-1061 (gated on sct); LF/CDEF/LR suppression `if !allow_intrabc` (:1146/1160/1185/1217). Fed from `sc_derivation.allow_intrabc` (pipeline.rs:2405). |
| **`use_intrabc` block field** | `svtav1-types/src/block_mode.rs:68` | field exists; `is_inter()` = `use_intrabc \|\| ref_frame[0]>0` (:110). |
| **sc_detect `intrabc_level`** | `svtav1-encoder/src/sc_detect.rs:368,440` | COMPUTED (M0→3…M4→7). |
| **the gate flag** | `svtav1-encoder/src/sc_detect.rs:452` | `let allow_intrabc = false;` — **hardcoded false. This is the switch to flip.** |
| **encoder IBC translation** | `svtav1-encoder/src/intrabc.rs` (2379 lines) | LARGE, UNINVOKED, UNVERIFIED. See B.3. |
| **dsp IBC placeholder** | `svtav1-dsp/src/intrabc.rs` (197 lines) | NAIVE, non-faithful, CONFLICTS. See B.4. |
| verified MV coding | `svtav1-entropy/src/mv_coding.rs` | `NmvContext`, `encode_mv_diff`, `get_mv_class` — C-parity verified (`tests/c_parity_mv.rs`); `ndvc` reuses this default. |

### B.2 MISSING (the genuine gaps)

1. **Hash table** (CRC-32C block hashing + bucket storage + frame-level
   `generate_ibc_data` build). Only the bucket-*selection* (given a fetched bucket) is
   translated. **CORRECTION to the WIP's §4b claim:** intrabc.rs asserts "skipping hash
   search entirely is always LEGAL (pure speed opt — every DV it would have found, the
   diamond/mesh search can still find)". That is true for BITSTREAM VALIDITY but FALSE
   for BYTE-EXACTNESS on two counts: (a) hash STRICTLY SHORT-CIRCUITS the pixel search
   (A.3 fact 3) — when C's hash hits, C never runs diamond/mesh, so a hash-stubbed port
   runs a different search and generally lands a different DV; (b) the diamond is local
   descent + bounded mesh — it does NOT necessarily find the hash's (arbitrarily far)
   DV at all. On screen content (flat/repeated regions = dense hash hits) C takes the
   hash path constantly. **The hash table is MANDATORY for the byte-match goal**; it can
   only be stubbed during intermediate diamond/mesh-only differential development.
2. **ref-mv-stack DRL** for `INTRA_FRAME`. **The fallback-only mental model is wrong:**
   `dv_ref` is genuinely data-dependent on neighbouring already-decided intrabc blocks'
   DVs via the full MVP machinery — `svt_aom_generate_av1_mvp_table` → `setup_ref_mv_
   list` (adaptive_mv_pred.c:651-971: row(-1)/col(-1)/top-right/top-left/±3,±5 scans;
   `add_ref_mv_candidate` skips non-intrabc intra neighbours via `is_inter_block`, so
   only intrabc neighbours contribute — their own DVs, weight 2*len +REF_CAT_LEVEL=640)
   → `sort_mvp_table` (STABLE bubble sort, descending weight, strict-`<` swap = ties
   keep insertion order) → `scan_row_col_light` pad → `clamp_mv_ref`. `find_ref_dv` is
   only the empty-stack fallback (first blocks / no intrabc neighbour in range). The
   port has NO MVP-stack machinery at all (only the `CandidateMv` type + `MAX_REF_MV_
   STACK_SIZE=8` exist in svtav1-types/motion.rs) — this is a REAL subsystem port. All
   the C fns are EXPORTED (FFI-diffable directly).
3. **RD integration**: the fast/full-cost `use_intrabc` arm (incl. the SB-cadence
   `dv_cost` tables + `intrabc_fac_bits` refresh, A.4), prediction compensation
   (recon-buffer substitution + full-pel copy + chroma half-pel
   `convolve_2d_for_intrabc`, A.6), and the shared-tx-path is_inter classification —
   all documented in intrabc.rs bottom, none wired.
4. **Injection wiring**: `intrabc.rs`'s `do_intra_bc_gate`/`intra_bc_search`/
   `build_intra_bc_candidate` are not called from the MD funnel; no `CAND_CLASS_4`
   lane exists in `leaf_funnel.rs` (the NIC machinery exists; the IBC class does not).
5. **Pack wiring**: `write_intrabc_info` not called from the block-mode-info writer;
   `ndvc` + `intrabc_cdf` not on `FrameContext` as ADAPTING state (the default
   `INTRABC_CDF=[2237,0,0]` IS already in default_cdfs.rs:4455 — the encoder's
   duplicate `INTRABC_DEFAULT_CDF` const should consolidate onto it); no
   `update_cdf(intrabc_cdf)` / ndvc adaptation hooks.
6. **The INTER var-tx tx_size writer** (`write_tx_size_vartx` over
   `txfm_partition_cdf`, entropy_coding.c:4513-4557): an intrabc block codes tx_size
   through the INTER path (A.5 item 13) because `is_inter_block` is true for it. The
   port (allintra) has NO `txfm_partition` machinery at all (zero grep hits) — writer,
   CDF row, contexts, and the MD-side cost for it are all new surface. (For
   TX_MODE_SELECT frames; confirm the allintra tx_mode the port codes and whether the
   skip/largest cases reduce this.)
7. **C-parity verification of EVERYTHING in B.3** — no FFI differentials exist.

### B.3 encoder/intrabc.rs — translated-vs-stubbed inventory (self-documented)

Translated (pure logic, self-tested only):
- §1 `IbcCtrls::for_level` (levels 0,3-7) + `allintra_intrabc_level` + QP mesh scaling
  (`scale_mesh_patterns_by_qp`, `qp_based_th_scaling_factors`).
- §2 `is_dv_valid` (:467) + `is_chroma_reference` (:432).
- §3 `find_ref_dv` (:568) + `resolve_dv_ref` (:595).
- §4 full search skeleton: `set_mv_search_range`, `mvsad_err_cost`, `get_mvpred_var`,
  `init_search_sites`, `diamond_search_sad` (:1058), `refining_search_sad` (:1150),
  `full_pixel_diamond` (:1220), `exhaustive_mesh_search` (:1350),
  `intrabc_full_pixel_exhaustive` (:1430), `full_pixel_search` (:1512), the DV rate
  tables (`build_nmv_cost_table` etc), `intra_bc_search` (:1765) [top entry].
- §4b hash-bucket **selection** (`hash_search_best_in_bucket` :1627) — NOT the table.
- §5 RD-time `mv_bit_cost` (:1897).
- §6 injection gates (`do_intra_bc_gate` :1972, `eval_intrabc_after_palette`,
  `parent_gate_allows_intrabc`) + `build_intra_bc_candidate` (:2023).
- §7 writer `write_intrabc_info` (:2077) + `INTRABC_DEFAULT_CDF` (:2069).

Stubbed / documented-only: hash TABLE (CRC), ref-mv-stack DRL, RD integration (bottom
of file, :2086-2150). `PORT-NOTE(unverified)` markers on: for_level levels 6/7 stale
mesh state (PPCS pool zero-init assumption — fine for single-KEY-frame), MvComponentCost
clamp (one ULP narrower than C's CLIP3), static-fn search primitives lacking exported
symbols, no_std exp.

**Independently spot-verified byte-exact (this scoping pass):** `IbcCtrls::for_level`
matches C `set_intrabc_level` (enc_mode_config.c:1657-1836) **field-for-field for ALL
levels 0-7** — including the subtle C quirk that cases 6 and `MAX_INTRABC_LEVEL`(7)
assign ONLY enabled/palette_hint/nsq/b4/max_block_size_hash=8/max_cand_per_bucket=32/
exhaustive_mesh_thresh=`~0`/search_dir(0 at L6, 1 at L7) and leave
`mesh_patterns`/`mesh_search_mv_diff_threshold`/`mesh_qp_scaling` UNassigned (PPCS-pool
fall-through). The port zero-defaults those (correct for single-KEY-frame). Also verified
byte-exact: `allintra_intrabc_level` (M0→3…M4→7, enc_mode_config.c:2344-2371),
`do_intra_bc_gate` (mode_decision.c:3597-3616), `svt_aom_allow_intrabc` (= I_SLICE &&
sct && allow_intrabc). So the gate + level table need only an FFI/transcription LOCK
(Chunk 2), not re-derivation — the 6/7 stale-state hazard is the only residual (multi-frame only).

### B.4 dsp/intrabc.rs — a CONFLICTING naive placeholder (delete/reconcile)

`is_valid_intrabc_mv` (:18) uses a hand-rolled "src in frame + above current SB row +
no overlap" check — **missing** the tile bounds, sub-8×8 chroma-ref margin, and the
INTRABC_DELAY wavefront that `svt_aom_is_dv_valid` requires. `hash_search_intrabc`
(:77) uses a **sum-of-pixels** hash — not the SVT CRC. `predict_intrabc` (:52) is a
plain block copy (shape right, no clamp). This predates the encoder bulk-port and is a
consolidation trap: the encoder `intrabc.rs` is the real translation; the dsp file's
`is_valid_intrabc_mv`/`hash_search_intrabc` must NOT be used. `predict_intrabc` *could*
be repurposed as the recon-copy primitive once made C-faithful (or superseded by the
inter-dispatch zero-tap convolve).

---

## §C. aom-rs KB-15 reference: the 11-root bug catalog + SVT-vs-libaom differences

aom-rs (`/root/aom-rs`, a libaom port) has a full IBC impl (KB-15). It is **libaom's**
IBC — an APPROACH reference + a trap catalog, NOT copy source. Its own module docs are
stale relative to code (e.g. intrabc_search.rs:14-20 says the coeff arm "is NOT ported"
while the body calls it) — trust function bodies. As of aom-rs origin/main tip
(2026-07-22) there are ELEVEN landed roots + one open pack-side residual. Every root was
found the same way: **byte-inert instrumented sibling-C dumps of per-candidate RD at the
first divergent block** — copy the methodology, not just the bug list.

### C.1 The roots (what / symptom / fix), SVT-relevance annotated

1. **Unclamped tile-bounds sentinel into `is_dv_valid`.** Caller passed raw `1<<16`
   tile-end sentinels; `total_sb64_per_row` exploded 4→4096, the already-coded-SB gate
   stopped rejecting → port accepted DVs C rejects. Fix: clamp to mi_rows/mi_cols at the
   bounds-construction site. **SVT: same formula is spec-shared (verified line-for-line
   vs adaptive_mv_pred.c:1908) — audit every `TileMiBounds` construction in the port.**
2. **`skip_ctx` hardcoded 0.** True invariant on pure-intra KEY; breaks when a skip-arm
   intrabc neighbour has `skip_txfm=1`. Fix: derive from live per-mi grid. **SVT: check
   the port's skip-ctx derivation once IBC blocks can stamp skip.**
3. **Missing `intrabc_cost[0]` on every intra candidate** → all intra leaves ~35 units
   too cheap on IBC frames. **SVT analog is rd_cost.c:629-631 — Chunk 3's exact scope.**
4. **`cfl_store_block` unported.** `is_inter_block` is TRUE for intrabc; a
   non-chroma-ref intrabc block must still publish luma to the CfL buffer for a later
   chroma-ref sibling. Symptom: 10/995 leaves diverged, all UV_CFL_PRED, deltas ×16.
   **SVT: locate SVT's CfL-store path and its is_inter gating before Chunk 7.**
5. **DV search cost measured vs RECON; C uses SOURCE.** Invisible until searched region
   was already coded. **SVT: source-pixels confirmed (A.3 fact 1) — port accordingly.**
6. **`get_tx_size_context` inter-neighbour override unwired** (search + pack + repack).
   Symptom class: identical bits short-term, drifted CDF ROW STATE read later by the
   per-SB cost refresh. **SVT: find SVT's tx-size-ctx neighbour derivation; intrabc
   neighbours qualify as inter.**
7. **Intra-budget early exit skipped the intrabc search.** C sets rate=INT_MAX but does
   NOT return — intrabc still runs with the untightened budget and can rescue the leaf.
   **SVT's funnel differs (candidates injected up front) but the analogous trap is
   fast-cost-stage pruning of IBC candidates — see C.3 axis 1.**
8. **Skip-arm chroma extent `(bw>>ss_x, bh>>ss_y)` instead of padded plane-block.**
   Sub-8×8: unwritten 128-islands corrupted a LATER block's CfL DC-pred → mode flip
   blocks away. Mis-attributed twice before root-caused — first-divergent-byte ≠
   root-cause block.
9. **`error_per_bit` from frame rdmult, not per-SB rdmult** (C recomputes per block;
   per-SB modifier fold changed it 7276→4547). **SVT: `errorperbit = full_lambda>>6`
   from `ctx->full_lambda_md` (mode_decision.c:3011) — verify that lambda's refresh
   granularity matches C's at the injection site.**
10. **Wrong diamond VARIANT: libaom intrabc pins NSTEP_8PT (16 stages, always 8 tangent
    points), not plain NSTEP (15 stages, 12-point tangent stages).** The extra points
    reached a lower-SAD optimum C never visits. **"Diamond search" is not one algorithm
    even within one codebase. SVT's own table (A.3 fact 8: 1024-halving, 8/step, 89
    sites) is DIFFERENT from both libaom variants — port SVT's, byte-for-byte.**
11. **`txfm_partition_cost` frame-constant instead of per-SB adapted** (~47 units low at
    the var-tx split root). **SVT: same class — the MD rate tables (`md_rate_est_ctx`)
    are an adapting chain; verify which snapshot the IBC coeff-arm reads.**
- **Open residual (aom-rs, 2026-07-22):** search + coeff re-encode proven byte-exact at
  the divergent block, yet bits still diverge → PACK-side write-context (dv-diff write,
  flag write, or `write_tx_size_vartx` write-time CDF). Lesson: **"RD decision matches"
  and "bits match" are separate proofs** — plan Chunk 9's gate to test write-time
  contexts, not just symbol values.

### C.2 aom-rs file map (for methodology reference)

`crates/aom-encode/src/intrabc_search.rs` (hash table + diamond/mesh + skip-arm +
entry `rd_pick_intrabc_mode_sb`), `crates/aom-dsp/src/entropy/dv_ref.rs` (`is_dv_valid`
:1369-1448, `find_dv_ref_mvs`, `DvTileBounds`), `crates/aom-encode/src/var_tx.rs` (coeff
arm), `crates/aom-dsp/src/intra/cfl.rs` (`cfl_store_block` :534), `encode_sb.rs`
(skip/coeff-arm re-encode), `partition_pick.rs` (leaf args + Root 1/2/9/11 fix sites),
`pack.rs` (Root 6 pack side), `rd_pick.rs` (Root 7 fix), `mode_costs.rs` (Root 3 fix),
`aom-bench/tests/rd_close_intrabc.rs` (the self-promoting pinned witness gate shape —
decode-census anti-vacuity + asserts-divergence-present, auto-flips to byte-match).

### C.3 SVT-vs-libaom axes (follow SVT source, NEVER aom-rs, on these)

1. **Candidate architecture:** libaom = one `rd_pick_intrabc_mode_sb` after the intra
   search, compared at full RD. SVT = `inject_intra_bc_candidates` puts ≤2 IBC
   candidates into the general MD candidate funnel → staged fast-cost → full-cost.
   **New SVT-specific bug shape: a wrong FAST-cost for an IBC candidate prunes it before
   full RD ever sees it.** Audit the fast-cost model (rd_cost.c:531-541) with the same
   rigor as full cost.
2. **Hash short-circuits in SVT** (hash hit → pixel search never runs); libaom runs
   both and takes the better. Port SVT's either/or, not aom-rs's best-of-two.
   (SVT's own Appendix-Intra-Block-Copy.md describes the order BACKWARDS — "Diamond
   search followed by Hash search" — trust mode_decision.c:3086-3117.)
3. **Diamond site tables differ** (SVT 89-site 3-step-halving vs libaom NSTEP/NSTEP_8PT).
4. **Mesh gating provenance differs:** SVT's `exhaustive_mesh_thresh` is per-LEVEL from
   `set_intrabc_level` (+ QP scaling); libaom uses a fixed screen constant.
5. **Preset knobs `search_dir` / `max_block_size_hash` / `max_cand_per_bucket`** have no
   libaom analog — SVT's `IntrabcCtrls` table (verified §B.3) is authoritative.
6. **`is_dv_valid` IS shared** (spec-normative, verified line-for-line same formula) —
   the port's translation matches SVT; Root 1's input-clamp audit still applies.
   Note SVT hardcodes ss=1,1 in the sub-8×8 chroma clause (4:2:0 only; the real
   `pd->subsampling_x/y` reads are commented out) — flag if 4:4:4/4:2:2 IBC ever needed.
7. **Mv field naming: SVT `.x/.y` vs libaom `.row/.col`** — transposition risk in any
   formula carried across; add an asymmetric-DV round-trip unit test.
8. **Coeff arm routing:** aom-rs hand-ported a parallel var-tx module; SVT routes
   non-skip IBC candidates through its SHARED inter residual/tx path (`full_loop.c:2219`
   is_inter classification). Bug density in aom-rs lived here (roots 2,4,6,8,9,11) —
   in SVT the same class hides in whether the shared path's contexts (skip-ctx,
   tx-size-ctx, CfL store, chroma extent) handle an "inter" block whose reference is
   the current frame.

---

## §D. Decomposition — smallest independently-landable, differentially-verifiable chunks

Reframed around the existing WIP. Ordered by dependency + verifiability. Each chunk:
C fn(s) it ports/verifies, the gate that proves it, byte-inertness surface, size.

> **Byte-inertness invariant (applies to every chunk):** IBC is gated on
> `allow_intrabc = enable_intrabc && sc_class5 && M≤4`. Non-screen frames and M5+ have
> `sc_class5=false` → `palette_level=intrabc_level=0` → `allow_screen_content_tools=0`
> → `svt_aom_allow_intrabc` false → every IBC path unreachable AND the sct/intrabc FH
> bits stay 0. As long as each chunk's changes are behind that gate (or are pure-additive
> dormant code / new tests), the 340+ passing cells stay byte-identical. **Until Chunk 1
> flips the gate, EVERY chunk is provably inert** (the module is uninvoked).
>
> **The one nuance — Chunk 1 (gate flip) on SCREEN cells:** on a `sc_class5` M0-M4 frame
> `allow_screen_content_tools` is ALREADY 1 (palette_level nonzero: sc_detect.rs:458), so
> flipping `allow_intrabc` false→true adds the intrabc FH bit and DROPS the LF/CDEF/LR
> header blocks on those frames. That changes screen-cell output — but **no currently
> PASSING gate regresses**, because the gb82-sc screen cells already diverge today (IBC
> unported → C codes intrabc blocks the port can't), and there is no existing gb82-sc
> byte-match gate in the suite. So the flip is safe w.r.t. the passing set; it just
> changes HOW screen cells diverge (now with the correct FH prefix). Two viable
> orderings: **(i)** flip early (Chunk 1) with a header-prefix / self-promoting screen
> assert, developing the body dormant behind FFI diffs (Chunks 2-9), OR **(ii)** develop
> ALL machinery dormant + FFI-verified first and flip the gate LAST as the activation
> step that should immediately byte-match. (i) matches this codebase's self-promoting
> pinned-gate house style; (ii) keeps the flip strictly to the end. Run
> `cargo test --workspace` after each chunk to confirm non-screen inertness either way.

**Chunk 0 — Reconcile the two intrabc.rs + wire CDF/ndvc state.** (small, no gate flip)
- Delete/quarantine `svtav1-dsp/src/intrabc.rs`'s non-faithful `is_valid_intrabc_mv` +
  `hash_search_intrabc` (or gate them out); make `svtav1-encoder/src/intrabc.rs` the
  single source of truth. Move `INTRABC_DEFAULT_CDF` → `default_cdfs.rs` and add
  `intrabc_cdf: [AomCdfProb;3]` + `ndvc: NmvContext` to `FrameContext`.
- Verify: `cargo build` + full suite unchanged (pure-dormant). No C fn.
- Risk: LOW. Collateral: FrameContext layout.

**Chunk 1 — Flip the gate + FH/SH byte differential.** (small)
- C fn: `svt_aom_sig_deriv_multi_processes_allintra` gate (enc_mode_config.c:2370),
  `svt_aom_allow_intrabc` (entropy_coding.c:4401). Set `sc_detect.rs:452
  allow_intrabc = IbcCtrls::for_level(intrabc_level).enabled`.
- Gate: a `c_parity` differential that on a gb82-sc screen cell the **SH + FH bytes**
  match real SVT C (the FH now emits the allow_intrabc bit + suppresses LF/CDEF/LR).
  Blocks will still diverge — that's fine; assert only the header prefix.
- Byte-inertness: non-screen/M5+ still `false`. **This is the first cell where behavior
  changes** — but only header bytes until injection lands; guard the assert to the
  header. Risk: MEDIUM (first live change). Size: small.

**Chunk 2 — C-parity verify the pure-math translations.** (medium, HIGH value)
- C fns (EXPORTED T-symbols → direct FFI diff): `svt_aom_is_dv_valid`
  (adaptive_mv_pred.c:1908), `svt_aom_find_ref_dv` (inter_prediction.c:2390).
- Also verify (via transcription cross-check or shim): `set_intrabc_level` level table
  (enc_mode_config.c:1657), `svt_aom_get_qp_based_th_scaling_factors`
  (enc_mode_config.c:25), the `build_nmv_cost_table` chain vs `svt_av1_build_nmv_cost_
  table` (md_rate_estimation.c:446), `svt_aom_mv_err_cost` (av1me.c:141).
- Gate: `c_parity_intrabc_validity` / `_refdv` / `_costs` — randomized-input FFI diffs
  vs the exported C fns. **This is where the existing translation gets its first real
  proof.** Expect to FIX bugs here (esp. is_dv_valid tile-bound inputs — see §C).
- Byte-inertness: tests only. Risk: LOW (tests), but may surface translation bugs. Size: medium.

**Chunk 3 — rate terms: `intrabc_fac_bits` + DV cost tables.** (small-medium)
- C fns: `intrabc_fac_bits` fill (md_rate_estimation.c:253-255 via :48-67/:33-43);
  `svt_aom_estimate_mv_rate`'s dv arm (:484-486, `fc->ndvc` + `MV_SUBPEL_NONE`);
  `svt_av1_mv_bit_cost` (rd_cost.c:70, **`MV_COST_WEIGHT_SUB=120`**); the
  per-intra-candidate charge (rd_cost.c:629-631) + the intrabc fast-cost arm shape
  (:531-545).
- Gate: unit diff — **`intrabc_fac_bits = [51, 1982]`** from the default CDF (values
  hand-verified from C); `mv_bit_cost` FFI diff vs a shim (or transcription-lock) across
  randomized DVs incl. the ±16384 clip; a fast-cost unit test that an ordinary intra
  candidate on an IBC frame pays `+[0]` and an IBC candidate pays `mv_rate + [1]` with
  `fast_chroma_rate=0`.
- Cadence: wire the fill into the SAME per-SB refresh path the port's other MD rates
  use (A.4 cadence row); replicate the `approx_inter_rate` early-return ordering hazard
  faithfully. Byte-inertness: fills gated on allow_intrabc. Risk: LOW-MEDIUM. Size:
  small-medium.

**Chunk 4 — Hash table (CRC-32C + frame build + query).** (large — MANDATORY for byte-match)
- C fns: CRC-32C `svt_av1_get_crc32c_value_c` (hash.c:55, table hash.c:28); frame build
  `generate_ibc_data` (md_config_process.c:585) = `svt_av1_generate_block_2x2_hash_value`
  (hash_motion.c:153, identity pack) + `svt_av1_generate_block_hash_value` (:192, CRC of
  4 sub-hashes) + `svt_aom_rtime_alloc_svt_av1_add_to_hash_map_by_row_with_precal_data`
  (:218, **hierarchical insertion order :244-303 — THE tie-break**, bucket cap drop-later
  :127-136); query `svt_av1_get_block_hash_value` (:309, in-block ping-pong pyramid);
  lookup `hash_table_count`/`get_first_iterator` (:140/:148).
- Gate: `c_parity_block_hash` — (a) CRC-32C of random buffers vs exported C fn; (b) a
  golden frame's per-size hash arrays byte-match; (c) bucket CONTENTS AND ORDER match
  (order is load-bearing — first-inserted wins cost ties).
- **NOT optional/deferrable for byte-exactness** (hash short-circuits the pixel search —
  §B.2.1). May be sequenced after Chunk 5's diamond/mesh differentials, but must land
  before Chunk 8 (injection) for any hope of cell byte-match.
- Risk: HIGH (CRC exactness, insertion-order determinism, 8-bit-always, `pic_disallow_
  4x4` gating, bucket truncation). Size: large.

**Chunk 5 — Diamond + mesh + per-direction search driver (the big one).** (large)
- C fns: `intra_bc_search` (mode_decision.c:2976, static — per-direction limits +
  either/or gate), `svt_av1_set_mv_search_range` (av1me.c:97), `svt_av1_full_pixel_
  search` (av1me.c:1115), `full_pixel_diamond` (av1me.c:489), `svt_av1_diamond_search_
  sad_c` (av1me.c:291), `svt_av1_refining_search_sad` (av1me.c:424),
  `intrabc_full_pixel_exhaustive` (av1me.c:566), `exhaustive_mesh_search` (av1me.c:212),
  `svt_av1_get_mvpred_var` (av1me.c:195), `svt_av1_init3smotion_compensation`
  (av1me.c:162), `svt_av1_intrabc_hash_search` (av1me.c:1056).
- Gate: layered — (i) `c_parity_diamond`: the exported `svt_av1_diamond_search_sad_c` +
  `svt_av1_full_pixel_search` vs the port on golden pixel windows (NOTE: these take
  `PictureControlSet*`/`IntraBcContext*` — the differential needs a small C shim in
  `svtav1-cref/shims/` that assembles them from plain arrays, the established pattern);
  (ii) `c_parity_mesh` via a shim around static `intrabc_full_pixel_exhaustive`;
  (iii) `c_parity_intrabc_search`: the exported `svt_av1_intrabc_hash_search` end-to-end
  (needs Chunk 4's C-side table, buildable via the exported create/add fns).
  Anti-vacuity: assert non-trivial DVs + at least one mesh trigger in the fixture set.
- Must-match micro-semantics (from A.3): two-metric asymmetry (SAD in-stage, variance
  cross-stage), num00 skip, strict-`<` everywhere (first-found wins), dead
  NEW_DIAMOND_SEARCH excluded, `col_step=4` finest-mesh batching, per-frame ss_cfg +
  one-shot mesh QP rescale, either/or hash gate, `errorperbit`/`sadperbit16` sources.
- Byte-inertness: dormant until injection. Risk: HIGH. Size: large. Depends: Chunk 2
  (costs), Chunk 4 for layer (iii).

**Chunk 6 — MVP stack for INTRA_FRAME (dv_ref).** (medium-large — a REAL subsystem)
- C fns (ALL exported): `svt_aom_generate_av1_mvp_table` (adaptive_mv_pred.c:1329) →
  `setup_ref_mv_list` (:651-971, intrabc-restricted spatial scans + weights +
  REF_CAT_LEVEL), `sort_mvp_table` (:450, stable bubble), `scan_row_col_light` (:469),
  `clamp_mv_ref` (:963-970), `svt_av1_get_ref_mv_from_stack` (:2002),
  `svt_av1_find_best_ref_mvs_from_stack` (:2030).
- Gate: `c_parity_find_best_ref_mvs` vs the exported C fns on randomized mode-info
  grids with intrabc neighbours (exercise: weights, ties→insertion-order, the light
  rescan pad, clamp).
- Sequencing: CAN land after a first screen-cell attempt (early blocks hit the
  empty-stack `find_ref_dv` fallback) but every block with an intrabc neighbour needs
  it — i.e. required for ANY full-cell byte match beyond the first few blocks. The port
  has zero existing MVP machinery (B.2.2). Only the INTRA_FRAME/intrabc slice of
  `setup_ref_mv_list`'s scan is needed (non-intrabc neighbours are skipped by
  `is_inter_block`), which bounds the port surface. Risk: MEDIUM-HIGH. Size: medium-large.

**Chunk 7 — RD integration (fast/full cost + prediction + shared-tx classification).** (large)
- C fns: fast-cost arm (rd_cost.c:526-640 both halves); prediction chain —
  `svt_aom_inter_pu_prediction_av1` recon substitution (enc_inter_prediction.c:
  3809-3816), `svt_aom_inter_prediction` identity-sf sites (:3296/:3400),
  `svt_inter_predictor` full-pel copy + `convolve_2d_for_intrabc` chroma half-pel
  (inter_prediction.c:1386-1442/:1195-1252); the shared tx path's is_inter
  classification (product_coding_loop.c tx_type_search :4582+ inter ext-tx row,
  perform_tx_partitioning; full_loop.c:2219); uv-search skip sites (:7335/:7463) —
  uv stays UV_DC_PRED.
- Gate: with a fixed injected DV: (i) prediction unit diff — full-pel luma copy +
  chroma half-pel bilinear output matches C (incl. an odd-DV case exercising the
  literal-8 half-pel row-select); (ii) fast/full cost matches for one block; (iii) the
  tx-type search over the inter ext-tx set matches C's winner on golden residuals.
- Risk: HIGH (touches shared RD/prediction/tx machinery — collateral surface; the
  chroma half-pel literal). Size: large. Depends: 3, 5.

**Chunk 8 — Injection wiring into the MD funnel.** (medium)
- C fns: MVP-table build site (product_coding_loop.c:9388, `ref_frame={INTRA_FRAME}`);
  candidate-gen gate + `do_intra_bc` tree (mode_decision.c:3587-3617 — note
  `eval_intrabc` = palette injected >0 cands, checked only when palette geometrically
  eligible; b4 gate reads the PARENT's PART_N winner, nsq gate reads the SAME block's
  PART_N winner); `inject_intra_bc_candidates` (:3127-3162, verbatim fields A.6);
  `CAND_CLASS_4` classification (:3646-3673) + its NIC lane; PD0 exclusion (structural).
- Gate: on a screen cell, the port's stage-0 candidate list at a block equals C's
  (IBC candidates present with C's DV/pred_dv/class); funnel survival matches (the
  fast-cost near-tie pruning risk C.3.1 — assert the IBC candidate reaches MDS3 where
  C's does). Risk: MEDIUM-HIGH (funnel plumbing + class lane). Size: medium.
  Depends: 5, 6, 7.

**Chunk 9 — Pack writer wiring (+ the inter var-tx tx_size writer).** (medium — grew)
- C fns: the A.5 write order — `write_intrabc_info` (:4405) at the :5021 position;
  `svt_av1_encode_dv` (:4381, MV_SUBPEL_NONE, its OWN joint fn); the 5 suppression
  gates (:5024-5089); per-block `write_cdef` 0-bit path (:3991-3998); **`write_tx_size_
  vartx` (:4513-4557) + `txfm_partition_cdf`** (B.2.6 — new machinery for the port);
  `update_cdf(intrabc_cdf)` + ndvc adaptation (md_rate_estimation.c:854-855) on the
  pack/commit side; neighbour stamp `mode=DC_PRED` for the kf-y-mode ctx (A.5 risk b).
- Gate: byte-diff — a hand-built block sequence containing intrabc + intra neighbours
  writes C's exact bytes (flag, DV diff, suppressions, tx_size var-tx, and the NEXT
  intra block's y-mode row unperturbed — the DC_PRED-stamp witness). Verify write-time
  CDF adaptation, not just symbol values (aom-rs open-residual lesson, §C.1).
- Risk: MEDIUM-HIGH. Size: medium. Depends: 0 (ndvc/cdf on FrameContext).

**Chunk 10 — End-to-end byte-match gate on gb82-sc p0-p4.** (the finish)
- Gate: `screen_ibc_gate.sh` — the port byte-matches real SVT C on the gb82-sc
  p0-p4 bd8 cells (graph/gui/imac/imessage/terminal/windows). Promote from
  "assert-divergence-present" (self-promoting) to full byte-identity.
- Risk: this is where residual near-ties (KB-2/KB-13-class) surface. Size: iterative.

---

## §E. Verification / byte-inertness strategy

- **The gate is the safety net:** `allow_intrabc = enable_intrabc && sc_class5 && M≤4`
  (enc_mode_config.c:2344). The 340+ passing cells are non-screen or M5+ → `allow_intrabc
  =false` → `svt_aom_allow_intrabc` false → all IBC code unreachable. Run
  `cargo test --workspace` after each chunk; any non-screen-cell delta is a gate leak.
- **Prefer exported-C FFI differentials** (`c_parity_*`) before the byte gate, per the
  evidence hierarchy. **Export status VERIFIED** (`grep static` per fn):
  - **EXPORTED T-symbols (direct FFI diff):** `svt_aom_is_dv_valid`
    (adaptive_mv_pred.c:1908), `svt_aom_find_ref_dv` (inter_prediction.c:2390),
    `svt_av1_intrabc_hash_search` (av1me.c:1056 — **the whole DV search entry**),
    `svt_av1_full_pixel_search` (av1me.h:74), `svt_av1_diamond_search_sad_c`
    (av1me.c:291), `svt_av1_get_mvpred_var` (av1me.h:78), `svt_av1_get_block_hash_value`
    + `svt_av1_generate_block_hash_value` (hash_motion.c), `svt_av1_find_best_ref_mvs_
    from_stack` (adaptive_mv_pred.c:2030), `svt_aom_mv_err_cost` (av1me.c:141).
  - **static (need a throwaway `ref_shims.c` wrapper — this project's established
    pattern, see palette's six static-only fns):** `intrabc_full_pixel_exhaustive`
    (av1me.c:566), `svt_av1_build_nmv_cost_table` (md_rate_estimation.c:446, static),
    `mvsad_err_cost`, `intra_bc_search` (mode_decision.c:2976).
  - **Consequence:** because `svt_av1_intrabc_hash_search` is exported, Chunk 5 can
    FFI-diff the ENTIRE hash→diamond→mesh search's winning DV directly against C — the
    strongest possible gate — with a shim needed only for the mesh sub-step.
- **Screen corpus:** `/root/work/codec-corpus/gb82-sc/*.png` (graph, gui, imac_*,
  imessage, terminal, windows*). Anti-vacuity: decode-census that C genuinely codes ≥1
  intrabc block per gated cell (the sc-detection dig already measured this).
- **End-state:** `screen_ibc_gate.sh` byte-matches C on gb82-sc p0-p4.

---

## §F. Risks / unknowns

1. **Search determinism (Chunk 5, highest):** the two-metric asymmetry (SAD in-stage vs
   variance cross-stage — A.3.7), strict-`<` first-found-wins ties everywhere, the exact
   89-site visit order + num00 pass-skipping, the mesh trigger
   (`exhaustive_mesh_thresh >> (10-(log2w+log2h))` + the mv-diff override) and its
   range/interval adaptation, `col_step=4` finest-pass batching, per-direction mv-limit
   derivations, the either/or hash gate, and dead `NEW_DIAMOND_SEARCH` (must NOT be
   ported). Any tie divergence propagates (self-referential mesh recentering).
2. **Hash determinism (Chunk 4):** CRC-32C (Castagnoli — NOT CRC-32/IEEE) exactness; the
   hierarchical bucket-insertion order as THE cost-tie tie-break; bucket truncation
   semantics (drop later, never replace); 8-bit-always hashing; `pic_disallow_4x4`;
   count<=1 = empty for intra. Hash is MANDATORY for byte-match (short-circuit — §B.2.1).
3. **DV-clamp / tile-bounds sentinel (aom-rs Root 1):** audit every `TileMiBounds`
   construction feeding `is_dv_valid` is frame-clamped. Pre-empt in Chunk 2 with
   adversarial FFI cases (tile end at/past frame edge).
4. **CfL / palette / shared-inter-path interaction (aom-rs roots 2,4,6,8):** screen
   frames use CfL AND palette AND intrabc. SVT routes the IBC coeff arm through its
   SHARED inter path — the aom-rs bug class maps to: skip-ctx from intrabc neighbours,
   CfL-store for non-chroma-ref intrabc blocks, tx-size-ctx inter-neighbour override,
   sub-8×8 chroma extents. The palette-hint coupling (palette-port-map.md:98-101) is a
   confirmed search-space coupling. Expect interaction near-ties in Chunk 10.
5. **Cost-table freshness (aom-rs roots 9+11 class — now CONFIRMED per-SB in SVT):**
   `dv_cost`/`dv_joint_cost`/`intrabc_fac_bits` rebuild PER SB from the adapted
   neighbour-averaged CDF snapshot (A.4 cadence row) — not per frame. The search binds
   `md_rate_est_ctx` tables + `full_lambda_md`-derived `errorperbit` at the block
   context. Two concrete hazards: `svt_aom_estimate_mv_rate`'s `approx_inter_rate`
   early-return SKIPS the dv-table fill (stale tables — replicate the ordering, and
   check `approx_inter_rate`'s M0-M4 allintra value at wiring); `MV_COST_WEIGHT_SUB=120`
   vs `MV_COST_WEIGHT=108` (RD-time) vs the search-domain `mvsad_err_cost`/
   `svt_aom_mv_err_cost` family — three distinct cost formulas, don't cross-wire.
6. **SVT-specific funnel pruning (C.3.1):** IBC candidates pass the staged fast-cost MD
   funnel — a wrong fast cost prunes them before full RD. Audit rd_cost.c:531-541
   fidelity + the candidate's class/NIC treatment (Agent-B item; verify at Chunk 8).
7. **Pack-side write-context class (aom-rs open residual):** "RD matches" ≠ "bits
   match" — Chunk 9's gate must verify write-time ctx derivation (ndvc state, intrabc
   CDF adaptation, tx-size write ctx), not just symbol values.
8. **MVP stack (Chunk 6):** a real subsystem the port lacks entirely; ties resolved by
   stable-sort insertion order. Required for any full-cell match.
9. **PPCS pool stale mesh state (for_level 6/7):** fine for single-KEY-frame (this
   port's scope); revisit before multi-frame.
10. **RESOLVED — tx-type IS re-searched** over the inter ext-tx set for IBC blocks
   (A.6 coeff arm); injection DCT_DCT is only the initial value. The residual risk moved:
   the port's shared tx path must accept an inter-classified block on an I-frame, and
   tx_size must code via the NEW var-tx writer (B.2.6 / Chunk 9).
11. **Mv field convention:** SVT `.x/.y` vs libaom `.row/.col` — transposition risk when
   consulting aom-rs; add an asymmetric-DV round-trip test (C.3.7).
12. **Chroma half-pel literal `8` (Chunk 7):** `convolve_2d_for_intrabc` hardcodes the
   bilinear row-select to exact half-pel; correct only under the luma-integer-DV +
   4:2:0 invariant. A port threading the real Q4 fraction into a generic bilinear kernel
   must produce identical output — pin with an odd-DV chroma unit diff.
13. **The whole translation is UNVERIFIED:** treat every intrabc.rs value as a hypothesis
   until its `c_parity_*` diff lands (Chunk 2+). Citations + the level table + gates
   spot-check clean (§B.3), but runtime fidelity is unproven. aom-rs lesson: its OWN
   stale doc comments mislead — trust bodies + differentials, not prose.

---

_Branch `ibc/scoping` off `ca96121d7`; scoping pass 2026-07-22 (read-only vs C; the
only artifact of that pass is this doc). Depth extracted via 3 parallel read-only
agents (DV-search internals / writer+rate+coeff arm / aom-rs KB-15 catalog) + direct
spot-verification of the level table, gates, and export statuses._
