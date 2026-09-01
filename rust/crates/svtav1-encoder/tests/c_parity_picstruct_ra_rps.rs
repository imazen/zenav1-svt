//! The random-access hierarchical reference structure, checked against the
//! **real C encoder's own bitstream** — evidence tier 2
//! (`docs/WORKING-ON-THIS.md` §4).
//!
//! `av1_generate_rps_info` is `static` with no exported symbol, so tier 1 is
//! not reachable and `port_picstruct_ra`'s tables are hand transcriptions of
//! `pd_process.c`. A transcribed oracle agreeing with transcribed code proves
//! only that both were transcribed the same way — so this test does not read
//! the C source at all. It reads what the encoder WROTE.
//!
//! Everything `av1_generate_rps_info` computes has a written form in the
//! uncompressed frame header:
//!
//! | port | AV1 frame header (spec 5.9.2) |
//! |---|---|
//! | `rps.ref_dpb_index[i]` | `ref_frame_idx[i]` |
//! | `rps.refresh_frame_mask` | `refresh_frame_flags` |
//! | `show_frame` | `show_frame` |
//! | `has_show_existing` + `show_existing_frame` | the following `show_existing_frame` OBU's `frame_to_show_map_idx` |
//!
//! So the test parses those four out of a committed C stream and drives the
//! port over the same picture sequence.
//!
//! ## The captures
//!
//! `tests/data/picstruct_ra/ra_hl{1..5}.obu` (preset 8) and
//! `ra_p4_hl{1..5}.obu` (preset 4), produced by
//! `tools/gen_ra_rps_captures.sh` — the real SVT-AV1 library through
//! `tools/capture_c_trace`, 64x64 gradient, qp 35, `--pred-struct 2`
//! (RANDOM_ACCESS), `--hierarchical-levels H`. Frame counts are chosen so
//! EVERY mini-GOP is complete: 7, 9, 17, 17 and 33 frames for HL1..HL5, i.e.
//! one key frame plus 6, 8, 16, 16 and 32 inter frames.
//!
//! ## What this gate covers — and what it structurally cannot
//!
//! * **Every `pic_idx` of every branch table** is exercised: 2/2, 4/4, 8/8,
//!   16/16 and 32/32 mini-GOP positions, at two presets.
//! * **1,092 reference columns compared, of which 865 carry the table's own
//!   value and 227 carry `prune_refs`'s.** `prune_refs` folds unused list
//!   slots onto LAST / BWD, and a folded column cannot witness the entry that
//!   was overwritten — no bitstream oracle can, because the encoder never
//!   wrote it. The test MEASURES that split rather than assuming it; the count
//!   is asserted so a change that shrinks it has to say so. This is exactly
//!   why preset 4 is here: at preset 8 alone the caps are 3 and 2, so GOLD and
//!   ALT are folded on every single frame (verified by mutation — changing the
//!   HL5 layer-3 ALT entry leaves the preset-8 captures green).
//! * **The layer-0 toggle ring** (0->1->2->0) is walked end to end only at HL1
//!   (3 mini-GOPs) and partly at HL2/HL3 (2 each). HL4 and HL5 see one
//!   mini-GOP, so their layer-0 toggle only advances 0->1 here.
//! * **NOT covered at all: an incomplete trailing mini-GOP**, which is the one
//!   shape that exercises the LOW_DELAY-inside-RA adjustment in
//!   `toggles_for_picture`. Both gaps are a harness limit rather than a
//!   choice — the C driver's ST-mode object pool exhausts above 7 / 9 / 17 /
//!   25 / 41 frames at HL1..HL5 ("empty object pool exhausted after pumping
//!   dispatcher"), so longer runs need a driver change.
//! * **Not covered: overlay frames** (`enable_overlays` is off by default) and
//!   `referencing_scheme == 2`, which the preset cascade never selects.
//!
//! ## The configuration, read out of the C source (not guessed)
//!
//! `set_mrp_ctrl` (`Globals/enc_handle.c:3573-3612`) maps preset 8 to mrp
//! level 6 and preset 4 to level 4; `set_mrp_ctrl_with_level` (`:3361`) gives
//! their fields. `captures_have_the_shape_they_claim` checks the one field
//! that is observable in the bitstream — `referencing_scheme`, via whether
//! top-layer pictures refresh a DPB slot — so that value is evidence rather
//! than an assertion.

