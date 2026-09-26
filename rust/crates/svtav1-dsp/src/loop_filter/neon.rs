use super::*;

// =============================================================================
// AArch64 (neon) 6/8/14-tap kernels — transliterations of the AVX2 (v3)
// arms above, which are themselves ports of `ASM_SSE2/dlf_intrin_sse2.c`.
// Same merged-pair vector shapes, same register-level algorithm; every op
// maps to the exact NEON equivalent (`subs_epu8` -> `vqsubq_u8`,
// `unpacklo_*` -> `vzip1q_*`, `srli_si128` -> `vextq` against zero,
// `packs` -> `vqmovn_s16`, `packus` -> `vqmovun_s16`, `andnot` -> `vbicq`,
// `movemask != 0xffff` -> `vminvq_u8 == 0` on the {0,0xFF} mask).
//
// Loads mirror the SSE2 width semantics exactly: 4-byte rows are
// zero-extended to 16 bytes (`_mm_loadu_si32`), 8-byte rows to 16
// (`_mm_loadu_si64`) — the upper lanes feed max/sum folds, so they must be
// zero just as they are on x86.
// =============================================================================

/// `_mm_loadu_si32` equivalent: 4 bytes in lane 0, zeros above.
#[cfg(target_arch = "aarch64")]
#[rite]
pub(super) fn lpf_ld4_neon(_t: NeonToken, r: &[u8; 4]) -> uint8x16_t {
    vreinterpretq_u8_u32(vsetq_lane_u32::<0>(u32::from_le_bytes(*r), vdupq_n_u32(0)))
}

/// `_mm_storeu_si32` equivalent: low 4 bytes.
#[cfg(target_arch = "aarch64")]
#[rite]
pub(super) fn lpf_st4_neon(_t: NeonToken, d: &mut [u8; 4], v: uint8x16_t) {
    d.copy_from_slice(&vgetq_lane_u32::<0>(vreinterpretq_u32_u8(v)).to_le_bytes());
}

/// `_mm_loadu_si64` equivalent: 8 bytes, zeros above.
#[cfg(target_arch = "aarch64")]
#[rite]
pub(super) fn lpf_ld8_neon(_t: NeonToken, r: &[u8; 8]) -> uint8x16_t {
    vcombine_u8(vld1_u8(r), vdup_n_u8(0))
}

/// `_mm_srli_si128::<N>` equivalent: whole-register byte shift right.
#[cfg(target_arch = "aarch64")]
#[rite]
pub(super) fn lpf_bsrl_neon<const N: i32>(_t: NeonToken, v: uint8x16_t) -> uint8x16_t {
    vextq_u8::<N>(v, vdupq_n_u8(0))
}

/// `_mm_slli_si128::<4>` equivalent.
#[cfg(target_arch = "aarch64")]
#[rite]
pub(super) fn lpf_bsll4_neon(_t: NeonToken, v: uint8x16_t) -> uint8x16_t {
    vextq_u8::<12>(vdupq_n_u8(0), v)
}

/// `_mm_unpacklo_epi8(a, b)` on byte vectors.
#[cfg(target_arch = "aarch64")]
#[rite]
pub(super) fn lpf_ziplo8_neon(_t: NeonToken, a: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
    vzip1q_u8(a, b)
}

/// `_mm_unpackhi_epi8(a, b)`.
#[cfg(target_arch = "aarch64")]
#[rite]
pub(super) fn lpf_ziphi8_neon(_t: NeonToken, a: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
    vzip2q_u8(a, b)
}

/// `_mm_unpacklo_epi64` — interleave the low u64 lane of each input.
#[cfg(target_arch = "aarch64")]
#[rite]
pub(super) fn lpf_ziplo64_neon(_t: NeonToken, a: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
    vreinterpretq_u8_u64(vzip1q_u64(vreinterpretq_u64_u8(a), vreinterpretq_u64_u8(b)))
}

/// `_mm_unpackhi_epi64`.
#[cfg(target_arch = "aarch64")]
#[rite]
pub(super) fn lpf_ziphi64_neon(_t: NeonToken, a: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
    vreinterpretq_u8_u64(vzip2q_u64(vreinterpretq_u64_u8(a), vreinterpretq_u64_u8(b)))
}

/// `_mm_unpacklo_epi32`.
#[cfg(target_arch = "aarch64")]
#[rite]
pub(super) fn lpf_ziplo32_neon(_t: NeonToken, a: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
    vreinterpretq_u8_u32(vzip1q_u32(vreinterpretq_u32_u8(a), vreinterpretq_u32_u8(b)))
}

/// `_mm_unpackhi_epi32`.
#[cfg(target_arch = "aarch64")]
#[rite]
pub(super) fn lpf_ziphi32_neon(_t: NeonToken, a: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
    vreinterpretq_u8_u32(vzip2q_u32(vreinterpretq_u32_u8(a), vreinterpretq_u32_u8(b)))
}

/// `_mm_unpacklo_epi8(v, 0)` — widen the low 8 bytes to u16.
#[cfg(target_arch = "aarch64")]
#[rite]
pub(super) fn lpf_widen_neon(_t: NeonToken, v: uint8x16_t) -> uint16x8_t {
    vmovl_u8(vget_low_u8(v))
}

/// `_mm_shuffle_epi32::<0x4e>` — swap the two u64 halves.
#[cfg(target_arch = "aarch64")]
#[rite]
pub(super) fn lpf_swap64_neon(_t: NeonToken, v: uint8x16_t) -> uint8x16_t {
    vextq_u8::<8>(v, v)
}

/// C `abs_diff`: |a - b| per u8 lane. One `vabdq_u8` — exact.
#[cfg(target_arch = "aarch64")]
#[rite]
pub(super) fn lpf_abs_diff_neon(_t: NeonToken, a: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
    vabdq_u8(a, b)
}

/// `_mm_packs_epi16(a, a)` — saturating i16 -> i8 narrow, duplicated.
#[cfg(target_arch = "aarch64")]
#[rite]
pub(super) fn lpf_packs_dup_neon(_t: NeonToken, v: int16x8_t) -> int8x16_t {
    vcombine_s8(vqmovn_s16(v), vqmovn_s16(v))
}

/// `_mm_packus_epi16(a, a)` — saturating i16 -> u8 narrow, duplicated.
#[cfg(target_arch = "aarch64")]
#[rite]
pub(super) fn lpf_packus_dup_neon(_t: NeonToken, v: int16x8_t) -> uint8x16_t {
    vcombine_u8(vqmovun_s16(v), vqmovun_s16(v))
}

