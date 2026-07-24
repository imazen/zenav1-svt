# 16. Data Structures

This chapter documents the core data structures used throughout the SVT-AV1 encoder. These types represent the AV1 bitstream elements, encoder state, and pipeline objects that a port must faithfully reproduce.

---

## 16.1 AV1 Specification Enumerations (`definitions.h`)

### 16.1.1 BlockSize

```c
typedef enum ATTRIBUTE_PACKED {
    BLOCK_4X4,        // 0
    BLOCK_4X8,        // 1
    BLOCK_8X4,        // 2
    BLOCK_8X8,        // 3
    BLOCK_8X16,       // 4
    BLOCK_16X8,       // 5
    BLOCK_16X16,      // 6
    BLOCK_16X32,      // 7
    BLOCK_32X16,      // 8
    BLOCK_32X32,      // 9
    BLOCK_32X64,      // 10
    BLOCK_64X32,      // 11
    BLOCK_64X64,      // 12
    BLOCK_64X128,     // 13
    BLOCK_128X64,     // 14
    BLOCK_128X128,    // 15
    BLOCK_4X16,       // 16
    BLOCK_16X4,       // 17
    BLOCK_8X32,       // 18
    BLOCK_32X8,       // 19
    BLOCK_16X64,      // 20
    BLOCK_64X16,      // 21
    BLOCK_SIZES_ALL,  // 22
    BLOCK_SIZES = BLOCK_4X16,  // 16 (square + rectangular, no extended)
    BLOCK_INVALID = 255,
    BLOCK_LARGEST = BLOCK_128X128
} BlockSize;
```

22 block sizes total: 6 square (4x4 to 128x128), 10 rectangular (2:1 and 1:2 aspect ratios), 6 extended rectangular (4:1 and 1:4 aspect ratios).

### 16.1.2 PartitionType

```c
typedef enum ATTRIBUTE_PACKED {
    PARTITION_NONE,       // No split
    PARTITION_HORZ,       // Horizontal split into 2
    PARTITION_VERT,       // Vertical split into 2
    PARTITION_SPLIT,      // Quad-split into 4
    PARTITION_HORZ_A,     // Top-left + top-right + bottom
    PARTITION_HORZ_B,     // Top + bottom-left + bottom-right
    PARTITION_VERT_A,     // Top-left + bottom-left + right
    PARTITION_VERT_B,     // Left + top-right + bottom-right
    PARTITION_HORZ_4,     // Horizontal split into 4
    PARTITION_VERT_4,     // Vertical split into 4
    EXT_PARTITION_TYPES,  // 10
    PARTITION_INVALID = 255
} PartitionType;
```

### 16.1.3 Part (Encoder Internal Shape)

```c
typedef enum ATTRIBUTE_PACKED {
    PART_N,   // No split (square)
    PART_H,   // Horizontal (2 blocks)
    PART_V,   // Vertical (2 blocks)
    PART_H4,  // Horizontal 4-way
    PART_V4,  // Vertical 4-way
    PART_HA,  // Horizontal-A (asymmetric)
    PART_HB,  // Horizontal-B (asymmetric)
    PART_VA,  // Vertical-A (asymmetric)
    PART_VB,  // Vertical-B (asymmetric)
    PART_S    // Split (quad)
} Part;
```

The encoder uses `Part` internally for mode decision traversal; it maps to `PartitionType` for bitstream writing.

### 16.1.4 TxSize

```c
typedef enum ATTRIBUTE_PACKED {
    TX_4X4,    TX_8X8,    TX_16X16,  TX_32X32,  TX_64X64,  // Square
    TX_4X8,    TX_8X4,    TX_8X16,   TX_16X8,               // 2:1 / 1:2
    TX_16X32,  TX_32X16,  TX_32X64,  TX_64X32,              // 2:1 / 1:2 (large)
    TX_4X16,   TX_16X4,   TX_8X32,   TX_32X8,               // 4:1 / 1:4
    TX_16X64,  TX_64X16,                                     // 4:1 / 1:4 (large)
    TX_SIZES_ALL,  // 19
    TX_SIZES = TX_4X8,     // 5 (square sizes only)
    TX_SIZES_LARGEST = TX_64X64,
    TX_INVALID = 255,
} TxSize;
```

### 16.1.5 TxType

```c
typedef enum ATTRIBUTE_PACKED {
    DCT_DCT,       ADST_DCT,      DCT_ADST,      ADST_ADST,
    FLIPADST_DCT,  DCT_FLIPADST,  FLIPADST_FLIPADST, ADST_FLIPADST,
    FLIPADST_ADST, IDTX,          V_DCT,          H_DCT,
    V_ADST,        H_ADST,        V_FLIPADST,     H_FLIPADST,
    TX_TYPES,  // 16
} TxType;
```

16 transform types combining DCT, ADST, FLIPADST, and identity in horizontal/vertical directions. Grouped into sets by `TxSetType`:

| Set | Count | Types |
|-----|-------|-------|
| `EXT_TX_SET_DCTONLY` | 1 | DCT_DCT |
| `EXT_TX_SET_DCT_IDTX` | 2 | DCT_DCT, IDTX |
| `EXT_TX_SET_DTT4_IDTX` | 5 | DCT, ADST, FLIPADST pairs + IDTX |
| `EXT_TX_SET_DTT4_IDTX_1DDCT` | 7 | Above + V_DCT, H_DCT |
| `EXT_TX_SET_DTT9_IDTX_1DDCT` | 12 | All except V/H ADST/FLIPADST |
| `EXT_TX_SET_ALL16` | 16 | All types |

### 16.1.6 PredictionMode

```c
typedef enum ATTRIBUTE_PACKED {
    // Intra modes
    DC_PRED,     V_PRED,      H_PRED,      D45_PRED,
    D135_PRED,   D113_PRED,   D157_PRED,   D203_PRED,
    D67_PRED,    SMOOTH_PRED, SMOOTH_V_PRED, SMOOTH_H_PRED,
    PAETH_PRED,
    INTRA_MODES,          // 13
    INTRA_MODE_END = INTRA_MODES,

    // Inter modes (single reference)
    NEARESTMV = INTRA_MODE_END,
    NEARMV,    GLOBALMV,   NEWMV,
    // Compound modes
    NEAREST_NEARESTMV,    NEAR_NEARMV,
    NEAREST_NEWMV,        NEW_NEARESTMV,
    NEAR_NEWMV,           NEW_NEARMV,
    GLOBAL_GLOBALMV,      NEW_NEWMV,
    MB_MODE_COUNT,         // 25
    INTRA_INVALID = 255,
} PredictionMode;
```

### 16.1.7 MvReferenceFrame

```c
typedef enum ATTRIBUTE_PACKED {
    NONE_FRAME   = -1,
    INTRA_FRAME  = 0,
    LAST_FRAME   = 1,
    LAST2_FRAME  = 2,
    LAST3_FRAME  = 3,
    GOLDEN_FRAME = 4,
    BWDREF_FRAME = 5,
    ALTREF2_FRAME = 6,
    ALTREF_FRAME  = 7,
    REF_FRAMES,          // 8
    ...
} MvReferenceFrame;
```

### 16.1.8 InterpFilter

```c
typedef enum {
    EIGHTTAP_REGULAR,
    EIGHTTAP_SMOOTH,
    MULTITAP_SHARP,
    BILINEAR,
    SWITCHABLE_FILTERS,  // 4
    SWITCHABLE = SWITCHABLE_FILTERS,
    EXTRA_FILTERS,
} InterpFilter;
```

### 16.1.9 FrameType / SliceType

```c
typedef enum FrameType {
    KEY_FRAME,
    INTER_FRAME,
    INTRA_ONLY_FRAME,
    S_FRAME,
} FrameType;

typedef enum ATTRIBUTE_PACKED {
    I_SLICE,
    B_SLICE,
    P_SLICE,
    INVALID_SLICE,
} SliceType;
```

### 16.1.10 EbPtrType

```c
typedef enum EbPtrType {
    EB_N_PTR,        // malloc'd
    EB_C_PTR,        // calloc'd
    EB_A_PTR,        // aligned malloc
    EB_MUTEX,        // mutex handle
    EB_SEMAPHORE,    // semaphore handle
    EB_THREAD,       // thread handle
    EB_PTR_TYPE_TOTAL,
} EbPtrType;
```

Used by debug memory tracking to classify allocations.

---

## 16.2 Bitstream Structures (`av1_structs.h`)

### 16.2.1 ObuHeader

```c
typedef struct ObuHeader {
    size_t  size;               // 1 or 2 bytes
    uint8_t obu_forbidden_bit;  // Must be 0
    ObuType obu_type;           // OBU_SEQUENCE_HEADER, OBU_FRAME, etc.
    uint8_t obu_extension_flag;
    uint8_t obu_has_size_field;
    uint8_t temporal_id;
    uint8_t spatial_id;
    size_t  payload_size;
} ObuHeader;
```

OBU types: `OBU_SEQUENCE_HEADER` (1), `OBU_TEMPORAL_DELIMITER` (2), `OBU_FRAME_HEADER` (3), `OBU_TILE_GROUP` (4), `OBU_METADATA` (5), `OBU_FRAME` (6), `OBU_REDUNDANT_FRAME_HEADER` (7), `OBU_PADDING` (15).

### 16.2.2 SeqHeader

The sequence header contains all codec-level configuration:

| Field | Type | Description |
|-------|------|-------------|
| `seq_profile` | EbAv1SeqProfile | Profile (0=Main, 1=High, 2=Professional) |
| `still_picture` | uint8_t | Single-frame sequence flag |
| `reduced_still_picture_header` | uint8_t | Omit syntax elements not needed for stills |
| `timing_info` | EbTimingInfo | Frame rate information |
| `decoder_model_info_present_flag` | uint8_t | HRD model present |
| `decoder_model_info` | DecoderModelInfo | Buffer delay lengths |
| `operating_points_cnt_minus_1` | uint8_t | Number of operating points - 1 |
| `operating_point[MAX_NUM_OPERATING_POINTS]` | EbAv1OperatingPoint | Per-OP parameters |
| `frame_width_bits` / `frame_height_bits` | uint8_t | Bits for frame dimensions |
| `max_frame_width` / `max_frame_height` | uint16_t | Maximum dimensions |
| `frame_id_numbers_present_flag` | uint8_t | Frame ID syntax present |
| `use_128x128_superblock` | uint8_t | SB size selection |
| `sb_size` | BlockSize | BLOCK_64X64 or BLOCK_128X128 |
| `sb_mi_size` | uint8_t | SB size in MI units (16 or 32) |
| `sb_size_log2` | uint8_t | log2(SB size) |
| `filter_intra_level` | uint8_t | Filter intra enabled |
| `enable_intra_edge_filter` | uint8_t | Intra edge filtering |
| `enable_interintra_compound` | uint8_t | Inter-intra compound |
| `enable_masked_compound` | uint8_t | Masked compound |
| `enable_warped_motion` | uint8_t | Warped motion |
| `enable_dual_filter` | uint8_t | Independent H/V filter types |
| `order_hint_info` | OrderHintInfo | Order hint configuration |
| `seq_force_screen_content_tools` | uint8_t | Screen content tools control |
| `seq_force_integer_mv` | uint8_t | Integer MV control |
| `enable_superres` | uint8_t | Super-resolution enabled |
| `cdef_level` | uint8_t | CDEF filtering level |
| `enable_restoration` | uint8_t | Loop restoration enabled |
| `color_config` | EbColorConfig | Color space, bit depth, subsampling |
| `film_grain_params_present` | uint8_t | Film grain parameters present |

### 16.2.3 OrderHintInfo

```c
typedef struct OrderHintInfo {
    uint8_t enable_order_hint;     // Enable order-hint-based tools
    uint8_t enable_jnt_comp;       // Distance-weighted compound
    uint8_t enable_ref_frame_mvs;  // Temporal MV prediction
    uint8_t order_hint_bits;       // Bits for order hint
} OrderHintInfo;
```

### 16.2.4 FrameSize

```c
typedef struct FrameSize {
    uint16_t frame_width;
    uint16_t frame_height;
    uint16_t render_width;
    uint16_t render_height;
    uint8_t  superres_denominator;  // 8..16, 8 = no superres
    uint16_t superres_upscaled_width;
    uint16_t superres_upscaled_height;
} FrameSize;
```

### 16.2.5 TilesInfo

```c
typedef struct TilesInfo {
    uint16_t max_tile_width_sb;
    uint16_t max_tile_height_sb;
    uint8_t  min_log2_tile_cols, max_log2_tile_cols;
    uint8_t  min_log2_tile_rows, max_log2_tile_rows;
    uint8_t  min_log2_tiles;
    uint8_t  uniform_tile_spacing_flag;
    uint8_t  tile_cols, tile_rows;
    uint8_t  tile_cols_log2, tile_rows_log2;
    uint16_t tile_col_start_mi[MAX_TILE_ROWS + 1];
    uint16_t tile_row_start_mi[MAX_TILE_COLS + 1];
    uint16_t context_update_tile_id;
    uint8_t  tile_size_bytes;
} TilesInfo;
```

### 16.2.6 QuantizationParams

```c
typedef struct QuantizationParams {
    uint8_t base_q_idx;            // Base QP (0-255)
    int8_t  delta_q_dc[MAX_PLANES]; // DC delta per plane
    int8_t  delta_q_ac[MAX_PLANES]; // AC delta per plane
    uint8_t using_qmatrix;
    uint8_t qm[MAX_PLANES];        // QMatrix level per plane
    uint8_t qindex[MAX_SEGMENTS];   // Per-segment QP
} QuantizationParams;
```

### 16.2.7 CdefParams

```c
typedef struct CdefParams {
    uint8_t cdef_damping;                        // Damping factor
    uint8_t cdef_bits;                           // Bits for filter index
    uint8_t cdef_y_strength[CDEF_MAX_STRENGTHS];  // Y primary+secondary strength
    uint8_t cdef_uv_strength[CDEF_MAX_STRENGTHS]; // UV primary+secondary strength
} CdefParams;
```

### 16.2.8 LrParams (Loop Restoration)

```c
typedef struct LrParams {
    RestorationType frame_restoration_type; // RESTORE_NONE, _WIENER, _SGRPROJ, _SWITCHABLE
    uint16_t loop_restoration_size;
    uint8_t  lr_size_log2;
} LrParams;
```

### Source Implementation Files