use svtav1_encoder::inter_mvp::OrderHintInfo;
use svtav1_encoder::port_picstruct as pp;
use svtav1_encoder::port_picstruct_ra as ra;

// ---------------------------------------------------------------------------
// A frame-header reader, spec 5.9.2 as far as `ref_frame_idx[]`
// ---------------------------------------------------------------------------

const OBU_SEQUENCE_HEADER: u8 = 1;
const OBU_FRAME_HEADER: u8 = 3;
const OBU_FRAME: u8 = 6;
const KEY_FRAME: u8 = 0;
const INTRA_ONLY_FRAME: u8 = 2;
const SWITCH_FRAME: u8 = 3;
const SELECT_SCREEN_CONTENT_TOOLS: u8 = 2;
const SELECT_INTEGER_MV: u8 = 2;

/// MSB-first bit cursor. Reading past the end panics rather than wrapping —
/// a misaligned parse must fail loudly, not return a plausible wrong answer.
struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Bits<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }
    fn f(&mut self, n: usize) -> u32 {
        let mut v = 0u32;
        for _ in 0..n {
            let byte = self.data[self.pos >> 3];
            v = (v << 1) | u32::from((byte >> (7 - (self.pos & 7))) & 1);
            self.pos += 1;
        }
        v
    }
    fn uvlc(&mut self) -> u64 {
        let mut leading = 0;
        while self.f(1) == 0 {
            leading += 1;
            assert!(leading < 32, "malformed uvlc");
        }
        if leading == 0 {
            return 0;
        }
        u64::from(self.f(leading)) + (1u64 << leading) - 1
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct SeqHeader {
    reduced_still_picture_header: bool,
    decoder_model_info_present: bool,
    equal_picture_interval: bool,
    frame_presentation_time_len: usize,
    frame_id_numbers_present: bool,
    id_len: usize,
    delta_frame_id_len: usize,
    enable_order_hint: bool,
    order_hint_bits: usize,
    seq_force_screen_content_tools: u8,
    seq_force_integer_mv: u8,
}

fn parse_sequence_header(payload: &[u8]) -> SeqHeader {
    let mut b = Bits::new(payload);
    let mut sh = SeqHeader::default();
    let seq_profile = b.f(3);
    b.f(1); // still_picture
    sh.reduced_still_picture_header = b.f(1) == 1;
    assert!(
        !sh.reduced_still_picture_header,
        "these captures are multi-frame streams"
    );
    assert!(seq_profile <= 2);

    let mut buffer_delay_len = 0usize;
    sh.equal_picture_interval = true;
    if b.f(1) == 1 {
        // timing_info()
        b.f(32);
        b.f(32);
        sh.equal_picture_interval = b.f(1) == 1;
        if sh.equal_picture_interval {
            b.uvlc();
        }
        sh.decoder_model_info_present = b.f(1) == 1;
        if sh.decoder_model_info_present {
            buffer_delay_len = b.f(5) as usize + 1;
            b.f(32);
            b.f(5); // buffer_removal_time_length_minus_1
            sh.frame_presentation_time_len = b.f(5) as usize + 1;
        }
    }
    let initial_display_delay_present = b.f(1) == 1;
    let ops = b.f(5) + 1;
    for _ in 0..ops {
        b.f(12);
        if b.f(5) > 7 {
            b.f(1);
        }
        if sh.decoder_model_info_present && b.f(1) == 1 {
            b.f(buffer_delay_len);
            b.f(buffer_delay_len);
            b.f(1);
        }
        if initial_display_delay_present && b.f(1) == 1 {
            b.f(4);
        }
    }

    let w_bits = b.f(4) as usize + 1;
    let h_bits = b.f(4) as usize + 1;
    b.f(w_bits);
    b.f(h_bits);

    sh.frame_id_numbers_present = b.f(1) == 1;
    if sh.frame_id_numbers_present {
        sh.delta_frame_id_len = b.f(4) as usize + 2;
        sh.id_len = b.f(3) as usize + 1 + sh.delta_frame_id_len;
    }

    b.f(1); // use_128x128_superblock
    b.f(1); // enable_filter_intra
    b.f(1); // enable_intra_edge_filter

    b.f(1); // enable_interintra_compound
    b.f(1); // enable_masked_compound
    b.f(1); // enable_warped_motion
    b.f(1); // enable_dual_filter
    sh.enable_order_hint = b.f(1) == 1;
    if sh.enable_order_hint {
        b.f(1); // enable_jnt_comp
        b.f(1); // enable_ref_frame_mvs
    }
    sh.seq_force_screen_content_tools = if b.f(1) == 1 {
        SELECT_SCREEN_CONTENT_TOOLS
    } else {
        b.f(1) as u8
    };
    sh.seq_force_integer_mv = if sh.seq_force_screen_content_tools > 0 {
        if b.f(1) == 1 {
            SELECT_INTEGER_MV
        } else {
            b.f(1) as u8
        }
    } else {
        SELECT_INTEGER_MV
    };
    sh.order_hint_bits = if sh.enable_order_hint {
        b.f(3) as usize + 1
    } else {
        0
    };
    sh
}

/// One frame header, in decode order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Frame {
    /// A coded frame.
    Coded {
        frame_type: u8,
        show_frame: bool,
        order_hint: u32,
        refresh_frame_flags: u8,
        /// `None` for an intra frame, which codes no reference indices.
        ref_frame_idx: Option<[u8; 7]>,
    },
    /// A `show_existing_frame` header.
    ShowExisting { map_idx: u8 },
}

