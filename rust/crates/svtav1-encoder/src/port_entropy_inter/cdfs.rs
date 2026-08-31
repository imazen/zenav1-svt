//! Default CDF tables for the inter syntax elements, taken from C.
//!
//! Every table here was EXTRACTED from the real
//! `svt_aom_init_mode_probs` (cabac_context_model.c:740) through the
//! `svtav1-cref` shim rather than transcribed from the C source, and
//! `tests/c_parity_entropy_inter.rs` re-asserts each one against that same
//! exported function — evidence tier 1 (`docs/WORKING-ON-THIS.md` §4).
//!
//! # Why this module exists rather than fields on `FrameContext`
//!
//! Two distinct gaps, both measured by reading
//! `entropy/context.rs::FrameContext::new_default`:
//!
//! * **Placeholders.** `skip_mode_cdf`, `newmv_cdf`, `globalmv_cdf`,
//!   `refmv_cdf`, `drl_cdf`, `inter_compound_mode_cdf` and
//!   `interp_filter_cdf` are initialised to UNIFORM values there, not to the
//!   C defaults. They are byte-inert today (the public entry point still
//!   refuses inter frames, and no intra site touches them), but the first
//!   inter block coded against a uniform table desyncs the tile immediately.
//! * **Absent fields.** `comp_ref_type_cdf`, `uni_comp_ref_cdf`,
//!   `comp_bwdref_cdf`, `obmc_cdf`, `motion_mode_cdf`, `comp_group_idx_cdf`,
//!   `compound_index_cdf`, `compound_type_cdf`, `interintra_cdf`,
//!   `interintra_mode_cdf`, `wedge_interintra_cdf` and `wedge_idx_cdf` have
//!   no `FrameContext` field at all.
//!
//! `FrameContext` is owned by another lane in this campaign, so this module
//! carries the tables and [`InterCdfs`] carries the mutable per-frame state
//! the writers here adapt. When the `FrameContext` owner lifts these in, the
//! constants move verbatim and the parity test moves with them.
//!
//! `single_ref_cdf`, `comp_ref_cdf`, `comp_inter_cdf`, `intra_inter_cdf`,
//! `y_mode_cdf` and `angle_delta_cdf` ARE already the real C tables on
//! `FrameContext`; the parity test checks those in place instead of
//! duplicating them.

use crate::entropy::cdf::AomCdfProb;

/// C `default_comp_ref_type_cdf` (cabac_context_model.c:449). NOT present in
/// `FrameContext` at all — the UNIDIR-vs-BIDIR compound-reference-type symbol
/// `write_ref_frames` emits first for any compound block.
#[rustfmt::skip]
pub static COMP_REF_TYPE_CDF: [[AomCdfProb; 3]; 5] = [
    [31570, 0, 0],
    [30698, 0, 0],
    [23602, 0, 0],
    [25269, 0, 0],
    [10293, 0, 0],
];

/// C `default_uni_comp_ref_cdf` (cabac_context_model.c:453),
/// `[UNI_COMP_REF_CONTEXTS][UNIDIR_COMP_REFS - 1][CDF2]`. NOT present in
/// `FrameContext`.
#[rustfmt::skip]
pub static UNI_COMP_REF_CDF: [[[AomCdfProb; 3]; 3]; 3] = [
    [
        [27484, 0, 0],
        [28903, 0, 0],
        [29640, 0, 0],
    ],
    [
        [9616, 0, 0],
        [18595, 0, 0],
        [17498, 0, 0],
    ],
    [
        [994, 0, 0],
        [7648, 0, 0],
        [6058, 0, 0],
    ],
];

/// C `default_comp_bwdref_cdf` (cabac_context_model.c:471),
/// `[REF_CONTEXTS][BWD_REFS - 1][CDF2]`. NOT present in `FrameContext`.
#[rustfmt::skip]
pub static COMP_BWDREF_CDF: [[[AomCdfProb; 3]; 2]; 3] = [
    [
        [30533, 0, 0],
        [31345, 0, 0],
    ],
    [
        [15586, 0, 0],
        [17593, 0, 0],
    ],
    [
        [2162, 0, 0],
        [2279, 0, 0],
    ],
];

/// C `default_skip_mode_cdfs` (cabac_context_model.c:598).
/// `FrameContext::skip_mode_cdf` holds a UNIFORM placeholder
/// (`context.rs`, `new_default`), so a block coded against it desyncs.
#[rustfmt::skip]
pub static SKIP_MODE_CDF: [[AomCdfProb; 3]; 3] = [
    [147, 0, 0],
    [12060, 0, 0],
    [24641, 0, 0],
];