| File | Role |
|------|------|
| `Source/Lib/Codec/block_structures.c` | Tile setup helpers: `svt_av1_tile_set_row` and `svt_av1_tile_set_col` compute MI-unit row/column boundaries for `TileInfo` from `TilesInfo` |
| `Source/Lib/Codec/coding_unit.c` | SuperBlock constructor/destructor (`svt_aom_largest_coding_unit_dctor`), partition tree (`PARTITION_TREE`) recursive setup, `BlkStruct`/`EcBlkStruct` array allocation |
| `Source/Lib/Codec/reference_object.c` | `EbReferenceObject`, `EbPaReferenceObject`, and `EbTplReferenceObject` constructors/destructors, reference picture buffer allocation, border sample initialization, downscaled reference management with per-scale mutexes |

---

## 16.3 Tile Information (`block_structures.h`)

### 16.3.1 TileInfo

```c
typedef struct TileInfo {
    int32_t mi_row_start, mi_row_end;
    int32_t mi_col_start, mi_col_end;
    int32_t tg_horz_boundary;  // Tile group horizontal boundary flag
    int32_t tile_row, tile_col;
    int32_t tile_rs_index;     // Tile index in raster order
} TileInfo;
```

---

## 16.4 Block Mode Information (`block_structures.h`)

### 16.4.1 BlockModeInfo

The core per-block mode information, written to the bitstream:

```c
typedef struct BlockModeInfo {
    // Prediction
    PredictionMode   mode;           // Luma prediction mode
    UvPredictionMode uv_mode;        // Chroma prediction mode (intra only)

    // Inter mode fields
    Mv               mv[2];          // Motion vectors (unipred: [0], bipred: [0]+[1])
    MvReferenceFrame ref_frame[2];   // Reference frames
    uint32_t         interp_filters; // Packed H+V interpolation filters
    InterInterCompoundData interinter_comp;  // Compound mode data
    MotionMode       motion_mode;    // SIMPLE, OBMC, or WARPED
    uint8_t          num_proj_ref;   // Samples for warp estimation
    InterIntraMode   interintra_mode;
    int8_t           interintra_wedge_index;

    // Intra mode fields
    int8_t  angle_delta[PLANE_TYPES]; // Angle delta for directional modes
    uint8_t filter_intra_mode;        // Filter intra mode type
    uint8_t cfl_alpha_signs;          // CFL joint sign
    uint8_t cfl_alpha_idx;            // CFL alpha index

    // Transform
    uint8_t tx_depth;                 // Transform depth (0-2)

    // Flags (bitfields)
    uint8_t is_interintra_used : 1;
    uint8_t use_wedge_interintra : 1;
    uint8_t comp_group_idx : 1;       // Compound group index
    uint8_t compound_idx : 1;         // 0=distance-weighted, 1=average
    uint8_t skip : 1;                 // Skip coefficients
    uint8_t skip_mode : 1;            // Skip mode info + coefficients
    uint8_t use_intrabc : 1;          // IntraBC mode
} BlockModeInfo;
```

### 16.4.2 MbModeInfo

Extended mode info stored per MI-unit in the frame grid:

```c
typedef struct MbModeInfo {
    BlockModeInfo       block_mi;
    BlockSize           bsize;
    PartitionType       partition;
    uint8_t             segment_id;
    PaletteLumaModeInfo palette_mode_info;
    int8_t              cdef_strength;
} MbModeInfo;
```

### 16.4.3 Helper Functions

- `has_second_ref(block_mi)` -- True if `ref_frame[1] > INTRA_FRAME`
- `has_uni_comp_refs(block_mi)` -- True if both refs are same direction
- `is_intrabc_block(block_mi)` -- True if `use_intrabc`
- `is_inter_block(block_mi)` -- True if intrabc or `ref_frame[0] > INTRA_FRAME`

---

## 16.5 Coding Unit (`coding_unit.h`)

### 16.5.1 Motion Vector Types

```c
typedef struct TPL_MV_REF {
    Mv      mfmv0;
    uint8_t ref_frame_offset;
} TPL_MV_REF;

typedef struct MV_REF {
    Mv               mv;
    MvReferenceFrame ref_frame;
} MV_REF;
```

### 16.5.2 MacroBlockD

Decoder-side macroblock context, used during reconstruction:

```c
typedef struct MacroBlockD {
    uint8_t      n8_w, n8_h;         // Block size in 8x8 units
    uint8_t      n4_w, n4_h;         // Block size in 4x4 units (for warp)
    uint8_t      ref_mv_count[MODE_CTX_REF_FRAMES];
    uint8_t      is_sec_rect;        // Secondary rectangle flag
    int8_t       up_available, left_available;
    int8_t       chroma_up_available, chroma_left_available;
    TileInfo     tile;
    int32_t      mi_stride;
    MbModeInfo** mi;                 // Pointer into MI grid

    // Distance from frame edges (in 1/8 pixel units)
    int32_t mb_to_left_edge, mb_to_right_edge;
    int32_t mb_to_top_edge, mb_to_bottom_edge;

    int     mi_row, mi_col;          // Position in MI units
    uint8_t neighbors_ref_counts[TOTAL_REFS_PER_FRAME];

    // Neighbor block pointers
    MbModeInfo* above_mbmi, *left_mbmi;
    MbModeInfo* chroma_above_mbmi, *chroma_left_mbmi;

    FRAME_CONTEXT* tile_ctx;         // Entropy coding context
    TXFM_CONTEXT*  above_txfm_context;
    TXFM_CONTEXT*  left_txfm_context;
    BlockSize      bsize;
} MacroBlockD;
```