fn parse_frame_header(payload: &[u8], sh: &SeqHeader) -> Frame {
    let mut b = Bits::new(payload);
    if b.f(1) == 1 {
        let map_idx = b.f(3) as u8;
        return Frame::ShowExisting { map_idx };
    }
    let frame_type = b.f(2) as u8;
    let frame_is_intra = frame_type == KEY_FRAME || frame_type == INTRA_ONLY_FRAME;
    let show_frame = b.f(1) == 1;
    if show_frame && sh.decoder_model_info_present && !sh.equal_picture_interval {
        b.f(sh.frame_presentation_time_len);
    }
    if !show_frame {
        b.f(1); // showable_frame
    }
    let error_resilient = if frame_type == SWITCH_FRAME || (frame_type == KEY_FRAME && show_frame) {
        true
    } else {
        b.f(1) == 1
    };
    b.f(1); // disable_cdf_update
    let allow_sc = if sh.seq_force_screen_content_tools == SELECT_SCREEN_CONTENT_TOOLS {
        b.f(1) == 1
    } else {
        sh.seq_force_screen_content_tools == 1
    };
    if allow_sc && sh.seq_force_integer_mv == SELECT_INTEGER_MV {
        b.f(1); // force_integer_mv
    }
    if sh.frame_id_numbers_present {
        b.f(sh.id_len);
    }
    if frame_type != SWITCH_FRAME {
        b.f(1); // frame_size_override_flag
    }
    let order_hint = b.f(sh.order_hint_bits);
    if !frame_is_intra && !error_resilient {
        b.f(3); // primary_ref_frame
    }
    assert!(
        !sh.decoder_model_info_present,
        "buffer_removal_time is not parsed here"
    );
    let refresh = if frame_type == SWITCH_FRAME || (frame_type == KEY_FRAME && show_frame) {
        0xFF
    } else {
        b.f(8) as u8
    };
    if (!frame_is_intra || refresh != 0xFF) && error_resilient && sh.enable_order_hint {
        for _ in 0..8 {
            b.f(sh.order_hint_bits);
        }
    }
    if frame_is_intra {
        return Frame::Coded {
            frame_type,
            show_frame,
            order_hint,
            refresh_frame_flags: refresh,
            ref_frame_idx: None,
        };
    }
    let short_signaling = if sh.enable_order_hint {
        b.f(1) == 1
    } else {
        false
    };
    assert!(
        !short_signaling,
        "frame_refs_short_signaling would hide ref_frame_idx"
    );
    let mut refs = [0u8; 7];
    for r in &mut refs {
        *r = b.f(3) as u8;
        if sh.frame_id_numbers_present {
            b.f(sh.delta_frame_id_len);
        }
    }
    Frame::Coded {
        frame_type,
        show_frame,
        order_hint,
        refresh_frame_flags: refresh,
        ref_frame_idx: Some(refs),
    }
}