/// C `default_newmv_cdf` (cabac_context_model.c:348).
/// `FrameContext::newmv_cdf` holds a UNIFORM placeholder.
#[rustfmt::skip]
pub static NEWMV_CDF: [[AomCdfProb; 3]; 6] = [
    [8733, 0, 0],
    [16138, 0, 0],
    [17429, 0, 0],
    [24382, 0, 0],
    [20546, 0, 0],
    [28092, 0, 0],
];

/// C `default_zeromv_cdf` (cabac_context_model.c:352) — the table
/// `FrameContext` calls `globalmv_cdf`, which holds a UNIFORM placeholder.
#[rustfmt::skip]
pub static ZEROMV_CDF: [[AomCdfProb; 3]; 2] = [
    [30593, 0, 0],
    [31714, 0, 0],
];

/// C `default_refmv_cdf` (cabac_context_model.c:356).
/// `FrameContext::refmv_cdf` holds a UNIFORM placeholder.
#[rustfmt::skip]
pub static REFMV_CDF: [[AomCdfProb; 3]; 6] = [
    [8794, 0, 0],
    [8580, 0, 0],
    [14920, 0, 0],
    [4146, 0, 0],
    [8456, 0, 0],
    [12845, 0, 0],
];

/// C `default_drl_cdf` (cabac_context_model.c:360).
/// `FrameContext::drl_cdf` holds a UNIFORM placeholder.
#[rustfmt::skip]
pub static DRL_CDF: [[AomCdfProb; 3]; 3] = [
    [19664, 0, 0],
    [8208, 0, 0],
    [13823, 0, 0],
];

/// C `default_inter_compound_mode_cdf` (cabac_context_model.c:364).
/// `FrameContext::inter_compound_mode_cdf` holds a UNIFORM placeholder.
#[rustfmt::skip]
pub static INTER_COMPOUND_MODE_CDF: [[AomCdfProb; 9]; 8] = [
    [25008, 18945, 16960, 15127, 13612, 12102, 5877, 0, 0],
    [22038, 13316, 11623, 10019, 8729, 7637, 4044, 0, 0],
    [22104, 12547, 11180, 9862, 8473, 7381, 4332, 0, 0],
    [19470, 15784, 12297, 8586, 7701, 7032, 6346, 0, 0],
    [13864, 9443, 7526, 5336, 4870, 4510, 2010, 0, 0],
    [22043, 15314, 12644, 9948, 8573, 7600, 6722, 0, 0],
    [15643, 8495, 6954, 5276, 4554, 4064, 2176, 0, 0],
    [19722, 9554, 8263, 6826, 5333, 4326, 3438, 0, 0],
];

/// C `default_switchable_interp_cdf` (cabac_context_model.c:714).
/// `FrameContext::interp_filter_cdf` holds a UNIFORM placeholder.
#[rustfmt::skip]
pub static SWITCHABLE_INTERP_CDF: [[AomCdfProb; 4]; 16] = [
    [833, 48, 0, 0],
    [27200, 49, 0, 0],
    [32346, 29830, 0, 0],
    [4524, 160, 0, 0],
    [1562, 815, 0, 0],
    [27906, 647, 0, 0],
    [31998, 31616, 0, 0],
    [11879, 7131, 0, 0],
    [858, 44, 0, 0],
    [28648, 56, 0, 0],
    [32463, 30521, 0, 0],
    [5365, 132, 0, 0],
    [1746, 759, 0, 0],
    [29805, 675, 0, 0],
    [32167, 31825, 0, 0],
    [17799, 11370, 0, 0],
];

/// C `default_motion_mode_cdf` (cabac_context_model.c:425),
/// `[BLOCK_SIZES_ALL][CDF_SIZE(MOTION_MODES)]`. NOT present in `FrameContext`.
#[rustfmt::skip]
pub static MOTION_MODE_CDF: [[AomCdfProb; 4]; 22] = [
    [21845, 10923, 0, 0],
    [21845, 10923, 0, 0],
    [21845, 10923, 0, 0],
    [25117, 8008, 0, 0],
    [28030, 8003, 0, 0],
    [27377, 7240, 0, 0],
    [13349, 5958, 0, 0],
    [27645, 9162, 0, 0],
    [21162, 8460, 0, 0],
    [6508, 3652, 0, 0],
    [12408, 4706, 0, 0],
    [11089, 5938, 0, 0],
    [3252, 2067, 0, 0],
    [3870, 2371, 0, 0],
    [1890, 1433, 0, 0],
    [261, 210, 0, 0],
    [21845, 10923, 0, 0],
    [21845, 10923, 0, 0],
    [3969, 1378, 0, 0],
    [6337, 1994, 0, 0],
    [3795, 1174, 0, 0],
    [3026, 1565, 0, 0],
];