### 16.5.3 BlkStruct

The encoder's per-block state during mode decision:

```c
typedef struct BlkStruct {
    MacroBlockD* av1xd;

    // Neighbor reconstruction buffers (for intra prediction)
    uint8_t*  neigh_left_recon[3];     // Per plane
    uint8_t*  neigh_top_recon[3];
    uint16_t* neigh_left_recon_16bit[3];
    uint16_t* neigh_top_recon_16bit[3];

    // Coefficient and recon buffers (when EncDec is bypassed)
    EbPictureBufferDesc* coeff_tmp;
    EbPictureBufferDesc* recon_tmp;

    // RD cost
    uint64_t cost;
    uint64_t total_rate;
    uint64_t full_dist;

    // Quantized DC indicator
    QuantDcData quant_dc;

    // End-of-block positions
    EobData eob;                       // Per TX block per plane

    // Transform types
    TxType tx_type[MAX_TXB_COUNT];     // Per TX block (luma)
    TxType tx_type_uv;                 // UV transform type

    // Has-coefficient flags
    uint16_t y_has_coeff;              // Bitmask per luma TX block
    uint8_t  u_has_coeff, v_has_coeff;

    // Palette
    PaletteInfo* palette_info;
    uint8_t      palette_mem;
    uint8_t      palette_size[2];      // [luma, chroma]

    // Mode information
    BlockModeInfo block_mi;
    Mv            predmv[2];           // Predicted MVs

    uint32_t overlappable_neighbors;
    int16_t  inter_mode_ctx;
    uint16_t mds_idx;                  // Mode decision scan index

    uint8_t qindex;
    uint8_t drl_index;                 // Dynamic reference list index
    int8_t  drl_ctx[2];               // DRL context
    int8_t  drl_ctx_near[2];
    uint8_t segment_id;

    // Warped motion parameters
    WarpedMotionParams wm_params_l0;
    WarpedMotionParams wm_params_l1;

    unsigned cnt_nz_coeff : 12;        // Non-zero coefficient count
    unsigned block_has_coeff : 1;      // Skip coeff flag
} BlkStruct;
```

### 16.5.4 EcBlkStruct

Reduced block structure passed to entropy coding (EC). Contains only the fields EC needs:

```c
typedef struct EcBlkStruct {
    MacroBlockD* av1xd;
    EobData      eob;
    TxType       tx_type[MAX_TXB_COUNT];
    TxType       tx_type_uv;
    PaletteInfo* palette_info;
    uint8_t      palette_size[2];
    Mv           predmv[2];
    uint32_t     overlappable_neighbors;
    int16_t      inter_mode_ctx;
    uint16_t     mds_idx;
    uint8_t      qindex;
    uint8_t      drl_index;
    int8_t       drl_ctx[2];
    int8_t       drl_ctx_near[2];
} EcBlkStruct;
```

### 16.5.5 EobData / QuantDcData

```c
typedef struct EobData {
    uint16_t y[MAX_TXB_COUNT];     // EOB position per luma TX block
    uint16_t u[MAX_TXB_COUNT_UV];  // EOB per U TX block
    uint16_t v[MAX_TXB_COUNT_UV];  // EOB per V TX block
} EobData;

typedef struct QuantDcData {
    uint8_t y[MAX_TXB_COUNT];
    uint8_t u[MAX_TXB_COUNT_UV];
    uint8_t v[MAX_TXB_COUNT_UV];
} QuantDcData;
```

### 16.5.6 SuperBlock

The top-level coding unit representing one SB (64x64 or 128x128):

```c
typedef struct SuperBlock {
    EbDctor                   dctor;
    struct PictureControlSet* pcs;
    EcBlkStruct*              final_blk_arr;  // Array of final block decisions
    MacroBlockD*              av1xd;
    struct PARTITION_TREE*    ptree;           // Partition tree

    unsigned index : 32;
    unsigned org_x : 32;                      // SB origin X (pixels)
    unsigned org_y : 32;                      // SB origin Y (pixels)
    uint8_t  qindex;
    TileInfo tile_info;
    uint16_t final_blk_cnt;                   // Blocks posted to EC
} SuperBlock;
```

### 16.5.7 TplStats / TplSrcStats

TPL (Temporal Prediction Layer) statistics per block:

```c
typedef struct TplStats {
    int64_t  srcrf_dist;     // Source reference frame distortion
    int64_t  recrf_dist;     // Recon reference frame distortion
    int64_t  srcrf_rate;     // Source reference frame rate
    int64_t  recrf_rate;     // Recon reference frame rate
    int64_t  mc_dep_rate;    // Motion-compensated dependency rate
    int64_t  mc_dep_dist;    // Motion-compensated dependency distortion
    Mv       mv;
    uint64_t ref_frame_poc;
} TplStats;

typedef struct TplSrcStats {
    int64_t        srcrf_dist;
    int64_t        srcrf_rate;
    uint64_t       ref_frame_poc;
    Mv             mv;
    uint8_t        best_mode;
    int32_t        best_rf_idx;
    PredictionMode best_intra_mode;
} TplSrcStats;
```

### 16.5.8 IntraBcContext

Context for IntraBC (intra block copy) mode:

```c
typedef struct IntraBcContext {
    int32_t           rdmult;
    MacroBlockDPlane  xdplane[MAX_PLANES];
    MacroBlockPlane   plane[MAX_PLANES];
    MvLimits          mv_limits;
    int               sadperbit16;
    int               errorperbit;
    Mv                best_mv;
    Mv                second_best_mv;
    MacroBlockD*      xd;
    int*              nmv_vec_cost;
    const int**       mv_cost_stack;
    uint32_t*         hash_value_buffer[2]; // Ping-pong hash buffers
    CRC32C            crc_calculator;
    uint8_t           approx_inter_rate;
} IntraBcContext;
```

---

## 16.6 Reference Objects (`reference_object.h`)

### 16.6.1 EbReferenceObject

Full-resolution reference frame used during encode (mode decision and reconstruction):

```c
typedef struct EbReferenceObject {
    EbDctor              dctor;
    EbPictureBufferDesc* reference_picture;

    // Multi-scale reference pictures: [super-res scales][resize scales]
    EbPictureBufferDesc* downscaled_reference_picture[NUM_SR_SCALES + 1][NUM_RESIZE_SCALES + 1];
    uint64_t             downscaled_picture_number[NUM_SR_SCALES + 1][NUM_RESIZE_SCALES + 1];
    EbHandle             resize_mutex[NUM_SR_SCALES + 1][NUM_RESIZE_SCALES + 1];

    uint64_t  ref_poc;
    uint8_t   base_q_idx;
    SliceType slice_type;

    // Coding statistics
    uint8_t  intra_coded_area;    // % intra coded (0-100)
    uint8_t  skip_coded_area;
    uint8_t  hp_coded_area;       // High-precision MV area
    uint8_t  is_mfmv_used;
    uint8_t  tmp_layer_idx;
    bool     is_scene_change;
    uint16_t pic_avg_variance;

    // Film grain
    AomFilmGrain film_grain_params;

    // Entropy context from this frame
    FRAME_CONTEXT frame_context;

    // Global motion parameters
    WarpedMotionParams global_motion[TOTAL_REFS_PER_FRAME];

    // Temporal MV buffer
    MV_REF* mvs;

    FrameType frame_type;
    uint32_t  order_hint;
    uint32_t  ref_order_hint[7];
    double    r0;                // TPL r0 value

    // Loop filter state
    int32_t filter_level[2];     // Y filter levels
    int32_t filter_level_u, filter_level_v;
    int32_t dlf_dist_dev;
    int32_t cdef_dist_dev;

    // CDEF strengths
    uint32_t ref_cdef_strengths_num;
    uint8_t  ref_cdef_strengths[2][TOTAL_STRENGTHS];

    // Per-SB statistics from this frame
    uint8_t*  sb_intra;
    uint8_t*  sb_skip;
    uint8_t*  sb_64x64_mvp;
    uint32_t* sb_me_64x64_dist;
    uint32_t* sb_me_8x8_cost_var;
    uint8_t*  sb_min_sq_size;
    uint8_t*  sb_max_sq_size;

    int32_t mi_cols, mi_rows;

    // Wiener filter info for forwarding to future frames
    WienerUnitInfo** unit_info;
} EbReferenceObject;
```

### 16.6.2 EbPaReferenceObject

Pre-analysis reference (used during picture analysis and motion estimation, before full encode):

```c
typedef struct EbPaReferenceObject {
    EbDctor              dctor;
    EbPictureBufferDesc* input_padded_pic;
    EbPictureBufferDesc* quarter_downsampled_picture_ptr;
    EbPictureBufferDesc* sixteenth_downsampled_picture_ptr;

    // Multi-scale variants: [super-res][resize]
    EbPictureBufferDesc* downscaled_input_padded_picture_ptr[NUM_SR_SCALES + 1][NUM_RESIZE_SCALES + 1];
    EbPictureBufferDesc* downscaled_quarter_downsampled_picture_ptr[NUM_SR_SCALES + 1][NUM_RESIZE_SCALES + 1];
    EbPictureBufferDesc* downscaled_sixteenth_downsampled_picture_ptr[NUM_SR_SCALES + 1][NUM_RESIZE_SCALES + 1];
    uint64_t             downscaled_picture_number[NUM_SR_SCALES + 1][NUM_RESIZE_SCALES + 1];
    EbHandle             resize_mutex[NUM_SR_SCALES + 1][NUM_RESIZE_SCALES + 1];

    uint64_t picture_number;
    uint64_t avg_luma;
    uint8_t  dummy_obj;
} EbPaReferenceObject;
```