/// C `filter4_sse2` (dlf_intrin_sse2.c:176): the narrow filter shared by
/// the 6- and 8-tap kernels. All arithmetic is in the signed domain after
/// the 0x80 flip — the vector values are carried as int8x16_t.
#[cfg(target_arch = "aarch64")]
#[rite]
pub(super) fn filter4_68_neon(
    token: NeonToken,
    p1p0: uint8x16_t,
    q1q0: uint8x16_t,
    hev: uint8x16_t,
    mask: uint8x16_t,
) -> (uint8x16_t, uint8x16_t) {
    // _mm_set_epi8(3 x8, 4 x8) — low lanes 4, high lanes 3.
    let t3t4: &[i8; 16] = &[4, 4, 4, 4, 4, 4, 4, 4, 3, 3, 3, 3, 3, 3, 3, 3];
    let t3t4 = vld1q_s8(t3t4);
    let t80 = vdupq_n_u8(0x80);
    let ff = vdupq_n_s8(-1);

    let ps1ps0_work = vreinterpretq_s8_u8(veorq_u8(p1p0, t80));
    let mut qs1qs0_work = vreinterpretq_s8_u8(veorq_u8(q1q0, t80));

    // filter = signed_char_clamp(ps1 - qs1) & hev
    let work = vqsubq_s8(ps1ps0_work, qs1qs0_work);
    let hev_s = vreinterpretq_s8_u8(hev);
    let mut filter = vandq_s8(
        vreinterpretq_s8_u8(lpf_bsrl_neon::<8>(token, vreinterpretq_u8_s8(work))),
        hev_s,
    );
    // filter = signed_char_clamp(filter + 3 * (qs0 - ps0)) & mask
    filter = vqsubq_s8(filter, work);
    filter = vqsubq_s8(filter, work);
    filter = vqsubq_s8(filter, work);
    filter = vandq_s8(filter, vreinterpretq_s8_u8(mask));
    filter = vreinterpretq_s8_u8(lpf_ziplo64_neon(
        token,
        vreinterpretq_u8_s8(filter),
        vreinterpretq_u8_s8(filter),
    ));

    // filter1 = signed_char_clamp(filter + 4) >> 3 (low 8 bytes);
    // filter2 = signed_char_clamp(filter + 3) >> 3 (high 8 bytes)
    let f21 = vqaddq_s8(filter, t3t4);
    let f2_16 = vshrq_n_s16::<11>(vreinterpretq_s16_s8(vzip2q_s8(f21, f21)));
    let f1_16 = vshrq_n_s16::<11>(vreinterpretq_s16_s8(vzip1q_s8(f21, f21)));
    let f21 = vcombine_s8(vqmovn_s16(f1_16), vqmovn_s16(f2_16)); // [f1 | f2]

    // filter = ROUND_POWER_OF_TWO(filter1, 1) & ~hev
    let mut filter1p1 = vqsubq_s8(f21, ff); // [f1+1 | f2+1]
    filter1p1 = vreinterpretq_s8_s16(vshrq_n_s16::<9>(vreinterpretq_s16_s8(vzip1q_s8(
        filter1p1, filter1p1,
    ))));
    filter1p1 = vcombine_s8(
        vqmovn_s16(vreinterpretq_s16_s8(filter1p1)),
        vqmovn_s16(vreinterpretq_s16_s8(filter1p1)),
    ); // [r1 | r1]
    filter1p1 = vreinterpretq_s8_u8(vbicq_u8(vreinterpretq_u8_s8(filter1p1), hev)); // [r1' | r1']

    // C reuses `hev` as the addend carrier [f2 | r1'].
    let ps_add = vreinterpretq_s8_u8(lpf_ziphi64_neon(
        token,
        vreinterpretq_u8_s8(f21),
        vreinterpretq_u8_s8(filter1p1),
    ));
    let qs_sub = vreinterpretq_s8_u8(lpf_ziplo64_neon(
        token,
        vreinterpretq_u8_s8(f21),
        vreinterpretq_u8_s8(filter1p1),
    )); // [f1 | r1']

    qs1qs0_work = vqsubq_s8(qs1qs0_work, qs_sub);
    let ps1ps0_out = vqaddq_s8(ps1ps0_work, ps_add);

    (
        veorq_u8(vreinterpretq_u8_s8(qs1qs0_work), t80),
        veorq_u8(vreinterpretq_u8_s8(ps1ps0_out), t80),
    )
}

/// Shared mask/hev prologue for the 6/8-tap internals — see
/// [`mask_hev_68_v3`] for the shape; identical ops on NEON.
/// Returns (mask, hev, abs_p1p0).
#[cfg(target_arch = "aarch64")]
#[rite]
#[allow(clippy::too_many_arguments)]
pub(super) fn mask_hev_68_neon(
    token: NeonToken,
    q2p2: uint8x16_t,
    q3p3: uint8x16_t,
    q1p1: uint8x16_t,
    q0p0: uint8x16_t,
    p1q1: uint8x16_t,
    p0q0: uint8x16_t,
    blimit16: uint8x16_t,
    limit: uint8x16_t,
    thresh: uint8x16_t,
) -> (uint8x16_t, uint8x16_t, uint8x16_t) {
    let zero = vdupq_n_u8(0);
    let fe = vdupq_n_u8(0xfe);
    let ff = vdupq_n_u8(0xff);

    let abs_p1p0 = lpf_abs_diff_neon(token, q1p1, q0p0);
    let abs_q1q0 = lpf_bsrl_neon::<8>(token, abs_p1p0);
    let abs_p0q0 = lpf_abs_diff_neon(token, q0p0, p0q0);
    let abs_p1q1 = lpf_abs_diff_neon(token, q1p1, p1q1);

    let flat_a = vmaxq_u8(abs_p1p0, abs_q1q0);
    let mut hev = vqsubq_u8(flat_a, thresh);
    hev = veorq_u8(vceqq_u8(hev, zero), ff);
    hev = lpf_ziplo64_neon(token, hev, hev);

    // The sum>blimit "don't filter" flag in u16 — same reasoning as the
    // v3 arm: _sse2-exact on all reachable inputs (mblim <= 193),
    // _c-exact everywhere.
    let a16 = lpf_widen_neon(token, abs_p0q0);
    let b8 = vshrq_n_u16::<1>(vreinterpretq_u16_u8(vandq_u8(abs_p1q1, fe)));
    let s16 = vaddq_u16(
        vaddq_u16(a16, a16),
        lpf_widen_neon(token, vreinterpretq_u8_u16(b8)),
    );
    let cmp = vcgtq_s16(vreinterpretq_s16_u16(s16), vreinterpretq_s16_u8(blimit16));
    // `_mm_packs_epi16` — SIGNED narrow: 0xFFFF (-1) -> 0xFF, not the
    // unsigned `vqmovun` clamp to 0.
    let flag = vreinterpretq_u8_s8(lpf_packs_dup_neon(token, vreinterpretq_s16_u16(cmp)));

    let work = vmaxq_u8(
        lpf_abs_diff_neon(token, q2p2, q1p1),
        lpf_abs_diff_neon(token, q3p3, q2p2),
    );
    let mut mask = vmaxq_u8(abs_p1p0, work);
    mask = vmaxq_u8(mask, lpf_bsrl_neon::<8>(token, mask));
    mask = vqsubq_u8(mask, limit);
    let mask = vbicq_u8(vceqq_u8(mask, zero), flag);
    let mask = lpf_ziplo64_neon(token, mask, mask);
    (mask, hev, abs_p1p0)
}