fn leb128(data: &[u8], i: &mut usize) -> usize {
    let mut value = 0usize;
    let mut shift = 0;
    loop {
        let b = data[*i];
        *i += 1;
        value |= usize::from(b & 0x7F) << shift;
        shift += 7;
        if b & 0x80 == 0 {
            return value;
        }
    }
}

fn parse_stream(data: &[u8]) -> (SeqHeader, Vec<Frame>) {
    let mut i = 0usize;
    let mut sh = None;
    let mut frames = Vec::new();
    while i < data.len() {
        let first = data[i];
        assert_eq!(first & 0x80, 0, "obu_forbidden_bit set at byte {i}");
        let obu_type = (first >> 3) & 0xF;
        let extension = (first >> 2) & 1;
        let has_size = (first >> 1) & 1;
        i += 1;
        if extension == 1 {
            i += 1;
        }
        assert_eq!(has_size, 1, "obu_has_size_field must be set");
        let size = leb128(data, &mut i);
        let payload = &data[i..i + size];
        i += size;
        match obu_type {
            OBU_SEQUENCE_HEADER => sh = Some(parse_sequence_header(payload)),
            OBU_FRAME_HEADER | OBU_FRAME => {
                let sh = sh.as_ref().expect("frame header before sequence header");
                frames.push(parse_frame_header(payload, sh));
            }
            _ => {}
        }
    }
    (sh.expect("no sequence header"), frames)
}

// ---------------------------------------------------------------------------
// Drive the port over the same picture sequence
// ---------------------------------------------------------------------------

/// The two `MrpCtrls` shapes these captures were encoded with, read out of
/// `set_mrp_ctrl_with_level` (`Globals/enc_handle.c:3361-3556`) and the
/// preset cascade in `set_mrp_ctrl` (`:3573-3612`).
///
/// **Why two presets and not one.** At preset 8 the list caps are 3 and 2, so
/// `prune_refs` folds `GOLD` onto `LAST` and `ALT` onto `BWD` on EVERY frame —
/// those two columns are then structurally unobservable in the bitstream and a
/// wrong table entry in either is invisible (verified by mutation: changing
/// `ALT` in the HL5 layer-3 row leaves the preset-8 captures passing). Preset 4
/// selects mrp level 4, whose caps are 4 and 3, so nothing is folded and both
/// columns are checked. It also turns ON the two knobs that MOVE table entries:
/// `referencing_scheme = 1` (which makes top-layer pictures references, and
/// changes two entries of the HL1 layer-1 row) and `more_5L_refs = 1` (six
/// entries across HL4).
fn mrp_for_preset(preset: u8) -> pp::MrpCtrls {
    let base = pp::MrpCtrls {
        referencing_scheme: 0,
        base_ref_list0_count: 3,
        base_ref_list1_count: 2,
        non_base_ref_list0_count: 3,
        non_base_ref_list1_count: 2,
        more_5l_refs: 0,
        safe_limit_nref: 2,
        safe_limit_zz_th: 60000,
        ld_reduce_ref_buffs: 0,
        flat_max_refs: 0,
        early_hme_l0_prune_th: 0,
    };
    match preset {
        // mrp level 6 (`:3450`) — preset 8 (`enc_mode <= ENC_M8`).
        8 => base,
        // mrp level 4 (`:3422`) — preset 4 (`enc_mode <= ENC_M4`).
        4 => pp::MrpCtrls {
            referencing_scheme: 1,
            base_ref_list0_count: 4,
            base_ref_list1_count: 3,
            non_base_ref_list0_count: 4,
            non_base_ref_list1_count: 3,
            more_5l_refs: 1,
            early_hme_l0_prune_th: 0,
            ..base
        },
        _ => unreachable!("no capture uses preset {preset}"),
    }
}