### 16.6.3 EbTplReferenceObject

TPL (temporal prediction layer) reference:

```c
typedef struct EbTplReferenceObject {
    EbDctor              dctor;
    EbPictureBufferDesc* ref_picture_ptr;
} EbTplReferenceObject;
```

---

## 16.7 AV1 Common State (`av1_common.h`)

### 16.7.1 Av1Common

Shared codec state between encoder and decoder sides:

```c
typedef struct Av1Common {
    int32_t  mi_rows, mi_cols;           // Frame dimensions in MI units
    int32_t  ref_frame_sign_bias[REF_FRAMES];
    uint8_t* last_frame_seg_map;         // Previous frame's segmentation map
    int32_t  mi_stride;

    int32_t use_highbitdepth;
    int32_t bit_depth;
    int32_t color_format;
    int32_t subsampling_x, subsampling_y;

    struct PictureControlSet* child_pcs;

    // Loop restoration output
    Yv12BufferConfig  rst_frame;
    Yv12BufferConfig* frame_to_show;
    int32_t           byte_alignment;

    // Tile dimensions
    int32_t last_tile_cols, last_tile_rows;
    int32_t log2_tile_cols, log2_tile_rows;
    int32_t tile_width, tile_height;     // In MI units

    // Restoration filter controls
    WnFilterCtrls wn_filter_ctrls;       // Wiener filter controls
    SgFilterCtrls sg_filter_ctrls;       // Self-guided filter controls
    uint8_t use_boundaries_in_rest_search;

    FrameSize frm_size;
    TilesInfo tiles_info;
} Av1Common;
```

### 16.7.2 WnFilterCtrls (Wiener Filter Controls)

```c
typedef struct WnFilterCtrls {
    bool    enabled;
    uint8_t filter_tap_lvl;           // 1=Y-7tap/UV-5tap, 2=Y-5tap/UV-5tap, 3=Y-3tap/UV-3tap
    bool    use_refinement;
    bool    max_one_refinement_step;
    bool    use_prev_frame_coeffs;
    bool    use_chroma;
} WnFilterCtrls;
```

### 16.7.3 SgFilterCtrls (Self-Guided Filter Controls)

```c
typedef struct SgFilterCtrls {
    bool   enabled;
    int8_t start_ep[PLANE_TYPES];    // Search start index
    int8_t end_ep[PLANE_TYPES];      // Search end index
    int8_t ep_inc[PLANE_TYPES];      // Search increment
    int8_t refine[PLANE_TYPES];      // Alpha/beta refinement
    bool   use_chroma;
} SgFilterCtrls;
```

---

## 16.8 Object Lifecycle Pattern (`object.h`)

### 16.8.1 Destructor Type

```c
typedef void (*EbDctor)(void* pobj);
```

Every heap-allocated object that needs cleanup has `EbDctor dctor` as its first field.

### 16.8.2 Construction Pattern

```c
EB_NEW(pointer, constructor_function, args...);
```

Expands to:
1. `EB_CALLOC(pointer, 1, sizeof(*pointer))` -- allocate and zero
2. Call `constructor_function(pointer, args...)` -- returns `EbErrorType`
3. On error: `EB_DELETE_UNCHECKED(pointer)` -- call dctor and free
4. On error: `return err` -- propagate error up

### 16.8.3 Destruction Pattern

```c
EB_DELETE(pointer);
```

Expands to: if non-null, call `pointer->dctor(pointer)`, then `EB_FREE(pointer)`.

### 16.8.4 Creator Functions

For objects managed by `EbSystemResource`, a separate "creator" function signature is used:

```c
typedef EbErrorType (*EbCreator)(EbPtr* object_dbl_ptr, EbPtr object_init_data_ptr);
```

These allocate the object internally and return it through the double pointer.

---

## 16.9 System Resource Manager Types (`sys_resource_manager.h`)

### 16.9.1 EbObjectWrapper

```c
typedef struct EbObjectWrapper {
    EbDctor dctor;
    EbDctor object_destroyer;     // Custom destroyer (or NULL for dctor-based)
    void*   object_ptr;           // The managed object
    uint32_t live_count;          // Active references (0 = available, ~0u = released)
    bool     release_enable;      // Whether release is permitted
    struct EbSystemResource* system_resource_ptr;
    struct EbObjectWrapper*  next_ptr;  // FIFO linkage
} EbObjectWrapper;
```