/// C `lpf_internal_8_sse2` (dlf_intrin_sse2.c:784). Identical op sequence
/// to [`lpf_internal_8_v3`]; returns (q1q0, p1p0, p2, q2).
#[cfg(target_arch = "aarch64")]
#[rite]
pub(super) fn lpf_internal_8_neon(
    token: NeonToken,
    p: &[uint8x16_t; 4],
    q: &[uint8x16_t; 4],
    blimit16: uint8x16_t,
    limit: uint8x16_t,
    thresh: uint8x16_t,
) -> (uint8x16_t, uint8x16_t, uint8x16_t, uint8x16_t) {
    let zero = vdupq_n_u8(0);
    let one = vdupq_n_u8(1);

    let q3p3 = lpf_ziplo64_neon(token, p[3], q[3]);
    let q2p2 = lpf_ziplo64_neon(token, p[2], q[2]);
    let q1p1 = lpf_ziplo64_neon(token, p[1], q[1]);
    let q0p0 = lpf_ziplo64_neon(token, p[0], q[0]);

    let p1q1 = lpf_swap64_neon(token, q1p1);
    let p0q0 = lpf_swap64_neon(token, q0p0);

    let (mask, hev, abs_p1p0) = mask_hev_68_neon(
        token, q2p2, q3p3, q1p1, q0p0, p1q1, p0q0, blimit16, limit, thresh,
    );

    // flat_mask4
    let mut flat = vmaxq_u8(
        lpf_abs_diff_neon(token, q2p2, q0p0),
        lpf_abs_diff_neon(token, q3p3, q0p0),
    );
    flat = vmaxq_u8(abs_p1p0, flat);
    flat = vmaxq_u8(flat, lpf_bsrl_neon::<8>(token, flat));
    flat = vqsubq_u8(flat, one);
    flat = vceqq_u8(flat, zero);
    flat = vandq_u8(flat, mask);
    flat = lpf_ziplo64_neon(token, flat, flat);

    // filter8 — the 7-tap wide filter (dlf_intrin_sse2.c:852-896)
    let four = vdupq_n_u16(4);
    let p2_16 = lpf_widen_neon(token, p[2]);
    let p1_16 = lpf_widen_neon(token, p[1]);
    let p0_16 = lpf_widen_neon(token, p[0]);
    let q0_16 = lpf_widen_neon(token, q[0]);
    let q1_16 = lpf_widen_neon(token, q[1]);
    let q2_16 = lpf_widen_neon(token, q[2]);
    let p3_16 = lpf_widen_neon(token, p[3]);
    let q3_16 = lpf_widen_neon(token, q[3]);

    // op2
    let mut workp_a = vaddq_u16(vaddq_u16(p3_16, p3_16), vaddq_u16(p2_16, p1_16));
    workp_a = vaddq_u16(vaddq_u16(workp_a, four), p0_16);
    let mut workp_b = vaddq_u16(vaddq_u16(q0_16, p2_16), p3_16);
    let s = vshrq_n_u16::<3>(vaddq_u16(workp_a, workp_b));
    let op2 = lpf_packus_dup_neon(token, vreinterpretq_s16_u16(s));

    // op1
    workp_b = vaddq_u16(vaddq_u16(q0_16, q1_16), p1_16);
    let op1 = vshrq_n_u16::<3>(vaddq_u16(workp_a, workp_b));

    // op0
    workp_a = vaddq_u16(vsubq_u16(workp_a, p3_16), q2_16);
    workp_b = vaddq_u16(vsubq_u16(workp_b, p1_16), p0_16);
    let op0 = vshrq_n_u16::<3>(vaddq_u16(workp_a, workp_b));
    let flat_p1p0 = vcombine_u8(
        vqmovun_s16(vreinterpretq_s16_u16(op0)),
        vqmovun_s16(vreinterpretq_s16_u16(op1)),
    ); // [op0 | op1]

    // oq0
    workp_a = vaddq_u16(vsubq_u16(workp_a, p3_16), q3_16);
    workp_b = vaddq_u16(vsubq_u16(workp_b, p0_16), q0_16);
    let oq0 = vshrq_n_u16::<3>(vaddq_u16(workp_a, workp_b));

    // oq1
    workp_a = vaddq_u16(vsubq_u16(workp_a, p2_16), q3_16);
    workp_b = vaddq_u16(vsubq_u16(workp_b, q0_16), q1_16);
    let oq1 = vshrq_n_u16::<3>(vaddq_u16(workp_a, workp_b));
    let flat_q0q1 = vcombine_u8(
        vqmovun_s16(vreinterpretq_s16_u16(oq0)),
        vqmovun_s16(vreinterpretq_s16_u16(oq1)),
    ); // [oq0 | oq1]

    // oq2
    workp_a = vaddq_u16(vsubq_u16(workp_a, p1_16), q3_16);
    workp_b = vaddq_u16(vsubq_u16(workp_b, q1_16), q2_16);
    let s = vshrq_n_u16::<3>(vaddq_u16(workp_a, workp_b));
    let oq2 = lpf_packus_dup_neon(token, vreinterpretq_s16_u16(s));

    // lp filter
    let p1p0 = lpf_ziplo64_neon(token, q0p0, q1p1);
    let q1q0 = lpf_ziphi64_neon(token, q0p0, q1p1);
    let (qs1qs0, ps1ps0) = filter4_68_neon(token, p1p0, q1q0, hev, mask);

    let q1q0_out = vorrq_u8(vbicq_u8(qs1qs0, flat), vandq_u8(flat, flat_q0q1));
    let p1p0_out = vorrq_u8(vbicq_u8(ps1ps0, flat), vandq_u8(flat, flat_p1p0));
    let q2_out = vorrq_u8(vbicq_u8(q[2], flat), vandq_u8(flat, oq2));
    let p2_out = vorrq_u8(vbicq_u8(p[2], flat), vandq_u8(flat, op2));
    (q1q0_out, p1p0_out, p2_out, q2_out)
}

/// C `lpf_internal_6_sse2` (dlf_intrin_sse2.c:632). Returns (q1q0, p1p0).
#[cfg(target_arch = "aarch64")]
#[rite]
pub(super) fn lpf_internal_6_neon(
    token: NeonToken,
    p: &[uint8x16_t; 3],
    q: &[uint8x16_t; 3],
    blimit16: uint8x16_t,
    limit: uint8x16_t,
    thresh: uint8x16_t,
) -> (uint8x16_t, uint8x16_t) {
    let zero = vdupq_n_u8(0);
    let one = vdupq_n_u8(1);

    let q2p2 = lpf_ziplo64_neon(token, p[2], q[2]);
    let q1p1 = lpf_ziplo64_neon(token, p[1], q[1]);
    let q0p0 = lpf_ziplo64_neon(token, p[0], q[0]);

    let p1q1 = lpf_swap64_neon(token, q1p1);
    let p0q0 = lpf_swap64_neon(token, q0p0);

    let (mask, hev, abs_p1p0) = mask_hev_68_neon(
        token, q2p2, q2p2, q1p1, q0p0, p1q1, p0q0, blimit16, limit, thresh,
    );

    // flat_mask (flat_mask3)
    let mut flat = vmaxq_u8(lpf_abs_diff_neon(token, q2p2, q0p0), abs_p1p0);
    flat = vmaxq_u8(flat, lpf_bsrl_neon::<8>(token, flat));
    flat = vqsubq_u8(flat, one);
    flat = vceqq_u8(flat, zero);
    flat = vandq_u8(flat, mask);
    flat = lpf_ziplo64_neon(token, flat, flat);

    // 5-tap filter (dlf_intrin_sse2.c:706-742)
    let four = vdupq_n_u16(4);
    let p2_16 = lpf_widen_neon(token, p[2]);
    let p1_16 = lpf_widen_neon(token, p[1]);
    let p0_16 = lpf_widen_neon(token, p[0]);
    let q0_16 = lpf_widen_neon(token, q[0]);
    let q1_16 = lpf_widen_neon(token, q[1]);
    let q2_16 = lpf_widen_neon(token, q[2]);

    // op1
    let mut workp_a = vaddq_u16(vaddq_u16(p0_16, p0_16), vaddq_u16(p1_16, p1_16));
    workp_a = vaddq_u16(vaddq_u16(workp_a, four), p2_16);
    let mut workp_b = vaddq_u16(vaddq_u16(p2_16, p2_16), q0_16);
    let op1 = vshrq_n_u16::<3>(vaddq_u16(workp_a, workp_b));

    // op0
    workp_b = vaddq_u16(vaddq_u16(q0_16, q0_16), q1_16);
    workp_a = vaddq_u16(workp_a, workp_b);
    let op0 = vshrq_n_u16::<3>(workp_a);
    let flat_p1p0 = vcombine_u8(
        vqmovun_s16(vreinterpretq_s16_u16(op0)),
        vqmovun_s16(vreinterpretq_s16_u16(op1)),
    ); // [op0 | op1]

    // oq0
    workp_a = vsubq_u16(vsubq_u16(workp_a, p2_16), p1_16);
    workp_b = vaddq_u16(q1_16, q2_16);
    workp_a = vaddq_u16(workp_a, workp_b);
    let oq0 = vshrq_n_u16::<3>(workp_a);

    // oq1
    workp_a = vsubq_u16(vsubq_u16(workp_a, p1_16), p0_16);
    workp_b = vaddq_u16(q2_16, q2_16);
    let oq1 = vshrq_n_u16::<3>(vaddq_u16(workp_a, workp_b));
    let flat_q0q1 = vcombine_u8(
        vqmovun_s16(vreinterpretq_s16_u16(oq0)),
        vqmovun_s16(vreinterpretq_s16_u16(oq1)),
    ); // [oq0 | oq1]

    // lp filter
    let p1p0 = lpf_ziplo64_neon(token, q0p0, q1p1);
    let q1q0 = lpf_ziphi64_neon(token, q0p0, q1p1);
    let (qs1qs0, ps1ps0) = filter4_68_neon(token, p1p0, q1q0, hev, mask);

    let q1q0_out = vorrq_u8(vbicq_u8(qs1qs0, flat), vandq_u8(flat, flat_q0q1));
    let p1p0_out = vorrq_u8(vbicq_u8(ps1ps0, flat), vandq_u8(flat, flat_p1p0));
    (q1q0_out, p1p0_out)
}