/// A picture's mini-GOP position and temporal layer, from its POC.
///
/// An `H`-level pyramid covers display POCs `k*2^H + 1 ..= (k+1)*2^H`; C's
/// `pic_idx` is the 0-based position inside that span, and the temporal layer
/// is `H` minus the number of trailing zeros of the 1-based position — so the
/// last picture of the span (`2^H`) is the base at layer 0 and every odd
/// position is top-layer. This is derived from the geometry, not from
/// `pd_process.c`, and the captures check it: a wrong layer would put the
/// picture in the wrong branch arm and the reference indices would not match.
fn position(poc: u64, hier: u8) -> (u32, u8) {
    let mg = 1u64 << hier;
    let n = ((poc - 1) % mg) + 1;
    let pic_idx = (n - 1) as u32;
    let tl = hier - u8::try_from(n.trailing_zeros()).unwrap();
    (pic_idx, tl)
}

struct Case {
    hier: u8,
    preset: u8,
    stream: &'static [u8],
}

macro_rules! cases {
    ($(($hier:expr, $preset:expr, $path:literal)),* $(,)?) => {
        &[$(Case { hier: $hier, preset: $preset, stream: include_bytes!($path) }),*]
    };
}

const CASES: &[Case] = cases![
    (1, 8, "data/picstruct_ra/ra_hl1.obu"),
    (2, 8, "data/picstruct_ra/ra_hl2.obu"),
    (3, 8, "data/picstruct_ra/ra_hl3.obu"),
    (4, 8, "data/picstruct_ra/ra_hl4.obu"),
    (5, 8, "data/picstruct_ra/ra_hl5.obu"),
    (1, 4, "data/picstruct_ra/ra_p4_hl1.obu"),
    (2, 4, "data/picstruct_ra/ra_p4_hl2.obu"),
    (3, 4, "data/picstruct_ra/ra_p4_hl3.obu"),
    (4, 4, "data/picstruct_ra/ra_p4_hl4.obu"),
    (5, 4, "data/picstruct_ra/ra_p4_hl5.obu"),
];