### 16.9.2 EbFifo

```c
typedef struct EbFifo {
    EbDctor dctor;
    EbHandle counting_semaphore;  // Blocks when empty
    EbHandle lockout_mutex;       // Protects list operations
    EbObjectWrapper* first_ptr;   // Head
    EbObjectWrapper* last_ptr;    // Tail
    bool quit_signal;             // Shutdown flag
    struct EbMuxingQueue* queue_ptr;  // Parent muxing queue
} EbFifo;
```

### 16.9.3 EbCircularBuffer

```c
typedef struct EbCircularBuffer {
    EbDctor  dctor;
    EbPtr*   array_ptr;           // Circular array of pointers
    uint32_t head_index;
    uint32_t tail_index;
    uint32_t buffer_total_count;  // Array capacity
    uint32_t current_count;       // Number of items
} EbCircularBuffer;
```

### 16.9.4 EbMuxingQueue

```c
typedef struct EbMuxingQueue {
    EbDctor           dctor;
    EbHandle          lockout_mutex;
    EbCircularBuffer* object_queue;       // Pending objects
    EbCircularBuffer* process_queue;      // Pending consumer FIFOs
    uint32_t          process_total_count;
    EbFifo**          process_fifo_ptr_array;
} EbMuxingQueue;
```

### 16.9.5 EbSystemResource

```c
typedef struct EbSystemResource {
    EbDctor dctor;
    uint32_t object_total_count;
    EbObjectWrapper** wrapper_ptr_pool;  // All wrappers
    EbMuxingQueue*    empty_queue;       // Available objects
    EbMuxingQueue*    full_queue;        // Completed objects (NULL if no consumers)
} EbSystemResource;
```

---

## 16.10 Motion Vector Types

### 16.10.1 Mv (from `mv.h`)

```c
typedef struct Mv {
    int16_t row;  // Vertical component (1/8 pel precision)
    int16_t col;  // Horizontal component (1/8 pel precision)
} Mv;
```

Motion vectors use 1/8-pixel precision (3 fractional bits). The AV1 spec allows 1/4-pixel precision with optional high-precision mode for 1/8-pixel.

### 16.10.2 MvLimits

```c
typedef struct MvLimits {
    int col_min;
    int col_max;
    int row_min;
    int row_max;
} MvLimits;
```

---

## 16.11 Compound Inter-Inter Data

### 16.11.1 InterInterCompoundData

```c
typedef struct InterInterCompoundData {
    uint8_t  wedge_index;
    uint8_t  wedge_sign;
    DIFFWTD_MASK_TYPE mask_type;
    uint8_t  type;            // COMPOUND_AVERAGE, _DISTWTD, _WEDGE, _DIFFWTD
} InterInterCompoundData;
```

### 16.11.2 Compound Types

| Type | Description |
|------|-------------|
| `COMPOUND_AVERAGE` | Simple averaging of two predictions |
| `COMPOUND_DISTWTD` | Distance-weighted blending |
| `COMPOUND_WEDGE` | Wedge-shaped mask |
| `COMPOUND_DIFFWTD` | Difference-weighted mask |

---

## 16.12 Warped Motion Parameters

```c
typedef struct WarpedMotionParams {
    TransformationType type;   // IDENTITY, TRANSLATION, ROTZOOM, AFFINE
    int32_t wmmat[6];          // Affine matrix parameters
    int16_t alpha, beta, gamma, delta;  // Derived shear parameters
    int8_t  invalid;
} WarpedMotionParams;
```

The `wmmat` array encodes the affine transform:
- `wmmat[0]`, `wmmat[1]`: Translation
- `wmmat[2]`, `wmmat[3]`: Scale + rotation
- `wmmat[4]`, `wmmat[5]`: Shear (affine only)

---

## 16.13 Porting Notes

### 16.13.1 Packed Enums

Many enums use `ATTRIBUTE_PACKED` to minimize memory. In a port, these should be stored as the smallest integer type that can represent all values. Most fit in `u8`.

### 16.13.2 Bitfield Packing

`BlockModeInfo` uses bitfields (`skip : 1`, `use_intrabc : 1`, etc.). A port should use explicit bitmask operations or the language's equivalent compact representation.

### 16.13.3 MI Grid

The frame is divided into a grid of 4x4 "mode info" (MI) units. All position calculations use MI coordinates. `mi_stride` is the number of MI columns (including padding).

### 16.13.4 EbPictureBufferDesc

Not fully documented here (see `pic_buffer_desc.h`), but this is the primary pixel buffer type. It holds Y, U, V planes with configurable bit depth, stride, and border padding. Every reference picture, input picture, and reconstruction buffer uses this type.

### 16.13.5 FRAME_CONTEXT

The entropy coding context (CDF tables). This is a large structure (~24KB) containing probability tables for all syntax elements. It is initialized from default tables and updated after each frame (backward update). A port must reproduce the exact initial CDFs and update algorithm.