/// C `default_obmc_cdf` (cabac_context_model.c:434). NOT present in
/// `FrameContext`.
#[rustfmt::skip]
pub static OBMC_CDF: [[AomCdfProb; 3]; 22] = [
    [16384, 0, 0],
    [16384, 0, 0],
    [16384, 0, 0],
    [22331, 0, 0],
    [23397, 0, 0],
    [23467, 0, 0],
    [15336, 0, 0],
    [18345, 0, 0],
    [17626, 0, 0],
    [6951, 0, 0],
    [9945, 0, 0],
    [10685, 0, 0],
    [2640, 0, 0],
    [1754, 0, 0],
    [1208, 0, 0],
    [130, 0, 0],
    [16384, 0, 0],
    [16384, 0, 0],
    [9104, 0, 0],
    [11867, 0, 0],
    [8760, 0, 0],
    [5889, 0, 0],
];

/// C `default_compound_idx_cdfs` (cabac_context_model.c:602). NOT present in
/// `FrameContext`.
#[rustfmt::skip]
pub static COMPOUND_INDEX_CDF: [[AomCdfProb; 3]; 6] = [
    [14524, 0, 0],
    [19903, 0, 0],
    [25715, 0, 0],
    [19509, 0, 0],
    [23434, 0, 0],
    [28124, 0, 0],
];

/// C `default_comp_group_idx_cdfs` (cabac_context_model.c:606). NOT present in
/// `FrameContext`.
#[rustfmt::skip]
pub static COMP_GROUP_IDX_CDF: [[AomCdfProb; 3]; 6] = [
    [6161, 0, 0],
    [9877, 0, 0],
    [13928, 0, 0],
    [8174, 0, 0],
    [12834, 0, 0],
    [10094, 0, 0],
];

/// C `default_interintra_cdf` (cabac_context_model.c:375). NOT present in
/// `FrameContext`.
#[rustfmt::skip]
pub static INTERINTRA_CDF: [[AomCdfProb; 3]; 4] = [
    [16384, 0, 0],
    [5881, 0, 0],
    [5171, 0, 0],
    [2531, 0, 0],
];

/// C `default_interintra_mode_cdf` (cabac_context_model.c:379). NOT present in
/// `FrameContext`.
#[rustfmt::skip]
pub static INTERINTRA_MODE_CDF: [[AomCdfProb; 5]; 4] = [
    [24576, 16384, 8192, 0, 0],
    [30893, 21686, 5436, 0, 0],
    [30295, 22772, 6380, 0, 0],
    [28530, 21231, 6842, 0, 0],
];

/// C `default_wedge_interintra_cdf` (cabac_context_model.c:386). NOT present in
/// `FrameContext`.
#[rustfmt::skip]
pub static WEDGE_INTERINTRA_CDF: [[AomCdfProb; 3]; 22] = [
    [16384, 0, 0],
    [16384, 0, 0],
    [16384, 0, 0],
    [12732, 0, 0],
    [7811, 0, 0],
    [6064, 0, 0],
    [5238, 0, 0],
    [3204, 0, 0],
    [3324, 0, 0],
    [5896, 0, 0],
    [16384, 0, 0],
    [16384, 0, 0],
    [16384, 0, 0],
    [16384, 0, 0],
    [16384, 0, 0],
    [16384, 0, 0],
    [16384, 0, 0],
    [16384, 0, 0],
    [16384, 0, 0],
    [16384, 0, 0],
    [16384, 0, 0],
    [16384, 0, 0],
];