/// C `svt_aom_lpf_horizontal_6_sse2` — 4 columns, 4-byte row loads.
#[cfg(target_arch = "aarch64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
pub(super) fn lpf_horizontal_6_impl_neon(
    token: NeonToken,
    buf: &mut [u8],
    off: usize,
    pitch: usize,
    t: LfThresh,
) {
    let blimit16 = vreinterpretq_u8_u16(vdupq_n_u16(t.mblim as u16));
    let limit = vdupq_n_u8(t.lim);
    let thresh = vdupq_n_u8(t.hev_thr);

    let row = |k: isize| -> uint8x16_t {
        let i = (off as isize + k * pitch as isize) as usize;
        let r: &[u8; 4] = buf[i..i + 4].try_into().unwrap();
        lpf_ld4_neon(token, r)
    };
    let p = [row(-1), row(-2), row(-3)];
    let q = [row(0), row(1), row(2)];
    let (q1q0, p1p0) = lpf_internal_6_neon(token, &p, &q, blimit16, limit, thresh);

    let st = |buf: &mut [u8], k: isize, v: uint8x16_t| {
        let i = (off as isize + k * pitch as isize) as usize;
        let d: &mut [u8; 4] = (&mut buf[i..i + 4]).try_into().unwrap();
        lpf_st4_neon(token, d, v);
    };
    st(buf, -1, p1p0);
    st(buf, -2, lpf_bsrl_neon::<8>(token, p1p0));
    st(buf, 0, q1q0);
    st(buf, 1, lpf_bsrl_neon::<8>(token, q1q0));
}

/// C `svt_aom_lpf_horizontal_8_sse2` — 4 columns, 4-byte row loads.
#[cfg(target_arch = "aarch64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
pub(super) fn lpf_horizontal_8_impl_neon(
    token: NeonToken,
    buf: &mut [u8],
    off: usize,
    pitch: usize,
    t: LfThresh,
) {
    let blimit16 = vreinterpretq_u8_u16(vdupq_n_u16(t.mblim as u16));
    let limit = vdupq_n_u8(t.lim);
    let thresh = vdupq_n_u8(t.hev_thr);

    let row = |k: isize| -> uint8x16_t {
        let i = (off as isize + k * pitch as isize) as usize;
        let r: &[u8; 4] = buf[i..i + 4].try_into().unwrap();
        lpf_ld4_neon(token, r)
    };
    let p = [row(-1), row(-2), row(-3), row(-4)];
    let q = [row(0), row(1), row(2), row(3)];
    let (q1q0, p1p0, p2, q2) = lpf_internal_8_neon(token, &p, &q, blimit16, limit, thresh);

    let st = |buf: &mut [u8], k: isize, v: uint8x16_t| {
        let i = (off as isize + k * pitch as isize) as usize;
        let d: &mut [u8; 4] = (&mut buf[i..i + 4]).try_into().unwrap();
        lpf_st4_neon(token, d, v);
    };
    st(buf, -1, p1p0);
    st(buf, -2, lpf_bsrl_neon::<8>(token, p1p0));
    st(buf, 0, q1q0);
    st(buf, 1, lpf_bsrl_neon::<8>(token, q1q0));
    st(buf, -3, p2);
    st(buf, 2, q2);
}

/// C `transpose6x6_sse2` (dlf_intrin_sse2.c:962): six rows in (rows 4-5
/// are zero on the forward call), three column-pair vectors out. Doubles
/// as the inverse, same as the v3 arm.
#[cfg(target_arch = "aarch64")]
#[rite]
pub(super) fn transpose6x6_neon(token: NeonToken, x: &[uint8x16_t; 6]) -> [uint8x16_t; 3] {
    let w0 = lpf_ziplo8_neon(token, x[0], x[1]);
    let w1 = lpf_ziplo8_neon(token, x[2], x[3]);
    let w2 = lpf_ziplo8_neon(token, x[4], x[5]);

    let z16 = |a: uint8x16_t, b: uint8x16_t, hi: bool| -> uint8x16_t {
        let (a16, b16) = (vreinterpretq_u16_u8(a), vreinterpretq_u16_u8(b));
        vreinterpretq_u8_u16(if hi {
            vzip2q_u16(a16, b16)
        } else {
            vzip1q_u16(a16, b16)
        })
    };
    let w4 = z16(w0, w1, false);
    let w5 = z16(w2, w0, false);
    let d0d1 = lpf_ziplo32_neon(token, w4, w5);
    let d2d3 = lpf_ziphi32_neon(token, w4, w5);

    let w4 = z16(w0, w1, true);
    let w5 = z16(w2, x[3], true);
    let d4d5 = lpf_ziplo32_neon(token, w4, w5);
    [d0d1, d2d3, d4d5]
}

/// C `transpose8x8_sse2` (dlf_intrin_sse2.c:993): eight rows in (rows
/// 4-7 are zero on the forward call), four column-pair vectors out.
/// Doubles as the inverse.
#[cfg(target_arch = "aarch64")]
#[rite]
pub(super) fn transpose8x8_neon(token: NeonToken, x: &[uint8x16_t; 8]) -> [uint8x16_t; 4] {
    let w0 = lpf_ziplo8_neon(token, x[0], x[1]);
    let w1 = lpf_ziplo8_neon(token, x[2], x[3]);
    let w2 = lpf_ziplo8_neon(token, x[4], x[5]);
    let w3 = lpf_ziplo8_neon(token, x[6], x[7]);

    let z16 = |a: uint8x16_t, b: uint8x16_t, hi: bool| -> uint8x16_t {
        let (a16, b16) = (vreinterpretq_u16_u8(a), vreinterpretq_u16_u8(b));
        vreinterpretq_u8_u16(if hi {
            vzip2q_u16(a16, b16)
        } else {
            vzip1q_u16(a16, b16)
        })
    };
    let w4 = z16(w0, w1, false);
    let w5 = z16(w2, w3, false);
    let d0d1 = lpf_ziplo32_neon(token, w4, w5);
    let d2d3 = lpf_ziphi32_neon(token, w4, w5);

    let w6 = z16(w0, w1, true);
    let w7 = z16(w2, w3, true);
    let d4d5 = lpf_ziplo32_neon(token, w6, w7);
    let d6d7 = lpf_ziphi32_neon(token, w6, w7);
    [d0d1, d2d3, d4d5, d6d7]
}

/// C `svt_aom_lpf_vertical_6_sse2` — 4 rows. Loads 8-byte rows at s-3
/// (the transpose consumes the low 6 bytes), transposes, filters,
/// transposes back, and stores 6 bytes per row.
#[cfg(target_arch = "aarch64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
pub(super) fn lpf_vertical_6_impl_neon(
    token: NeonToken,
    buf: &mut [u8],
    off: usize,
    pitch: usize,
    t: LfThresh,
) {
    let blimit16 = vreinterpretq_u8_u16(vdupq_n_u16(t.mblim as u16));
    let limit = vdupq_n_u8(t.lim);
    let thresh = vdupq_n_u8(t.hev_thr);

    let zero = vdupq_n_u8(0);
    let row = |k: usize| -> uint8x16_t {
        // Same 8-byte-load-of-6-used as the v3 arm: see `load8_zero_padded`.
        let r = load8_zero_padded(buf, off - 3 + k * pitch);
        lpf_ld8_neon(token, &r)
    };
    let x = [row(0), row(1), row(2), row(3), zero, zero];
    let [d0d1, d2d3, d4d5] = transpose6x6_neon(token, &x);
    let d1 = lpf_bsrl_neon::<8>(token, d0d1);
    let d3 = lpf_bsrl_neon::<8>(token, d2d3);
    let d5 = lpf_bsrl_neon::<8>(token, d4d5);

    // C: lpf_internal_6(&d0d1, &d5, &d1, &d4d5, &d2d3, &d3, ...)
    let p = [d2d3, d1, d0d1];
    let q = [d3, d4d5, d5];
    let (q1q0, p1p0) = lpf_internal_6_neon(token, &p, &q, blimit16, limit, thresh);

    let p0 = lpf_bsrl_neon::<8>(token, p1p0);
    let q0 = lpf_bsrl_neon::<8>(token, q1q0);
    // C: transpose6x6(&d0d1, &p0, &p1p0, &q1q0, &q0, &d5, ...)
    let xi = [d0d1, p0, p1p0, q1q0, q0, d5];
    let [o0, o2, _o4] = transpose6x6_neon(token, &xi);

    // C stores 6 bytes per row via a 16-byte temp + memcpy.
    let mut tmp = [0u8; 8];
    for (r, v) in [
        o0,
        lpf_bsrl_neon::<8>(token, o0),
        o2,
        lpf_bsrl_neon::<8>(token, o2),
    ]
    .iter()
    .enumerate()
    {
        vst1_u8(&mut tmp, vget_low_u8(*v));
        let b = off - 3 + r * pitch;
        buf[b..b + 6].copy_from_slice(&tmp[0..6]);
    }
}