/// Every coded frame of five real random-access streams: the port's
/// `ref_dpb_index[]`, `refresh_frame_mask`, `show_frame` and
/// `show_existing_frame` must equal what the C encoder wrote.
#[test]
fn c_parity_ra_reference_structure() {
    let mut total_inter = 0usize;
    let mut total_show_existing = 0usize;
    let mut distinct_rows = std::collections::HashSet::new();
    // How many of the 7 reference columns the BITSTREAM can actually witness.
    // `prune_refs` folds unused list slots onto LAST / BWD, and a folded column
    // carries prune's value rather than the table's — so it tests `prune_refs`
    // and the LAST/BWD entries, not the entry that was overwritten. Counting
    // the split is the difference between "156 frames compared" and knowing
    // what those comparisons could detect.
    let mut columns_from_table = 0usize;
    let mut columns_from_prune = 0usize;

    for case in CASES {
        let (sh, frames) = parse_stream(case.stream);
        let hier = case.hier;
        let preset = case.preset;
        let mg = 1u32 << hier;

        let seq = pp::SeqPicParams {
            pred_structure: pp::PredStructure::RandomAccess,
            rate_control_mode: pp::RcMode::CqpOrCrf,
            rtc: false,
            allintra: false,
            mrp_ctrls: mrp_for_preset(case.preset),
            order_hint_info: OrderHintInfo {
                enable_order_hint: sh.enable_order_hint,
                order_hint_bits: u32::try_from(sh.order_hint_bits).unwrap(),
            },
            hierarchical_levels: hier,
            max_managed_refs: 0,
        };

        let mut ctx = pp::PicDecisionCtx::new();
        ctx.mini_gop_length[0] = mg;
        let mut decode_order = 0u64;
        let mut pending_show_existing: Option<u8> = None;

        for (k, frame) in frames.iter().enumerate() {
            match *frame {
                Frame::ShowExisting { map_idx } => {
                    let want = pending_show_existing.take().unwrap_or_else(|| {
                        panic!("HL{hier} record {k}: C re-displayed slot {map_idx}, the port did not set has_show_existing")
                    });
                    assert_eq!(
                        want, map_idx,
                        "p{preset} HL{hier} record {k}: show_existing_frame slot"
                    );
                    total_show_existing += 1;
                }
                Frame::Coded {
                    frame_type,
                    show_frame,
                    order_hint,
                    refresh_frame_flags,
                    ref_frame_idx,
                } => {
                    assert!(
                        pending_show_existing.is_none(),
                        "HL{hier} record {k}: the port asked to re-display a slot C never re-displayed"
                    );
                    let poc = u64::from(order_hint);
                    let is_key = frame_type == KEY_FRAME;
                    let (pic_idx, tl) = if is_key { (0, 0) } else { position(poc, hier) };

                    let mut pic = pp::PicParams {
                        picture_number: poc,
                        decode_order,
                        slice_type: if is_key {
                            pp::SliceType::I
                        } else {
                            pp::SliceType::B
                        },
                        is_key_frame: is_key,
                        is_intra_only: is_key,
                        temporal_layer_index: tl,
                        hierarchical_levels: hier,
                        pred_struct_type: pp::PredStructure::RandomAccess,
                        pred_struct_entry_count: mg,
                        frame_offset: poc,
                        aligned_width: 64,
                        aligned_height: 64,
                        ..Default::default()
                    };
                    decode_order += 1;

                    // The row the branch table produces, BEFORE `prune_refs`.
                    // Read with the pre-call context, because the call
                    // advances the toggles.
                    let unpruned = (!is_key).then(|| {
                        let slots = ra::toggles_for_picture(&pic, &ctx, pic_idx);
                        let row = ra::slot_table(
                            hier,
                            tl,
                            pic_idx,
                            seq.mrp_ctrls.referencing_scheme,
                            seq.mrp_ctrls.more_5l_refs != 0,
                            false,
                        )
                        .expect("a coded position");
                        slots.resolve_row(row)
                    });

                    pp::picture_decision_per_picture(&mut pic, &seq, &mut ctx, pic_idx, 0)
                        .unwrap_or_else(|e| {
                            panic!("HL{hier} POC {poc} (pic_idx {pic_idx}, TL{tl}): {e}")
                        });

                    assert_eq!(
                        pic.rps.refresh_frame_mask, refresh_frame_flags,
                        "p{preset} HL{hier} POC {poc} (pic_idx {pic_idx}, TL{tl}): refresh_frame_flags"
                    );
                    if let Some(want) = ref_frame_idx {
                        assert_eq!(
                            pic.rps.ref_dpb_index, want,
                            "p{preset} HL{hier} POC {poc} (pic_idx {pic_idx}, TL{tl}): ref_frame_idx"
                        );
                        total_inter += 1;
                        distinct_rows.insert((hier, preset, want));
                        let unpruned = unpruned.expect("an inter frame has a table row");
                        for i in 0..7 {
                            if unpruned[i] == want[i] {
                                columns_from_table += 1;
                            } else {
                                columns_from_prune += 1;
                            }
                        }
                    }
                    assert_eq!(
                        pic.show_frame, show_frame,
                        "p{preset} HL{hier} POC {poc} (pic_idx {pic_idx}, TL{tl}): show_frame"
                    );
                    if pic.has_show_existing {
                        pending_show_existing = Some(pic.show_existing_frame);
                    }
                }
            }
        }
        assert!(
            pending_show_existing.is_none(),
            "HL{hier}: a re-display was left pending at the end of the stream"
        );
    }

    // Anti-vacuity: the loop above passes trivially if it compared nothing, or
    // if every frame carried the same reference row.
    assert_eq!(
        total_inter, 156,
        "every inter frame of the five captures must be compared"
    );
    assert_eq!(total_show_existing, 78, "every re-display must be compared");
    assert!(
        distinct_rows.len() >= 40,
        "only {} distinct reference rows — the captures are not exercising the tables",
        distinct_rows.len()
    );

    // The observability split, measured rather than assumed. A column
    // `prune_refs` overwrote cannot witness its table entry no matter what the
    // oracle is — that is a property of the bitstream, not of this test — so
    // this number is the honest denominator for "what the tier-2 gate covers".
    // It is asserted as an exact value so that a future change which quietly
    // shrinks it has to say so.
    let total_columns = columns_from_table + columns_from_prune;
    assert_eq!(total_columns, 156 * 7);
    assert_eq!(
        (columns_from_table, columns_from_prune),
        (865, 227),
        "the table-vs-prune column split changed"
    );
}