/// C `default_wedge_idx_cdf` (cabac_context_model.c:400). NOT present in
/// `FrameContext`.
#[rustfmt::skip]
pub static WEDGE_IDX_CDF: [[AomCdfProb; 17]; 22] = [
    [30720, 28672, 26624, 24576, 22528, 20480, 18432, 16384, 14336, 12288, 10240, 8192, 6144, 4096, 2048, 0, 0],
    [30720, 28672, 26624, 24576, 22528, 20480, 18432, 16384, 14336, 12288, 10240, 8192, 6144, 4096, 2048, 0, 0],
    [30720, 28672, 26624, 24576, 22528, 20480, 18432, 16384, 14336, 12288, 10240, 8192, 6144, 4096, 2048, 0, 0],
    [30330, 28328, 26169, 24105, 21763, 19894, 17017, 14674, 12409, 10406, 8641, 7066, 5016, 3318, 1597, 0, 0],
    [31962, 29502, 26763, 26030, 25550, 25401, 24997, 18180, 16445, 15401, 14316, 13346, 9929, 6641, 3139, 0, 0],
    [29989, 29030, 28085, 25555, 24993, 24751, 24113, 18411, 14829, 11436, 8248, 5298, 3312, 2239, 1112, 0, 0],
    [31084, 29143, 27093, 25660, 23466, 21494, 18339, 15624, 13605, 11807, 9884, 8297, 6049, 4054, 1891, 0, 0],
    [31626, 29277, 26491, 25454, 24679, 24413, 23745, 19144, 17399, 16038, 14654, 13455, 10247, 6756, 3218, 0, 0],
    [30026, 28573, 27041, 24733, 23788, 23432, 22622, 18644, 15498, 12235, 9334, 6796, 4824, 3198, 1352, 0, 0],
    [31041, 28820, 26667, 24972, 22927, 20424, 17002, 13824, 12130, 10730, 8805, 7457, 5780, 4002, 1756, 0, 0],
    [30720, 28672, 26624, 24576, 22528, 20480, 18432, 16384, 14336, 12288, 10240, 8192, 6144, 4096, 2048, 0, 0],
    [30720, 28672, 26624, 24576, 22528, 20480, 18432, 16384, 14336, 12288, 10240, 8192, 6144, 4096, 2048, 0, 0],
    [30720, 28672, 26624, 24576, 22528, 20480, 18432, 16384, 14336, 12288, 10240, 8192, 6144, 4096, 2048, 0, 0],
    [30720, 28672, 26624, 24576, 22528, 20480, 18432, 16384, 14336, 12288, 10240, 8192, 6144, 4096, 2048, 0, 0],
    [30720, 28672, 26624, 24576, 22528, 20480, 18432, 16384, 14336, 12288, 10240, 8192, 6144, 4096, 2048, 0, 0],
    [30720, 28672, 26624, 24576, 22528, 20480, 18432, 16384, 14336, 12288, 10240, 8192, 6144, 4096, 2048, 0, 0],
    [30720, 28672, 26624, 24576, 22528, 20480, 18432, 16384, 14336, 12288, 10240, 8192, 6144, 4096, 2048, 0, 0],
    [30720, 28672, 26624, 24576, 22528, 20480, 18432, 16384, 14336, 12288, 10240, 8192, 6144, 4096, 2048, 0, 0],
    [32614, 31781, 30843, 30717, 30680, 30657, 30617, 9735, 9065, 8484, 7783, 7084, 5509, 3885, 1857, 0, 0],
    [31633, 31446, 31275, 30133, 30072, 30031, 29998, 11752, 9833, 7711, 5517, 3595, 2679, 1808, 835, 0, 0],
    [30720, 28672, 26624, 24576, 22528, 20480, 18432, 16384, 14336, 12288, 10240, 8192, 6144, 4096, 2048, 0, 0],
    [30720, 28672, 26624, 24576, 22528, 20480, 18432, 16384, 14336, 12288, 10240, 8192, 6144, 4096, 2048, 0, 0],
];

/// C `default_compound_type_cdf` (cabac_context_model.c). NOT present in
/// `FrameContext`.
#[rustfmt::skip]
pub static COMPOUND_TYPE_CDF: [[AomCdfProb; 3]; 22] = [
    [16384, 0, 0],
    [16384, 0, 0],
    [16384, 0, 0],
    [9337, 0, 0],
    [19597, 0, 0],
    [21298, 0, 0],
    [22998, 0, 0],
    [23668, 0, 0],
    [24535, 0, 0],
    [26596, 0, 0],
    [16384, 0, 0],
    [16384, 0, 0],
    [16384, 0, 0],
    [16384, 0, 0],
    [16384, 0, 0],
    [16384, 0, 0],
    [16384, 0, 0],
    [16384, 0, 0],
    [20948, 0, 0],
    [25067, 0, 0],
    [16384, 0, 0],
    [16384, 0, 0],
];