/// C `svt_aom_lpf_vertical_8_sse2` — 4 rows. Loads 8-byte rows at s-4,
/// transposes, filters, transposes back, stores 8 bytes per row.
#[cfg(target_arch = "aarch64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
pub(super) fn lpf_vertical_8_impl_neon(
    token: NeonToken,
    buf: &mut [u8],
    off: usize,
    pitch: usize,
    t: LfThresh,
) {
    let blimit16 = vreinterpretq_u8_u16(vdupq_n_u16(t.mblim as u16));
    let limit = vdupq_n_u8(t.lim);
    let thresh = vdupq_n_u8(t.hev_thr);

    let zero = vdupq_n_u8(0);
    let row = |k: usize| -> uint8x16_t {
        let i = off - 4 + k * pitch;
        let r: &[u8; 8] = buf[i..i + 8].try_into().unwrap();
        lpf_ld8_neon(token, r)
    };
    let x = [row(0), row(1), row(2), row(3), zero, zero, zero, zero];
    let [d0d1, d2d3, d4d5, d6d7] = transpose8x8_neon(token, &x);
    let d1 = lpf_bsrl_neon::<8>(token, d0d1);
    let d3 = lpf_bsrl_neon::<8>(token, d2d3);
    let d5 = lpf_bsrl_neon::<8>(token, d4d5);
    let d7 = lpf_bsrl_neon::<8>(token, d6d7);

    // C: lpf_internal_8(&d0d1, &d7, &d1, &d6d7, &d2d3, &d5, &d3, &d4d5, ...)
    let p = [d3, d2d3, d1, d0d1];
    let q = [d4d5, d5, d6d7, d7];
    let (q1q0, p1p0, p2, q2) = lpf_internal_8_neon(token, &p, &q, blimit16, limit, thresh);

    let p0 = lpf_bsrl_neon::<8>(token, p1p0);
    let q0 = lpf_bsrl_neon::<8>(token, q1q0);
    // C: transpose8x8(&d0d1, &p2, &p0, &p1p0, &q1q0, &q0, &q2, &d7, ...)
    let xi = [d0d1, p2, p0, p1p0, q1q0, q0, q2, d7];
    let [o0, o2, _o4, _o6] = transpose8x8_neon(token, &xi);

    let st = |buf: &mut [u8], k: usize, v: uint8x16_t| {
        let i = off - 4 + k * pitch;
        let d: &mut [u8; 8] = (&mut buf[i..i + 8]).try_into().unwrap();
        vst1_u8(d, vget_low_u8(v));
    };
    st(buf, 0, o0);
    st(buf, 1, lpf_bsrl_neon::<8>(token, o0));
    st(buf, 2, o2);
    st(buf, 3, lpf_bsrl_neon::<8>(token, o2));
}

// -----------------------------------------------------------------------------
// AArch64 14-tap kernels — same transliteration of the v3 arms.
// -----------------------------------------------------------------------------

/// C `filter4_14_sse2` (dlf_intrin_sse2.c:227): the narrow-filter half of
/// the 14-tap kernel. Returns the (qs1qs0, ps1ps0) merged output pairs.
#[cfg(target_arch = "aarch64")]
#[rite]
pub(super) fn filter4_14_neon(
    token: NeonToken,
    p1p0: uint8x16_t,
    q1q0: uint8x16_t,
    hev: uint8x16_t,
    mask: uint8x16_t,
) -> (uint8x16_t, uint8x16_t) {
    // _mm_set_epi8(0 x8, 3 x4, 4 x4) — low lanes 4 x4, then 3 x4, then 0 x8.
    let t3t4: &[i8; 16] = &[4, 4, 4, 4, 3, 3, 3, 3, 0, 0, 0, 0, 0, 0, 0, 0];
    let t3t4 = vld1q_s8(t3t4);
    let t80 = vdupq_n_u8(0x80);
    let ff = vdupq_n_s8(-1);

    let ps1ps0_work = vreinterpretq_s8_u8(veorq_u8(p1p0, t80));
    let mut qs1qs0_work = vreinterpretq_s8_u8(veorq_u8(q1q0, t80));

    // filter = signed_char_clamp(ps1 - qs1) & hev
    let work = vqsubq_s8(ps1ps0_work, qs1qs0_work);
    let mut filter = vandq_s8(
        vreinterpretq_s8_u8(lpf_bsrl_neon::<4>(token, vreinterpretq_u8_s8(work))),
        vreinterpretq_s8_u8(hev),
    );
    // filter = signed_char_clamp(filter + 3 * (qs0 - ps0)) & mask
    filter = vqsubq_s8(filter, work);
    filter = vqsubq_s8(filter, work);
    filter = vqsubq_s8(filter, work);
    filter = vandq_s8(filter, vreinterpretq_s8_u8(mask));
    filter = vreinterpretq_s8_u8(lpf_ziplo32_neon(
        token,
        vreinterpretq_u8_s8(filter),
        vreinterpretq_u8_s8(filter),
    ));

    // filter1 = signed_char_clamp(filter + 4) >> 3;
    // filter2 = signed_char_clamp(filter + 3) >> 3
    let mut f21 = vqaddq_s8(filter, t3t4);
    f21 = vreinterpretq_s8_u8(vzip1q_u8(
        vreinterpretq_u8_s8(f21),
        vreinterpretq_u8_s8(f21),
    ));
    f21 = vreinterpretq_s8_s16(vshrq_n_s16::<11>(vreinterpretq_s16_s8(f21)));
    f21 = lpf_packs_dup_neon(token, vreinterpretq_s16_s8(f21));

    // filter = ROUND_POWER_OF_TWO(filter1, 1) & ~hev
    filter = vqsubq_s8(f21, ff);
    filter = vreinterpretq_s8_u8(vzip1q_u8(
        vreinterpretq_u8_s8(filter),
        vreinterpretq_u8_s8(filter),
    ));
    filter = vreinterpretq_s8_s16(vshrq_n_s16::<9>(vreinterpretq_s16_s8(filter)));
    filter = lpf_packs_dup_neon(token, vreinterpretq_s16_s8(filter));
    filter = vreinterpretq_s8_u8(vbicq_u8(vreinterpretq_u8_s8(filter), hev));
    filter = vreinterpretq_s8_u8(lpf_ziplo32_neon(
        token,
        vreinterpretq_u8_s8(filter),
        vreinterpretq_u8_s8(filter),
    ));

    let f21m = lpf_ziplo32_neon(token, vreinterpretq_u8_s8(f21), vreinterpretq_u8_s8(filter));
    let hev1 = lpf_bsrl_neon::<8>(token, f21m);
    // signed_char_clamp(qs1 - filter), signed_char_clamp(qs0 - filter1)
    qs1qs0_work = vqsubq_s8(qs1qs0_work, vreinterpretq_s8_u8(f21m));
    // signed_char_clamp(ps1 + filter), signed_char_clamp(ps0 + filter2)
    let ps1ps0_out = vqaddq_s8(ps1ps0_work, vreinterpretq_s8_u8(hev1));

    (
        veorq_u8(vreinterpretq_u8_s8(qs1qs0_work), t80),
        veorq_u8(vreinterpretq_u8_s8(ps1ps0_out), t80),
    )
}