/// The captures are what they claim to be, checked before any of their content
/// is believed: five RANDOM_ACCESS streams, one key frame each, with the
/// pyramid's own frame counts.
///
/// A capture regenerated with the wrong `SVT_HIER_LEVELS` would otherwise be
/// compared against the wrong branch and could only fail confusingly.
#[test]
fn captures_have_the_shape_they_claim() {
    for case in CASES {
        let (sh, frames) = parse_stream(case.stream);
        let hier = case.hier;
        assert!(
            sh.enable_order_hint,
            "HL{hier}: order hints must be present"
        );

        let coded: Vec<_> = frames
            .iter()
            .filter_map(|f| match f {
                Frame::Coded {
                    order_hint,
                    frame_type,
                    ..
                } => Some((*order_hint, *frame_type)),
                Frame::ShowExisting { .. } => None,
            })
            .collect();
        assert_eq!(
            coded[0],
            (0, KEY_FRAME),
            "HL{hier}: frame 0 is the key frame"
        );
        assert_eq!(
            coded.iter().filter(|(_, t)| *t == KEY_FRAME).count(),
            1,
            "HL{hier}: exactly one key frame"
        );

        // The POCs are 0..=N with no gaps and no repeats, and N is a whole
        // number of mini-GOPs — the premise that makes `position()` valid.
        let mut pocs: Vec<u32> = coded.iter().map(|(p, _)| *p).collect();
        pocs.sort_unstable();
        let n = pocs.len() as u32 - 1;
        assert_eq!(
            pocs,
            (0..=n).collect::<Vec<_>>(),
            "HL{hier}: POCs must be a gapless 0..=N"
        );
        assert_eq!(
            n % (1 << hier),
            0,
            "HL{hier}: {n} inter frames is not a whole number of mini-GOPs"
        );

        // `referencing_scheme` decides whether TOP-LAYER pictures enter the
        // DPB at all, and it is observable: a top-layer picture that refreshes
        // no slot is not a reference. Checking it here is what makes
        // `mrp_for_preset`'s value evidence rather than an assertion — the two
        // presets must disagree, and they must disagree in this direction.
        // `svt_aom_is_pic_used_as_ref` (`pd_process.c:1781-1798`) returns
        // `false` for hierarchical_levels 5 unconditionally — the ONE level
        // where the scheme does not matter, and the reason HL5's layer-5 arm
        // zeroes `refresh_frame_mask` without consulting `is_ref`
        // (`:3424`). The captures confirm that asymmetry independently: at
        // preset 4 (`referencing_scheme = 1`) HL1..HL4's top layers DO refresh
        // and HL5's still does not.
        let want_top_layer_is_ref =
            hier != 5 && mrp_for_preset(case.preset).referencing_scheme != 0;
        let mut saw_top_layer = 0;
        for f in &frames {
            if let Frame::Coded {
                order_hint,
                refresh_frame_flags,
                frame_type,
                ..
            } = *f
            {
                if frame_type == KEY_FRAME {
                    continue;
                }
                let (_, tl) = position(u64::from(order_hint), hier);
                if tl == hier {
                    saw_top_layer += 1;
                    assert_eq!(
                        refresh_frame_flags != 0,
                        want_top_layer_is_ref,
                        "p{} HL{hier} POC {order_hint}: top-layer refresh disagrees with referencing_scheme",
                        case.preset
                    );
                }
            }
        }
        assert!(
            saw_top_layer > 0,
            "HL{hier}: no top-layer picture in the capture"
        );
    }
}