/// C `lpf_internal_14_sse2` (dlf_intrin_sse2.c:357). `qp[i]` is the merged
/// `q{i}p{i}` pair vector; `qp[0..=5]` are written back, `qp[6]` is
/// read-only. Identical op sequence to [`lpf_internal_14_v3`].
#[cfg(target_arch = "aarch64")]
#[rite]
#[allow(clippy::too_many_arguments)]
pub(super) fn lpf_internal_14_neon(
    token: NeonToken,
    qp: &mut [uint8x16_t; 7],
    blimit16: uint8x16_t,
    limit: uint8x16_t,
    thresh: uint8x16_t,
) {
    let zero = vdupq_n_u8(0);
    let one = vdupq_n_u8(1);
    let fe = vdupq_n_u8(0xfe);
    let ff = vdupq_n_u8(0xff);

    let p1p0 = lpf_ziplo32_neon(token, qp[0], qp[1]);
    let q1q0 = lpf_bsrl_neon::<8>(token, p1p0);

    // filter_mask + hev_mask (dlf_intrin_sse2.c:371-404)
    let abs_p1p0 = lpf_abs_diff_neon(token, qp[1], qp[0]);
    let abs_q1q0 = lpf_bsrl_neon::<4>(token, abs_p1p0);
    let abs_p0q0 = lpf_abs_diff_neon(token, p1p0, q1q0);
    let mut abs_p1q1 = lpf_bsrl_neon::<4>(token, abs_p0q0);

    let flat_a = vmaxq_u8(abs_p1p0, abs_q1q0);
    let mut hev = vqsubq_u8(flat_a, thresh);
    hev = veorq_u8(vceqq_u8(hev, zero), ff);
    // replicate for the "merged variables" usage
    hev = lpf_ziplo32_neon(token, hev, hev);

    abs_p1q1 = vreinterpretq_u8_u16(vshrq_n_u16::<1>(vreinterpretq_u16_u8(vandq_u8(
        abs_p1q1, fe,
    ))));
    // The sum>blimit "don't filter" flag, computed in u16 — same
    // _sse2-exact/_c-exact reasoning as the v3 arm (mblim <= 193).
    let a16 = lpf_widen_neon(token, abs_p0q0);
    let s16 = vaddq_u16(vaddq_u16(a16, a16), lpf_widen_neon(token, abs_p1q1));
    let cmp = vcgtq_s16(vreinterpretq_s16_u16(s16), vreinterpretq_s16_u8(blimit16));
    let flag = vreinterpretq_u8_s8(lpf_packs_dup_neon(token, vreinterpretq_s16_u16(cmp)));

    let mut work = vmaxq_u8(
        lpf_abs_diff_neon(token, qp[2], qp[1]),
        lpf_abs_diff_neon(token, qp[3], qp[2]),
    );
    work = vmaxq_u8(abs_p1p0, work);
    work = vmaxq_u8(work, lpf_bsrl_neon::<4>(token, work));
    work = vqsubq_u8(work, limit);
    let mask = vbicq_u8(vceqq_u8(work, zero), flag);

    // lp filter (shared with the 6/8-tap kernels)
    let (qs1qs0, ps1ps0) = filter4_14_neon(token, p1p0, q1q0, hev, mask);
    let mut qs0ps0 = lpf_ziplo32_neon(token, ps1ps0, qs1qs0);
    let mut qs1ps1 = lpf_bsrl_neon::<8>(token, qs0ps0);

    // flat mask
    let mut flat = vmaxq_u8(
        lpf_abs_diff_neon(token, qp[2], qp[0]),
        lpf_abs_diff_neon(token, qp[3], qp[0]),
    );
    flat = vmaxq_u8(abs_p1p0, flat);
    flat = vmaxq_u8(flat, lpf_bsrl_neon::<4>(token, flat));
    flat = vqsubq_u8(flat, one);
    flat = vceqq_u8(flat, zero);
    flat = vandq_u8(flat, mask);
    flat = lpf_ziplo32_neon(token, flat, flat);
    flat = lpf_ziplo64_neon(token, flat, flat);

    // if flat == 0 then flat2 is zero as well and none of this is needed
    if vminvq_u8(vceqq_u8(flat, zero)) != 0xff {
        let eight = vdupq_n_u16(8);
        let four = vdupq_n_u16(4);
        let mut pq_16 = [vdupq_n_u16(0); 7];
        for (i, v) in pq_16.iter_mut().enumerate() {
            *v = lpf_widen_neon(token, qp[i]);
        }
        let q0_16 = vreinterpretq_u16_u8(lpf_bsrl_neon::<8>(token, vreinterpretq_u8_u16(pq_16[0])));
        let q1_16 = vreinterpretq_u16_u8(lpf_bsrl_neon::<8>(token, vreinterpretq_u8_u16(pq_16[1])));
        let q2_16 = vreinterpretq_u16_u8(lpf_bsrl_neon::<8>(token, vreinterpretq_u8_u16(pq_16[2])));
        let q3_16 = vreinterpretq_u16_u8(lpf_bsrl_neon::<8>(token, vreinterpretq_u8_u16(pq_16[3])));
        let q4_16 = vreinterpretq_u16_u8(lpf_bsrl_neon::<8>(token, vreinterpretq_u8_u16(pq_16[4])));
        let q5_16 = vreinterpretq_u16_u8(lpf_bsrl_neon::<8>(token, vreinterpretq_u8_u16(pq_16[5])));

        let mut sum_p = vaddq_u16(pq_16[5], vaddq_u16(pq_16[4], pq_16[3]));
        let mut sum_lp = vaddq_u16(pq_16[0], vaddq_u16(pq_16[2], pq_16[1]));
        sum_p = vaddq_u16(sum_p, sum_lp);

        let mut sum_lq =
            vreinterpretq_u16_u8(lpf_bsrl_neon::<8>(token, vreinterpretq_u8_u16(sum_lp)));
        let mut sum_q =
            vreinterpretq_u16_u8(lpf_bsrl_neon::<8>(token, vreinterpretq_u8_u16(sum_p)));

        let sum_p_0 = vaddq_u16(eight, vaddq_u16(sum_p, sum_q));
        sum_lp = vaddq_u16(four, vaddq_u16(sum_lp, sum_lq));

        let flat_p0 = vaddq_u16(sum_lp, vaddq_u16(pq_16[3], pq_16[0]));
        let flat_q0 = vaddq_u16(sum_lp, vaddq_u16(q3_16, q0_16));

        let mut sum_p6 = vaddq_u16(pq_16[6], pq_16[6]);
        let mut sum_p3 = vaddq_u16(pq_16[3], pq_16[3]);

        sum_q = vsubq_u16(sum_p_0, pq_16[5]);
        sum_p = vsubq_u16(sum_p_0, q5_16);

        let work0_0 = vaddq_u16(vaddq_u16(pq_16[6], pq_16[0]), pq_16[1]);
        let work0_1 = vaddq_u16(sum_p6, vaddq_u16(pq_16[1], vaddq_u16(pq_16[2], pq_16[0])));

        sum_lq = vsubq_u16(sum_lp, pq_16[2]);
        sum_lp = vsubq_u16(sum_lp, q2_16);

        work = vreinterpretq_u8_u16(vaddq_u16(sum_p3, pq_16[1]));
        let flat_p1 = vaddq_u16(sum_lp, vreinterpretq_u16_u8(work));
        let flat_q1 = vaddq_u16(
            sum_lq,
            vreinterpretq_u16_u8(lpf_bsrl_neon::<8>(token, work)),
        );

        let mut flat_pq0 = vshrq_n_u16::<3>(vreinterpretq_u16_u8(lpf_ziplo64_neon(
            token,
            vreinterpretq_u8_u16(flat_p0),
            vreinterpretq_u8_u16(flat_q0),
        )));
        let mut flat_pq1 = vshrq_n_u16::<3>(vreinterpretq_u16_u8(lpf_ziplo64_neon(
            token,
            vreinterpretq_u8_u16(flat_p1),
            vreinterpretq_u8_u16(flat_q1),
        )));
        flat_pq0 =
            vreinterpretq_u16_u8(lpf_packus_dup_neon(token, vreinterpretq_s16_u16(flat_pq0)));
        flat_pq1 =
            vreinterpretq_u16_u8(lpf_packus_dup_neon(token, vreinterpretq_s16_u16(flat_pq1)));

        sum_lp = vsubq_u16(sum_lp, q1_16);
        sum_lq = vsubq_u16(sum_lq, pq_16[1]);

        sum_p3 = vaddq_u16(sum_p3, pq_16[3]);
        work = vreinterpretq_u8_u16(vaddq_u16(sum_p3, pq_16[2]));

        let flat_p2 = vaddq_u16(sum_lp, vreinterpretq_u16_u8(work));
        let flat_q2 = vaddq_u16(
            sum_lq,
            vreinterpretq_u16_u8(lpf_bsrl_neon::<8>(token, work)),
        );
        let mut flat_pq2 = vshrq_n_u16::<3>(vreinterpretq_u16_u8(lpf_ziplo64_neon(
            token,
            vreinterpretq_u8_u16(flat_p2),
            vreinterpretq_u8_u16(flat_q2),
        )));
        flat_pq2 =
            vreinterpretq_u16_u8(lpf_packus_dup_neon(token, vreinterpretq_s16_u16(flat_pq2)));

        // flat2 mask
        let mut flat2 = vmaxq_u8(
            lpf_abs_diff_neon(token, qp[4], qp[0]),
            lpf_abs_diff_neon(token, qp[5], qp[0]),
        );
        work = lpf_abs_diff_neon(token, qp[6], qp[0]);
        flat2 = vmaxq_u8(work, flat2);
        flat2 = vmaxq_u8(flat2, lpf_bsrl_neon::<4>(token, flat2));
        flat2 = vqsubq_u8(flat2, one);
        flat2 = vceqq_u8(flat2, zero);
        flat2 = vandq_u8(flat2, flat); // flat2 & flat & mask
        flat2 = lpf_ziplo32_neon(token, flat2, flat2);

        // apply flat
        qs0ps0 = vbicq_u8(qs0ps0, flat);
        let flat_pq0u = vandq_u8(flat, vreinterpretq_u8_u16(flat_pq0));
        qp[0] = vorrq_u8(qs0ps0, flat_pq0u);

        qs1ps1 = vbicq_u8(qs1ps1, flat);
        let flat_pq1u = vandq_u8(flat, vreinterpretq_u8_u16(flat_pq1));
        qp[1] = vorrq_u8(qs1ps1, flat_pq1u);

        qp[2] = vbicq_u8(qp[2], flat);
        let flat_pq2u = vandq_u8(flat, vreinterpretq_u8_u16(flat_pq2));
        qp[2] = vorrq_u8(qp[2], flat_pq2u);

        if vminvq_u8(vceqq_u8(flat2, zero)) != 0xff {
            let mut flat2_pq = [vdupq_n_u8(0); 6];

            let flat2_p0 = vaddq_u16(sum_p_0, vaddq_u16(work0_0, q0_16));
            let flat2_q0 = vaddq_u16(
                sum_p_0,
                vaddq_u16(
                    vreinterpretq_u16_u8(lpf_bsrl_neon::<8>(token, vreinterpretq_u8_u16(work0_0))),
                    pq_16[0],
                ),
            );

            let flat2_p1 = vaddq_u16(sum_p, work0_1);
            let flat2_q1 = vaddq_u16(
                sum_q,
                vreinterpretq_u16_u8(lpf_bsrl_neon::<8>(token, vreinterpretq_u8_u16(work0_1))),
            );

            flat2_pq[0] =
                vreinterpretq_u8_u16(vshrq_n_u16::<4>(vreinterpretq_u16_u8(lpf_ziplo64_neon(
                    token,
                    vreinterpretq_u8_u16(flat2_p0),
                    vreinterpretq_u8_u16(flat2_q0),
                ))));
            flat2_pq[1] =
                vreinterpretq_u8_u16(vshrq_n_u16::<4>(vreinterpretq_u16_u8(lpf_ziplo64_neon(
                    token,
                    vreinterpretq_u8_u16(flat2_p1),
                    vreinterpretq_u8_u16(flat2_q1),
                ))));
            flat2_pq[0] = lpf_packus_dup_neon(token, vreinterpretq_s16_u8(flat2_pq[0]));
            flat2_pq[1] = lpf_packus_dup_neon(token, vreinterpretq_s16_u8(flat2_pq[1]));

            sum_p = vsubq_u16(sum_p, q4_16);
            sum_q = vsubq_u16(sum_q, pq_16[4]);

            sum_p6 = vaddq_u16(sum_p6, pq_16[6]);
            work = vreinterpretq_u8_u16(vaddq_u16(
                sum_p6,
                vaddq_u16(pq_16[2], vaddq_u16(pq_16[3], pq_16[1])),
            ));
            let flat2_p2 = vaddq_u16(sum_p, vreinterpretq_u16_u8(work));
            let flat2_q2 = vaddq_u16(sum_q, vreinterpretq_u16_u8(lpf_bsrl_neon::<8>(token, work)));
            flat2_pq[2] =
                vreinterpretq_u8_u16(vshrq_n_u16::<4>(vreinterpretq_u16_u8(lpf_ziplo64_neon(
                    token,
                    vreinterpretq_u8_u16(flat2_p2),
                    vreinterpretq_u8_u16(flat2_q2),
                ))));
            flat2_pq[2] = lpf_packus_dup_neon(token, vreinterpretq_s16_u8(flat2_pq[2]));

            sum_p6 = vaddq_u16(sum_p6, pq_16[6]);
            sum_p = vsubq_u16(sum_p, q3_16);
            sum_q = vsubq_u16(sum_q, pq_16[3]);

            work = vreinterpretq_u8_u16(vaddq_u16(
                sum_p6,
                vaddq_u16(pq_16[3], vaddq_u16(pq_16[4], pq_16[2])),
            ));
            let flat2_p3 = vaddq_u16(sum_p, vreinterpretq_u16_u8(work));
            let flat2_q3 = vaddq_u16(sum_q, vreinterpretq_u16_u8(lpf_bsrl_neon::<8>(token, work)));
            flat2_pq[3] =
                vreinterpretq_u8_u16(vshrq_n_u16::<4>(vreinterpretq_u16_u8(lpf_ziplo64_neon(
                    token,
                    vreinterpretq_u8_u16(flat2_p3),
                    vreinterpretq_u8_u16(flat2_q3),
                ))));
            flat2_pq[3] = lpf_packus_dup_neon(token, vreinterpretq_s16_u8(flat2_pq[3]));

            sum_p6 = vaddq_u16(sum_p6, pq_16[6]);
            sum_p = vsubq_u16(sum_p, q2_16);
            sum_q = vsubq_u16(sum_q, pq_16[2]);

            work = vreinterpretq_u8_u16(vaddq_u16(
                sum_p6,
                vaddq_u16(pq_16[4], vaddq_u16(pq_16[5], pq_16[3])),
            ));
            let flat2_p4 = vaddq_u16(sum_p, vreinterpretq_u16_u8(work));
            let flat2_q4 = vaddq_u16(sum_q, vreinterpretq_u16_u8(lpf_bsrl_neon::<8>(token, work)));
            flat2_pq[4] =
                vreinterpretq_u8_u16(vshrq_n_u16::<4>(vreinterpretq_u16_u8(lpf_ziplo64_neon(
                    token,
                    vreinterpretq_u8_u16(flat2_p4),
                    vreinterpretq_u8_u16(flat2_q4),
                ))));
            flat2_pq[4] = lpf_packus_dup_neon(token, vreinterpretq_s16_u8(flat2_pq[4]));

            sum_p6 = vaddq_u16(sum_p6, pq_16[6]);
            sum_p = vsubq_u16(sum_p, q1_16);
            sum_q = vsubq_u16(sum_q, pq_16[1]);

            work = vreinterpretq_u8_u16(vaddq_u16(
                sum_p6,
                vaddq_u16(pq_16[5], vaddq_u16(pq_16[6], pq_16[4])),
            ));
            let flat2_p5 = vaddq_u16(sum_p, vreinterpretq_u16_u8(work));
            let flat2_q5 = vaddq_u16(sum_q, vreinterpretq_u16_u8(lpf_bsrl_neon::<8>(token, work)));
            flat2_pq[5] =
                vreinterpretq_u8_u16(vshrq_n_u16::<4>(vreinterpretq_u16_u8(lpf_ziplo64_neon(
                    token,
                    vreinterpretq_u8_u16(flat2_p5),
                    vreinterpretq_u8_u16(flat2_q5),
                ))));
            flat2_pq[5] = lpf_packus_dup_neon(token, vreinterpretq_s16_u8(flat2_pq[5]));

            // wide flat apply
            for i in 0..6 {
                qp[i] = vorrq_u8(vbicq_u8(qp[i], flat2), vandq_u8(flat2, flat2_pq[i]));
            }
        }
    } else {
        qp[0] = qs0ps0;
        qp[1] = qs1ps1;
    }
}

/// C `svt_aom_lpf_horizontal_14_sse2` — 4 columns. Loads the 4-byte rows
/// p6..q6 pairwise into the merged `q{i}p{i}` vectors, filters, and stores
/// the 12 modified rows (p5..q5) back.
#[cfg(target_arch = "aarch64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
pub(super) fn lpf_horizontal_14_impl_neon(
    token: NeonToken,
    buf: &mut [u8],
    off: usize,
    pitch: usize,
    t: LfThresh,
) {
    let blimit16 = vreinterpretq_u8_u16(vdupq_n_u16(t.mblim as u16));
    let limit = vdupq_n_u8(t.lim);
    let thresh = vdupq_n_u8(t.hev_thr);

    let row = |k: isize| -> uint8x16_t {
        let i = (off as isize + k * pitch as isize) as usize;
        let r: &[u8; 4] = buf[i..i + 4].try_into().unwrap();
        lpf_ld4_neon(token, r)
    };
    let mut qp = [
        lpf_ziplo32_neon(token, row(-1), row(0)),
        lpf_ziplo32_neon(token, row(-2), row(1)),
        lpf_ziplo32_neon(token, row(-3), row(2)),
        lpf_ziplo32_neon(token, row(-4), row(3)),
        lpf_ziplo32_neon(token, row(-5), row(4)),
        lpf_ziplo32_neon(token, row(-6), row(5)),
        lpf_ziplo32_neon(token, row(-7), row(6)),
    ];
    lpf_internal_14_neon(token, &mut qp, blimit16, limit, thresh);
    // C `store_buffer_horz_8(q{num}p{num}, p, num, s)`, num = 0..=5:
    // stores 4 bytes at s-(num+1)*p and s+num*p.
    for num in 0..6isize {
        let v = qp[num as usize];
        let lo = (off as isize - (num + 1) * pitch as isize) as usize;
        let d_lo: &mut [u8; 4] = (&mut buf[lo..lo + 4]).try_into().unwrap();
        lpf_st4_neon(token, d_lo, v);
        let hi = (off as isize + num * pitch as isize) as usize;
        let d_hi: &mut [u8; 4] = (&mut buf[hi..hi + 4]).try_into().unwrap();
        lpf_st4_neon(token, d_hi, lpf_bsrl_neon::<4>(token, v));
    }
}

/// C `transpose_pq_14_sse2` (dlf_intrin_sse2.c:1029): four 16-byte rows in
/// (x[0..4] = file rows 0..3), eight merged `q{i}p{i}` pairs out.
#[cfg(target_arch = "aarch64")]
#[rite]
pub(super) fn transpose_pq_14_neon(token: NeonToken, x: &[uint8x16_t; 4]) -> [uint8x16_t; 8] {
    let w0 = lpf_ziplo8_neon(token, x[0], x[1]);
    let w1 = lpf_ziplo8_neon(token, x[2], x[3]);
    let w2 = lpf_ziphi8_neon(token, x[0], x[1]);
    let w3 = lpf_ziphi8_neon(token, x[2], x[3]);

    let z16 = |a: uint8x16_t, b: uint8x16_t, hi: bool| -> uint8x16_t {
        let (a16, b16) = (vreinterpretq_u16_u8(a), vreinterpretq_u16_u8(b));
        vreinterpretq_u8_u16(if hi {
            vzip2q_u16(a16, b16)
        } else {
            vzip1q_u16(a16, b16)
        })
    };
    let ww0 = z16(w0, w1, false);
    let ww1 = z16(w0, w1, true);
    let ww2 = z16(w2, w3, false);
    let ww3 = z16(w2, w3, true);

    [
        lpf_ziplo32_neon(token, lpf_bsrl_neon::<12>(token, ww1), ww2), // q0p0
        lpf_ziphi32_neon(token, ww1, lpf_bsll4_neon(token, ww2)),      // q1p1
        lpf_ziphi32_neon(token, lpf_bsll4_neon(token, ww1), ww2),      // q2p2
        lpf_ziplo32_neon(token, ww1, lpf_bsrl_neon::<12>(token, ww2)), // q3p3
        lpf_ziplo32_neon(token, lpf_bsrl_neon::<12>(token, ww0), ww3), // q4p4
        lpf_ziphi32_neon(token, ww0, lpf_bsll4_neon(token, ww3)),      // q5p5
        lpf_ziphi32_neon(token, lpf_bsll4_neon(token, ww0), ww3),      // q6p6
        lpf_ziplo32_neon(token, ww0, lpf_bsrl_neon::<12>(token, ww3)), // q7p7
    ]
}

/// C `transpose_pq_14_inv_sse2` (dlf_intrin_sse2.c:1062): eight merged
/// pairs in (x[0..8] = q7p7..q0p0, C's argument order), four 16-byte
/// rows out.
#[cfg(target_arch = "aarch64")]
#[rite]
pub(super) fn transpose_pq_14_inv_neon(token: NeonToken, x: &[uint8x16_t; 8]) -> [uint8x16_t; 4] {
    let z16 = |a: uint8x16_t, b: uint8x16_t, hi: bool| -> uint8x16_t {
        let (a16, b16) = (vreinterpretq_u16_u8(a), vreinterpretq_u16_u8(b));
        vreinterpretq_u8_u16(if hi {
            vzip2q_u16(a16, b16)
        } else {
            vzip1q_u16(a16, b16)
        })
    };
    let w0 = lpf_ziplo8_neon(token, x[0], x[1]);
    let w1 = lpf_ziplo8_neon(token, x[2], x[3]);
    let w2 = lpf_ziplo8_neon(token, x[4], x[5]);
    let w3 = lpf_ziplo8_neon(token, x[6], x[7]);

    let w4 = z16(w0, w1, false);
    let w5 = z16(w2, w3, false);

    let d0 = lpf_ziplo32_neon(token, w4, w5);
    let d2 = lpf_ziphi32_neon(token, w4, w5);

    let w10 = lpf_ziplo8_neon(token, x[7], x[6]);
    let w11 = lpf_ziplo8_neon(token, x[5], x[4]);
    let w12 = lpf_ziplo8_neon(token, x[3], x[2]);
    let w13 = lpf_ziplo8_neon(token, x[1], x[0]);

    let w4b = z16(w10, w11, true);
    let w5b = z16(w12, w13, true);

    let d1 = lpf_ziplo32_neon(token, w4b, w5b);
    let d3 = lpf_ziphi32_neon(token, w4b, w5b);

    [
        lpf_ziplo64_neon(token, d0, d1),
        lpf_ziphi64_neon(token, d0, d1),
        lpf_ziplo64_neon(token, d2, d3),
        lpf_ziphi64_neon(token, d2, d3),
    ]
}

/// C `svt_aom_lpf_vertical_14_sse2` — 4 rows. Loads a 16-byte p7..q7
/// window per row (the outermost pair round-trips unchanged), transposes
/// to merged pairs, filters, transposes back, stores all 16 bytes.
#[cfg(target_arch = "aarch64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
pub(super) fn lpf_vertical_14_impl_neon(
    token: NeonToken,
    buf: &mut [u8],
    off: usize,
    pitch: usize,
    t: LfThresh,
) {
    let blimit16 = vreinterpretq_u8_u16(vdupq_n_u16(t.mblim as u16));
    let limit = vdupq_n_u8(t.lim);
    let thresh = vdupq_n_u8(t.hev_thr);

    let mut x = [vdupq_n_u8(0); 4];
    for (i, v) in x.iter_mut().enumerate() {
        let b = off - 8 + i * pitch;
        let r: &[u8; 16] = buf[b..b + 16].try_into().unwrap();
        *v = vld1q_u8(r);
    }
    let qp8 = transpose_pq_14_neon(token, &x);
    let mut qp: [uint8x16_t; 7] = qp8[..7].try_into().unwrap();
    lpf_internal_14_neon(token, &mut qp, blimit16, limit, thresh);
    // C passes q7p7..q0p0 to the inverse transpose.
    let inv_in = [qp8[7], qp[6], qp[5], qp[4], qp[3], qp[2], qp[1], qp[0]];
    let pq = transpose_pq_14_inv_neon(token, &inv_in);
    for (i, v) in pq.iter().enumerate() {
        let b = off - 8 + i * pitch;
        let d: &mut [u8; 16] = (&mut buf[b..b + 16]).try_into().unwrap();
        vst1q_u8(d, *v);
    }
}
