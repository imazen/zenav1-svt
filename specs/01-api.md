# SVT-AV1 Public API Specification

## Overview

The SVT-AV1 public API provides a C-based interface for AV1 video encoding. The API follows a strict lifecycle: create handle, configure parameters, initialize encoder, send pictures, receive packets, deinitialize, destroy handle. The API is thread-safe for the send/receive operations (one sender thread, one receiver thread). All public symbols are prefixed with `svt_av1_` or `EB_`/`Eb`.

The API is designed for zero-copy integration where possible: input picture data is referenced by pointer (not copied by default), and output packets are managed by the library's internal buffer pool.

## Source Files

| File | Path | Purpose |
|------|------|---------|
| EbSvtAv1.h | Source/API/EbSvtAv1.h | Core types: EbBufferHeaderType, EbComponentType, EbErrorType, EbSvtIOFormat, film grain, CPU flags, color config |
| EbSvtAv1Enc.h | Source/API/EbSvtAv1Enc.h | Encoder API functions and EbSvtAv1EncConfiguration struct |
| EbSvtAv1Formats.h | Source/API/EbSvtAv1Formats.h | Color format enums: color primaries, transfer characteristics, matrix coefficients, bit depth, chroma format |
| EbSvtAv1ErrorCodes.h | Source/API/EbSvtAv1ErrorCodes.h | Internal error code enums for encoder error reporting |
| EbSvtAv1Metadata.h | Source/API/EbSvtAv1Metadata.h | Metadata types, allocation, and attachment to frames |
| EbConfigMacros.h | Source/API/EbConfigMacros.h | Compile-time feature toggles (quant matrix, OBMC, film grain, high bit depth) |
| EbDebugMacros.h | Source/API/EbDebugMacros.h | Debug/development feature flags |
| app_main.c | Source/App/app_main.c | Reference application: command-line encoder using the API |
| app_context.c | Source/App/app_context.c | Encoder initialization helper: buffer allocation, init_encoder/de_init_encoder |
| app_config.c | Source/App/app_config.c | Command-line parsing, parameter setting via svt_av1_enc_parse_parameter |
| app_config.h | Source/App/app_config.h | Application-level types: EbConfig, EncChannel, EncApp |
| enc_handle.c | Source/Lib/Globals/enc_handle.c | Implementation of API entry points (svt_av1_enc_init, send_picture, get_packet, etc.) |

## Test Coverage

| Test File | What It Tests |
|-----------|---------------|
| test/api_test/SvtAv1EncApiTest.cc | API null-pointer handling, normal setup lifecycle, repeated init/deinit for leak detection |
| test/api_test/SvtAv1EncApiTest.h | SvtAv1Context test helper struct definition |
| test/api_test/SvtAv1EncParamsTest.cc | Per-parameter validation: tests default, valid, and invalid values for each config field |
| test/api_test/params.h | Test vectors: arrays of default/valid/invalid values for enc_mode, intra_period_length, source dimensions, QP, etc. |
| test/api_test/MultiEncoderTest.cc | Multiple concurrent encoder instances |

## Encoding Lifecycle

The encoding lifecycle follows these mandatory steps:

```
1. svt_av1_enc_init_handle()     -- Allocate encoder, get default config
2. [Modify config fields]        -- Set source_width, source_height, enc_mode, etc.
3. svt_av1_enc_set_parameter()   -- Apply configuration to encoder
4. svt_av1_enc_init()            -- Initialize encoder (allocates internal resources, starts threads)
5. [Optional: svt_av1_enc_stream_header() -- Get sequence header before first frame]

   Encoding loop:
   6a. svt_av1_enc_send_picture()  -- Send input frame (or EOS signal)
   6b. svt_av1_enc_get_packet()    -- Receive encoded packet
   6c. svt_av1_enc_release_out_buffer() -- Release packet back to pool
   [Repeat 6a-6c until all frames processed]

7. svt_av1_enc_deinit()          -- Deinitialize encoder (stops threads, waits for completion)
8. svt_av1_enc_deinit_handle()   -- Free encoder handle and all resources
```

### Detailed Flow

**Step 1 - Handle Creation:**
```c
EbComponentType* handle = NULL;
EbSvtAv1EncConfiguration config;
EbErrorType err = svt_av1_enc_init_handle(&handle, &config);
// config is now populated with defaults
```

**Step 2-3 - Configuration:**
```c
config.source_width = 1920;
config.source_height = 1080;
config.enc_mode = 8;  // preset 8
// ... set other fields ...
err = svt_av1_enc_set_parameter(handle, &config);
```

Or use string-based parameter setting:
```c
err = svt_av1_enc_parse_parameter(&config, "enc-mode", "8");
```

**Step 4 - Initialization:**
```c
err = svt_av1_enc_init(handle);
// Encoder is now ready; threads are running
```

**Step 5 (Optional) - Stream Header:**
```c
EbBufferHeaderType* header = NULL;
err = svt_av1_enc_stream_header(handle, &header);
// Write header->p_buffer (header->n_filled_len bytes) to output
err = svt_av1_enc_stream_header_release(header);
```

**Step 6 - Encoding Loop:**
```c
// Sending frames:
EbBufferHeaderType input_buffer;
input_buffer.p_buffer = (uint8_t*)&io_format;  // EbSvtIOFormat with plane pointers
input_buffer.n_filled_len = total_frame_size;
input_buffer.pts = frame_number;
input_buffer.flags = 0;  // or EB_BUFFERFLAG_EOS for last frame
err = svt_av1_enc_send_picture(handle, &input_buffer);

// Receiving packets:
EbBufferHeaderType* output = NULL;
err = svt_av1_enc_get_packet(handle, &output, pic_send_done);
if (err == EB_ErrorNone && output) {
    // Write output->p_buffer (output->n_filled_len bytes) to file
    // Check output->flags for EB_BUFFERFLAG_EOS
    svt_av1_enc_release_out_buffer(&output);
}
```

**Blocking behavior of `svt_av1_enc_get_packet`:**
- If `pic_send_done == 0` and prediction structure is RANDOM_ACCESS: **non-blocking**, returns `EB_NoErrorEmptyQueue` if no packet is ready
- If `pic_send_done == 1`: **blocking**, waits until a packet is available (used after sending EOS)
- If prediction structure is LOW_DELAY: always **blocking**

**Step 7-8 - Cleanup:**
```c
err = svt_av1_enc_deinit(handle);
err = svt_av1_enc_deinit_handle(handle);
```

## Public Functions

### svt_av1_enc_init_handle

```c
EB_API EbErrorType svt_av1_enc_init_handle(
    EbComponentType** p_handle,
    EbSvtAv1EncConfiguration* config_ptr);
```

Allocates the encoder component and populates `config_ptr` with default parameter values. Must be called first.

- `p_handle`: Output pointer to the encoder component handle
- `config_ptr`: Output pointer to configuration struct (filled with defaults)
- Returns `EB_ErrorNone` on success, `EB_ErrorBadParameter` if either pointer is NULL

### svt_av1_enc_set_parameter

```c
EB_API EbErrorType svt_av1_enc_set_parameter(
    EbComponentType* svt_enc_component,
    EbSvtAv1EncConfiguration* pComponentParameterStructure);
```

Copies the configuration into the encoder. Must be called after `svt_av1_enc_init_handle` and before `svt_av1_enc_init`. Validates all parameters.

- Returns `EB_ErrorBadParameter` for invalid parameter values
- Returns `EB_ErrorBadParameter` if either pointer is NULL

### svt_av1_enc_parse_parameter

```c
EB_API EbErrorType svt_av1_enc_parse_parameter(
    EbSvtAv1EncConfiguration* pComponentParameterStructure,
    const char* name,
    const char* value);
```

Sets a single configuration parameter by name and string value. Useful for command-line-style configuration. Can be called multiple times before `svt_av1_enc_set_parameter`.

- `name`: Null-terminated parameter name (e.g., "enc-mode", "source-width")
- `value`: Null-terminated parameter value as string

### svt_av1_enc_init

```c
EB_API EbErrorType svt_av1_enc_init(EbComponentType* svt_enc_component);
```

Initializes the encoder: allocates all internal buffers, creates the processing pipeline, starts all worker threads. After this call, the encoder is ready to receive pictures.

- Returns `EB_ErrorBadParameter` if handle is NULL
- Returns `EB_ErrorInsufficientResources` if memory allocation fails

### svt_av1_enc_stream_header

```c
EB_API EbErrorType svt_av1_enc_stream_header(
    EbComponentType* svt_enc_component,
    EbBufferHeaderType** output_stream_ptr);
```

Optional. Returns the AV1 sequence header as an output buffer. Can be called after `svt_av1_enc_init` and before the first `svt_av1_enc_send_picture`.

### svt_av1_enc_stream_header_release

```c
EB_API EbErrorType svt_av1_enc_stream_header_release(
    EbBufferHeaderType* stream_header_ptr);
```

Releases the buffer returned by `svt_av1_enc_stream_header`.

### svt_av1_enc_send_picture

```c
EB_API EbErrorType svt_av1_enc_send_picture(
    EbComponentType* svt_enc_component,
    EbBufferHeaderType* p_buffer);
```

Sends one input picture to the encoder. The input buffer must contain:
- `p_buffer`: Pointer to an `EbSvtIOFormat` struct containing plane pointers
- `n_filled_len`: Total size of the input data
- `pts`: Presentation timestamp
- `flags`: `EB_BUFFERFLAG_EOS` to signal end of stream

The encoder may reference the input data asynchronously. For memory-mapped inputs, the data is copied internally. For regular inputs with `buffered_input == -1`, the library keeps a reference until the picture is no longer needed.

The `p_app_private` field can point to an `EbPrivDataNode` linked list for per-picture private data (metadata, ROI maps, resolution changes, rate changes, etc.).

### svt_av1_enc_get_packet

```c
EB_API EbErrorType svt_av1_enc_get_packet(
    EbComponentType* svt_enc_component,
    EbBufferHeaderType** p_buffer,
    uint8_t pic_send_done);
```

Retrieves an encoded output packet from the encoder.

- `p_buffer`: Output pointer to the encoded packet
- `pic_send_done`: Set to 1 after the EOS picture has been sent to make this call blocking until the final packet arrives; set to 0 during normal encoding
- Returns `EB_ErrorNone` with a valid packet
- Returns `EB_NoErrorEmptyQueue` when no packet is available (non-blocking mode)
- Returns `EB_ErrorMax` on encoding error

The returned `EbBufferHeaderType` contains:
- `p_buffer`: Encoded bitstream data (OBU format)
- `n_filled_len`: Size of encoded data in bytes
- `pts`: Presentation timestamp of the encoded frame
- `dts`: Decode timestamp
- `flags`: Combination of `EB_BUFFERFLAG_EOS`, `EB_BUFFERFLAG_SHOW_EXT`, `EB_BUFFERFLAG_HAS_TD`, `EB_BUFFERFLAG_IS_ALT_REF`
- `pic_type`: EbAv1PictureType indicating frame type
- `temporal_layer_index`: Temporal layer of the frame
- `qp`: Quantization parameter used
- `luma_sse` / `cb_sse` / `cr_sse`: Per-plane SSE (if stat_report enabled)
- `luma_ssim` / `cb_ssim` / `cr_ssim`: Per-plane SSIM (if stat_report enabled)

### svt_av1_enc_release_out_buffer

```c
EB_API void svt_av1_enc_release_out_buffer(EbBufferHeaderType** p_buffer);
```

Releases an output packet back to the encoder's internal buffer pool. Must be called for every packet received from `svt_av1_enc_get_packet`. Sets `*p_buffer` to NULL.

### svt_av1_get_recon

```c
EB_API EbErrorType svt_av1_get_recon(
    EbComponentType* svt_enc_component,
    EbBufferHeaderType* p_buffer);
```

Optional. Retrieves the reconstructed (decoded) picture. Only available when `recon_enabled` is set to true in the configuration. The caller must provide an allocated buffer in `p_buffer->p_buffer`.

### svt_av1_enc_get_stream_info

```c
EB_API EbErrorType svt_av1_enc_get_stream_info(
    EbComponentType* svt_enc_component,
    uint32_t stream_info_id,
    void* info);
```

Retrieves stream-level information. Currently supported IDs:
- `SVT_AV1_STREAM_INFO_FIRST_PASS_STATS_OUT` (1): Returns first-pass statistics for two-pass encoding

### svt_av1_enc_deinit

```c
EB_API EbErrorType svt_av1_enc_deinit(EbComponentType* svt_enc_component);
```

Deinitializes the encoder. Signals all pipeline threads to shut down, waits for completion, and releases internal resources. Must be called after all encoding is complete (after receiving the EOS output packet).

### svt_av1_enc_deinit_handle

```c
EB_API EbErrorType svt_av1_enc_deinit_handle(EbComponentType* svt_enc_component);
```

Destroys the encoder handle and frees all remaining resources. Must be the last API call for an encoder instance.

### svt_av1_get_version

```c
EB_API const char* svt_av1_get_version(void);
```

Returns a version string in the format `"v$tag-$commit_count-g$hash${dirty:+-dirty}"`.

### svt_av1_print_version

```c
EB_API void svt_av1_print_version(void);
```

Prints the version header and build information to the SVT log output (stderr or file specified by SVT_LOG_FILE environment variable).

### svt_av1_set_log_callback

```c
EB_API void svt_av1_set_log_callback(SvtAv1LogCallback callback, void* context);
```

Registers a callback function for intercepting log messages. When registered, all log output is dispatched to the callback instead of the default stderr/file output. This is a global setting affecting all encoder instances. Should be called before `svt_av1_enc_init_handle`.

### Metadata Functions

```c
EB_API SvtMetadataT* svt_metadata_alloc(const uint32_t type, const uint8_t* data, const size_t sz);
EB_API void svt_metadata_free(void* ptr);
EB_API SvtMetadataArrayT* svt_metadata_array_alloc(const size_t sz);
EB_API void svt_metadata_array_free(void* arr);
EB_API int svt_add_metadata(struct EbBufferHeaderType* buffer, const uint32_t type,
                            const uint8_t* data, const size_t sz);
EB_API size_t svt_metadata_size(SvtMetadataArrayT* metadata, const EbAv1MetadataType type);
EB_API int svt_aom_parse_mastering_display(struct EbSvtAv1MasteringDisplayInfo* mdi, const char* md_str);
EB_API int svt_aom_parse_content_light_level(struct EbContentLightLevel* cll, const char* cll_str);
```

Metadata can be attached to input frames via `svt_add_metadata()` and will be carried through to the output packets. Supported metadata types are defined in `EbAv1MetadataType`.

## Data Structures

### EbComponentType

```c
typedef struct EbComponentType {
    uint32_t size;                // Struct size
    void*    p_component_private; // Internal encoder handle (EbEncHandle*)
    void*    p_application_private; // Unused in encoder API
} EbComponentType;
```

Opaque handle to the encoder instance. Created by `svt_av1_enc_init_handle`, destroyed by `svt_av1_enc_deinit_handle`.

### EbBufferHeaderType

```c
typedef struct EbBufferHeaderType {
    uint32_t size;              // Struct size
    uint8_t* p_buffer;          // Picture/packet data buffer
    uint32_t n_filled_len;      // Bytes of valid data in p_buffer
    uint32_t n_alloc_len;       // Allocated size of p_buffer
    void*    p_app_private;     // Per-picture private data (EbPrivDataNode* linked list)
    void*    wrapper_ptr;       // Internal use (EbObjectWrapper*)
    uint32_t n_tick_count;      // Tick count
    int64_t  dts;               // Decode timestamp
    int64_t  pts;               // Presentation timestamp
    // Output-only fields:
    uint8_t          temporal_layer_index;
    uint32_t         qp;        // QP used for this frame
    uint32_t         avg_qp;    // Average QP
    EbAv1PictureType pic_type;  // Picture type
    uint64_t         luma_sse;  // Luma SSE (if stat_report)
    uint64_t         cr_sse;    // Cr SSE (if stat_report)
    uint64_t         cb_sse;    // Cb SSE (if stat_report)
    uint32_t         flags;     // EB_BUFFERFLAG_* flags
    double           luma_ssim; // Luma SSIM (if stat_report)
    double           cr_ssim;   // Cr SSIM (if stat_report)
    double           cb_ssim;   // Cb SSIM (if stat_report)
    struct SvtMetadataArray* metadata; // Attached metadata
} EbBufferHeaderType;
```

Used for both input pictures and output packets. For input, `p_buffer` points to an `EbSvtIOFormat`. For output, `p_buffer` points to encoded OBU data.

### EbSvtIOFormat

```c
typedef struct EbSvtIOFormat {
    uint8_t* luma;      // Luma plane pointer
    uint8_t* cb;        // Cb (U) plane pointer
    uint8_t* cr;        // Cr (V) plane pointer
    uint32_t y_stride;  // Luma stride in samples
    uint32_t cr_stride; // Cr stride in samples
    uint32_t cb_stride; // Cb stride in samples
} EbSvtIOFormat;
```

Input picture format. For 8-bit content, each sample is one byte. For 10-bit packed content, each sample is two bytes (little-endian 16-bit with upper 6 bits unused).

### EbSvtAv1EncConfiguration

The main configuration struct. All fields documented with defaults and valid ranges:

#### Preset and Quality

| Field | Type | Default | Range | Description |
|-------|------|---------|-------|-------------|
| `enc_mode` | int8_t | 12 | -2..13 | Encoder preset. -1=research, 0=highest quality, 13=fastest |
| `tune` | uint8_t | 1 | 0..4 | Tuning target: 0=VQ, 1=PSNR, 2=SSIM, 3=IQ (Image Quality), 4=MS-SSIM |

#### GOP Structure

| Field | Type | Default | Range | Description |
|-------|------|---------|-------|-------------|
| `intra_period_length` | int32_t | -2 | -2..2^31 | Intra period in frames. -1=no intra refresh, -2=auto |
| `intra_refresh_type` | SvtAv1IntraRefreshType | 1 | 1..2 | 1=CRA (open GOP), 2=IDR (closed GOP) |
| `hierarchical_levels` | uint32_t | auto (~0) | 0..5 | Number of hierarchical levels. MiniGOP size = 2^levels |
| `pred_structure` | uint8_t | 2 | 0..2 | 0=LOW_DELAY_P, 1=LOW_DELAY_B, 2=RANDOM_ACCESS |
| `enable_dg` | bool | true | 0..1 | Enable dynamic GOP |
| `startup_mg_size` | uint8_t | 0 | 0,2,3,4 | Startup mini-GOP size (0=off, 2-4=hierarchical levels) |

#### Input Format

| Field | Type | Default | Range | Description |
|-------|------|---------|-------|-------------|
| `source_width` | uint32_t | 0 | 64..16384 | Frame width in pixels |
| `source_height` | uint32_t | 0 | 64..8704 | Frame height in pixels |
| `frame_rate_numerator` | uint32_t | 0 | - | FPS numerator (0 uses legacy fps) |
| `frame_rate_denominator` | uint32_t | 0 | - | FPS denominator |
| `encoder_bit_depth` | uint32_t | 8 | 8, 10 | Input/encoding bit depth |
| `encoder_color_format` | EbColorFormat | EB_YUV420 | 0..3 | Color format (only YUV420 currently supported) |
| `forced_max_frame_width` | uint32_t | 0 | - | Max frame width for sequence header (0=auto) |
| `forced_max_frame_height` | uint32_t | 0 | - | Max frame height for sequence header (0=auto) |

#### Bitstream

| Field | Type | Default | Range | Description |
|-------|------|---------|-------|-------------|
| `profile` | EbAv1SeqProfile | MAIN_PROFILE | 0..2 | AV1 profile: 0=main, 1=high, 2=professional |
| `tier` | uint32_t | 0 | 0..1 | 0=Main, 1=High |
| `level` | uint32_t | 0 | 0, 20..73 | AV1 level (0=auto, 20=2.0, 63=6.3) |

#### Color

| Field | Type | Default | Range | Description |
|-------|------|---------|-------|-------------|
| `color_primaries` | EbColorPrimaries | EB_CICP_CP_UNSPECIFIED | 0..26 | CICP color primaries |
| `transfer_characteristics` | EbTransferCharacteristics | EB_CICP_TC_UNSPECIFIED | 0..23 | CICP transfer function |
| `matrix_coefficients` | EbMatrixCoefficients | EB_CICP_MC_UNSPECIFIED | 0..18 | CICP matrix coefficients |
| `color_range` | EbColorRange | EB_CR_STUDIO_RANGE | 0..1 | 0=studio range, 1=full range |
| `chroma_sample_position` | EbChromaSamplePosition | EB_CSP_UNKNOWN | 0..2 | Chroma sample position |
| `mastering_display` | EbSvtAv1MasteringDisplayInfo | zeros | - | HDR mastering display metadata |
| `content_light_level` | EbContentLightLevel | zeros | - | HDR content light level |

#### Rate Control

| Field | Type | Default | Range | Description |
|-------|------|---------|-------|-------------|
| `rate_control_mode` | uint8_t | 0 | 0..2 | 0=CQP/CRF, 1=VBR, 2=CBR |
| `qp` | uint32_t | 50 | 1..63 | Base QP for CQP/CRF modes |
| `aq_mode` | uint8_t | 0 | 0..2 | 0=CQP, 1=variance-based segmentation, 2=CRF (with TPL delta-QP) |
| `target_bit_rate` | uint32_t | 2000513 | - | Target bitrate in bits/second (VBR/CBR) |
| `max_bit_rate` | uint32_t | 0 | - | Maximum bitrate for capped CRF |
| `max_qp_allowed` | uint32_t | 63 | 1..63 | Maximum QP |
| `min_qp_allowed` | uint32_t | auto | 1..63 | Minimum QP |
| `vbr_min_section_pct` | uint32_t | 0 | 0..100 | VBR minimum section percentage |
| `vbr_max_section_pct` | uint32_t | 2000 | 0..10000 | VBR maximum section percentage |
| `under_shoot_pct` | uint32_t | 25/50 | 0..100 | Undershoot tolerance (25 for CBR, 50 for VBR) |
| `over_shoot_pct` | uint32_t | 25 | 0..100 | Overshoot tolerance |
| `mbr_over_shoot_pct` | uint32_t | 50 | 0..100 | Max bitrate overshoot tolerance (capped CRF) |
| `starting_buffer_level_ms` | int64_t | 600 | 20..10000 | CBR starting buffer level (ms) |
| `optimal_buffer_level_ms` | int64_t | 600 | 20..10000 | CBR optimal buffer level (ms) |
| `maximum_buffer_size_ms` | int64_t | 1000 | 20..10000 | CBR maximum buffer size (ms) |
| `recode_loop` | uint32_t | 4 | 0..4 | Recode loop aggressiveness |
| `gop_constraint_rc` | bool | false | - | Match target rate per GOP |
| `use_qp_file` | bool | false | - | Use external QP file |
| `use_fixed_qindex_offsets` | uint8_t | 0 | 0..2 | Fixed QP offsets per temporal layer |
| `qindex_offsets[6]` | int32_t | 0 | - | Per-temporal-layer QIndex offsets |
| `key_frame_qindex_offset` | int32_t | 0 | - | Key frame QIndex offset |
| `key_frame_chroma_qindex_offset` | int32_t | 0 | - | Key frame chroma QIndex offset |
| `chroma_qindex_offsets[6]` | int32_t | 0 | - | Per-temporal-layer chroma QIndex offsets |
| `luma_y_dc_qindex_offset` | int32_t | 0 | - | Luma Y DC QIndex offset |
| `chroma_u_dc_qindex_offset` | int32_t | 0 | - | Chroma U DC QIndex offset |
| `chroma_u_ac_qindex_offset` | int32_t | 0 | - | Chroma U AC QIndex offset |
| `chroma_v_dc_qindex_offset` | int32_t | 0 | - | Chroma V DC QIndex offset |
| `chroma_v_ac_qindex_offset` | int32_t | 0 | - | Chroma V AC QIndex offset |
| `lambda_scale_factors[7]` | int32_t | 0 | - | Per-frame-update-type lambda scale factors (factor >> 7) |

#### Multi-Pass

| Field | Type | Default | Range | Description |
|-------|------|---------|-------|-------------|
| `rc_stats_buffer` | SvtAv1FixedBuf | empty | - | Buffer for pass-1 statistics (input for pass-2, output for pass-1) |
| `pass` | int | 0 | 0..2 | 0=single pass, 1=first pass, 2=second pass |

#### Coding Tools

| Field | Type | Default | Range | Description |
|-------|------|---------|-------|-------------|
| `enable_dlf_flag` | uint8_t | 1 | 0..2 | Deblocking filter: 0=off, 1=on, 2=more accurate |
| `cdef_level` | int | -1 | -1..5 | CDEF: -1=auto |
| `enable_restoration_filtering` | int | -1 | -1..1 | Restoration filter: -1=auto, 0=off, 1=on |
| `enable_mfmv` | int | -1 | -1..1 | Motion field motion vector: -1=auto |
| `enable_tf` | uint8_t | 1 | 0..2 | Temporal filtering: 0=off, 1=on, 2=adaptive |
| `enable_overlays` | bool | false | - | Enable overlay frames for temporal filtering |
| `screen_content_mode` | uint32_t | 0 | 0..2 | Screen content: 0=off, 1=on, 2=auto |
| `enable_qm` | bool | false | - | Enable quantization matrices |
| `min_qm_level` | uint8_t | 8 | 0..15 | Min QM flatness |
| `max_qm_level` | uint8_t | 15 | 0..15 | Max QM flatness |
| `min_chroma_qm_level` | uint8_t | 8 | 0..15 | Min chroma QM flatness |
| `max_chroma_qm_level` | uint8_t | 15 | 0..15 | Max chroma QM flatness |
| `fast_decode` | uint8_t | 0 | 0..2 | Decoder-speed-targeted optimizations |
| `lossless` | bool | false | - | Lossless coding mode |
| `max_tx_size` | uint8_t | 64 | 32, 64 | Maximum transform size |

#### Film Grain

| Field | Type | Default | Range | Description |
|-------|------|---------|-------|-------------|
| `film_grain_denoise_strength` | uint32_t | 0 | 0..50 | Film grain denoising strength |
| `film_grain_denoise_apply` | uint8_t | 0 | 0..1 | Denoising application level |
| `adaptive_film_grain` | bool | true | - | Adaptive film grain block size |
| `fgs_table` | AomFilmGrain* | NULL | - | Optional external film grain synthesis table |

#### Tiling

| Field | Type | Default | Range | Description |
|-------|------|---------|-------|-------------|
| `tile_columns` | int32_t | 0 | 0..6 | Log2 number of tile columns |
| `tile_rows` | int32_t | 0 | 0..6 | Log2 number of tile rows |

#### Super Resolution / Reference Scaling

| Field | Type | Default | Range | Description |
|-------|------|---------|-------|-------------|
| `superres_mode` | uint8_t | SUPERRES_NONE | 0..4 | Super-resolution mode |
| `superres_denom` | uint8_t | 8 | 8..16 | Super-resolution denominator |
| `superres_kf_denom` | uint8_t | 8 | 8..16 | Key frame super-resolution denominator |
| `superres_qthres` | uint8_t | 0 | 0..63 | Q-threshold for superres |
| `superres_kf_qthres` | uint8_t | 0 | 0..63 | Key frame Q-threshold for superres |
| `superres_auto_search_type` | uint8_t | 0 | 0..2 | Auto superres search type |
| `resize_mode` | uint8_t | RESIZE_NONE | 0..4 | Reference scaling mode |
| `resize_denom` | uint8_t | 8 | 8..16 | Resize denominator |
| `resize_kf_denom` | uint8_t | 8 | 8..16 | Key frame resize denominator |
| `frame_scale_evts` | SvtAv1FrameScaleEvts | empty | - | Dynamic resize events |

#### S-Frames (Switch Frames)

| Field | Type | Default | Range | Description |
|-------|------|---------|-------|-------------|
| `sframe_dist` | int32_t | 0 | 0..2^31 | S-Frame interval (0=off) |
| `sframe_mode` | EbSFrameMode | 1 | 1..4 | S-Frame insertion mode |
| `sframe_posi` | SvtAv1SFramePositions | empty | - | Explicit S-Frame positions |
| `sframe_qp` | uint8_t | 0 | 0..63 | S-Frame QP |
| `sframe_qp_offset` | int8_t | 0 | - | S-Frame QP offset |

#### Visual Quality Tuning

| Field | Type | Default | Range | Description |
|-------|------|---------|-------|-------------|
| `enable_variance_boost` | bool | false | - | Boost low-variance regions |
| `variance_boost_strength` | uint8_t | 2 | 1..4 | Variance boost curve strength |
| `variance_octile` | uint8_t | 5 | 1..8 | Variance octile for boost detection |
| `variance_boost_curve` | uint8_t | 0 | 0..2 | Boost curve type |
| `sharpness` | int8_t | 0 | -7..7 | Sharpness bias for deblocking and RD |
| `luminance_qp_bias` | uint8_t | 0 | 0..100 | Frame-level luminance QP bias |
| `tf_strength` | uint8_t | varies | - | Temporal filtering strength |
| `qp_scale_compress_strength` | uint8_t | 1 | 0..3 | QP hierarchical layer compression |
| `ac_bias` | double | 0.0 | 0.0..1.0 | AC bias strength for texture preservation |

#### Threading

| Field | Type | Default | Range | Description |
|-------|------|---------|-------|-------------|
| `level_of_parallelism` | uint32_t | 0 | 0..N | Parallelism level (0=auto from core count, 1-6=explicit) |
| `use_cpu_flags` | EbCpuFlags | EB_CPU_FLAGS_ALL | - | CPU instruction set mask |

#### Diagnostics / Output

| Field | Type | Default | Range | Description |
|-------|------|---------|-------|-------------|
| `stat_report` | uint32_t | 0 | 0..1 | Enable PSNR/SSIM computation per frame |
| `recon_enabled` | bool | false | - | Enable reconstructed picture output |
| `scene_change_detection` | uint32_t | 1 | 0..1 | Enable scene change detection |
| `look_ahead_distance` | uint32_t | auto | 0..120 | Lookahead distance in frames |
| `force_key_frames` | bool | false | - | Enable forced key frame positions |
| `multiply_keyint` | bool | false | - | Treat intra_period_length as seconds |
| `avif` | bool | false | - | Still-picture (AVIF) coding mode |
| `rtc` | bool | false | - | Real-time coding mode |
| `enable_roi_map` | bool | false | - | Enable ROI map |
| `extended_crf_qindex_offset` | uint8_t | 0 | 0..28 | Extended CRF fractional QIndex offset |

## Enums

### EbErrorType

| Value | Name | Meaning |
|-------|------|---------|
| 0x00000000 | EB_ErrorNone | Success |
| 0x40001000 | EB_DecUnsupportedBitstream | Unsupported bitstream (decoder) |
| 0x40001004 | EB_DecNoOutputPicture | No output picture (decoder) |
| 0x40001008 | EB_DecDecodingError | Decoding error |
| 0x4000100C | EB_Corrupt_Frame | Corrupt frame |
| 0x80001000 | EB_ErrorInsufficientResources | Memory allocation failure |
| 0x80001001 | EB_ErrorUndefined | Undefined error |
| 0x80001004 | EB_ErrorInvalidComponent | Invalid component handle |
| 0x80001005 | EB_ErrorBadParameter | Invalid parameter |
| 0x80002012 | EB_ErrorDestroyThreadFailed | Thread destruction failed |
| 0x80002021 | EB_ErrorSemaphoreUnresponsive | Semaphore unresponsive |
| 0x80002022 | EB_ErrorDestroySemaphoreFailed | Semaphore destruction failed |
| 0x80002030 | EB_ErrorCreateMutexFailed | Mutex creation failed |
| 0x80002031 | EB_ErrorMutexUnresponsive | Mutex unresponsive |
| 0x80002032 | EB_ErrorDestroyMutexFailed | Mutex destruction failed |
| 0x80002033 | EB_NoErrorEmptyQueue | No packet available (not an error) |
| 0x80002034 | EB_NoErrorFifoShutdown | FIFO shutdown signal (not an error) |

### EbAv1PictureType

| Value | Name | Description |
|-------|------|-------------|
| 0 | EB_AV1_INTER_PICTURE | Inter-predicted frame |
| 1 | EB_AV1_ALT_REF_PICTURE | Altref (temporally filtered) frame -- not displayed |
| 2 | EB_AV1_INTRA_ONLY_PICTURE | Intra-only frame |
| 3 | EB_AV1_KEY_PICTURE | Key frame (IDR) |
| 4 | EB_AV1_NON_REF_PICTURE | Non-reference frame |
| 5 | EB_AV1_FW_KEY_PICTURE | Forward key frame |
| 6 | EB_AV1_SHOW_EXISTING_PICTURE | Show existing frame |
| 7 | EB_AV1_SWITCH_PICTURE | Switch frame (S-Frame) |
| 0xFF | EB_AV1_INVALID_PICTURE | Invalid |

### EncMode (Preset)

| Value | Name | Description |
|-------|------|-------------|
| -1 | ENC_MR | Research mode (higher quality than M0) |
| 0 | ENC_M0 | Highest quality |
| 1-12 | ENC_M1..ENC_M12 | Intermediate presets |
| 13 | ENC_M13 / MAX_ENC_PRESET | Fastest preset |

### EbColorFormat

| Value | Name | Description |
|-------|------|-------------|
| 0 | EB_YUV400 | Monochrome |
| 1 | EB_YUV420 | 4:2:0 subsampling (primary supported format) |
| 2 | EB_YUV422 | 4:2:2 subsampling |
| 3 | EB_YUV444 | 4:4:4 no subsampling |

### EbBitDepth

| Value | Name |
|-------|------|
| 8 | EB_EIGHT_BIT |
| 10 | EB_TEN_BIT |
| 12 | EB_TWELVE_BIT (unsupported) |

### EbAv1SeqProfile

| Value | Name | Description |
|-------|------|-------------|
| 0 | MAIN_PROFILE | 8/10-bit 4:2:0 |
| 1 | HIGH_PROFILE | 8/10-bit 4:4:4 |
| 2 | PROFESSIONAL_PROFILE | 12-bit, any subsampling |

### SvtAv1RcMode

| Value | Name | Description |
|-------|------|-------------|
| 0 | SVT_AV1_RC_MODE_CQP_OR_CRF | Constant QP or Constant Rate Factor |
| 1 | SVT_AV1_RC_MODE_VBR | Variable Bit Rate |
| 2 | SVT_AV1_RC_MODE_CBR | Constant Bit Rate |

### SvtAv1IntraRefreshType

| Value | Name | Description |
|-------|------|-------------|
| 1 | SVT_AV1_FWDKF_REFRESH | CRA (Clean Random Access) - open GOP |
| 2 | SVT_AV1_KF_REFRESH | IDR (Instantaneous Decoder Refresh) - closed GOP |

### PredStructure

| Value | Name | Description |
|-------|------|-------------|
| 0 | LOW_DELAY_P | Low-delay P-only prediction |
| 1 | LOW_DELAY_B | Low-delay bidirectional prediction |
| 2 | RANDOM_ACCESS | Random access (standard hierarchical B) |

### EbSFrameMode

| Value | Name | Description |
|-------|------|-------------|
| 1 | SFRAME_STRICT_BASE | S-Frame only if base layer inter frame |
| 2 | SFRAME_NEAREST_BASE | Next base layer inter frame becomes S-Frame |
| 3 | SFRAME_FLEXIBLE_BASE | Modify miniGOP to promote frame to altref, then S-Frame |
| 4 | SFRAME_DEC_POSI_BASE | Modify miniGOP in decode order for S-Frame insertion |

### SvtAv1LogLevel

| Value | Name | Description |
|-------|------|-------------|
| -1 | SVT_AV1_LOG_ALL | Log all messages |
| 0 | SVT_AV1_LOG_FATAL | Fatal errors only |
| 1 | SVT_AV1_LOG_ERROR | Errors |
| 2 | SVT_AV1_LOG_WARN | Warnings |
| 3 | SVT_AV1_LOG_INFO | Informational |
| 4 | SVT_AV1_LOG_DEBUG | Debug messages |

### EbAv1MetadataType

| Value | Name | Description |
|-------|------|-------------|
| 0 | EB_AV1_METADATA_TYPE_AOM_RESERVED_0 | Reserved |
| 1 | EB_AV1_METADATA_TYPE_HDR_CLL | Content light level |
| 2 | EB_AV1_METADATA_TYPE_HDR_MDCV | Mastering display color volume |
| 3 | EB_AV1_METADATA_TYPE_SCALABILITY | Scalability |
| 4 | EB_AV1_METADATA_TYPE_ITUT_T35 | ITU-T T.35 |
| 5 | EB_AV1_METADATA_TYPE_TIMECODE | Timecode |
| 6 | EB_AV1_METADATA_TYPE_FRAME_SIZE | Frame size |

### PrivDataType (Per-Picture Private Data)

| Value | Name | Description |
|-------|------|-------------|
| 0 | PRIVATE_DATA | Pass-through data written to bitstream |
| 1 | REF_FRAME_SCALING_EVENT | Per-picture reference frame scaling |
| 2 | ROI_MAP_EVENT | Per-picture ROI map |
| 3 | RES_CHANGE_EVENT | Resolution change (key frames only) |
| 4 | RATE_CHANGE_EVENT | Bitrate change |
| 5 | FRAME_RATE_CHANGE_EVENT | Frame rate change |
| 6 | COMPUTE_QUALITY_EVENT | Per-frame quality computation request |

### Buffer Flags

| Macro | Value | Description |
|-------|-------|-------------|
| EB_BUFFERFLAG_EOS | 0x00000001 | End of stream |
| EB_BUFFERFLAG_SHOW_EXT | 0x00000002 | Packet contains show-existing-frame |
| EB_BUFFERFLAG_HAS_TD | 0x00000004 | Packet contains temporal delimiter |
| EB_BUFFERFLAG_IS_ALT_REF | 0x00000008 | Packet is an alt-ref frame |
| EB_BUFFERFLAG_ERROR_MASK | 0xFFFFFFF0 | Error flag mask |

### CPU Flags (x86_64)

| Macro | Bit | Instruction Set |
|-------|-----|-----------------|
| EB_CPU_FLAGS_MMX | 0 | MMX |
| EB_CPU_FLAGS_SSE | 1 | SSE |
| EB_CPU_FLAGS_SSE2 | 2 | SSE2 |
| EB_CPU_FLAGS_SSE3 | 3 | SSE3 |
| EB_CPU_FLAGS_SSSE3 | 4 | SSSE3 |
| EB_CPU_FLAGS_SSE4_1 | 5 | SSE4.1 |
| EB_CPU_FLAGS_SSE4_2 | 6 | SSE4.2 |
| EB_CPU_FLAGS_AVX | 7 | AVX |
| EB_CPU_FLAGS_AVX2 | 8 | AVX2 |
| EB_CPU_FLAGS_AVX512F | 9 | AVX-512 Foundation |
| EB_CPU_FLAGS_AVX512CD | 10 | AVX-512 Conflict Detection |
| EB_CPU_FLAGS_AVX512DQ | 11 | AVX-512 Doubleword/Quadword |
| EB_CPU_FLAGS_AVX512BW | 14 | AVX-512 Byte/Word |
| EB_CPU_FLAGS_AVX512VL | 15 | AVX-512 Vector Length |
| EB_CPU_FLAGS_AVX512ICL | 16 | AVX-512 Icelake extensions |

### CPU Flags (AArch64)

| Macro | Bit | Feature |
|-------|-----|---------|
| EB_CPU_FLAGS_NEON | 0 | Armv8.0-A Neon |
| EB_CPU_FLAGS_ARM_CRC32 | 1 | CRC32 instructions |
| EB_CPU_FLAGS_NEON_DOTPROD | 2 | Neon dot product |
| EB_CPU_FLAGS_NEON_I8MM | 3 | Neon i8mm |
| EB_CPU_FLAGS_SVE | 4 | SVE |
| EB_CPU_FLAGS_SVE2 | 5 | SVE2 |
| EB_CPU_FLAGS_NEOVERSE_V2 | 6 | Neoverse V2 specializations |

### Special Constants

| Macro | Value | Description |
|-------|-------|-------------|
| EB_CPU_FLAGS_ALL | (max >> 1) - 1 | All CPU flags enabled |
| EB_CPU_FLAGS_INVALID | 1 << 63 | Invalid flag (used as sentinel) |
| SVT_AV1_VERSION_MAJOR | 4 | API major version |
| SVT_AV1_VERSION_MINOR | 0 | API minor version |
| SVT_AV1_VERSION_PATCHLEVEL | 1 | API patch level |
| SVT_AV1_ENC_ABI_VERSION | 0 | ABI version (incremented on struct changes) |
| MAX_TEMPORAL_LAYERS | 6 | Maximum temporal layers |
| HIERARCHICAL_LEVELS_AUTO | ~0u | Auto-detect hierarchical levels |
| MAX_HIERARCHICAL_LEVEL | 6 | Maximum hierarchical level value |
| REF_LIST_MAX_DEPTH | 4 | Maximum references per list |
| EB_MAX_NUM_OPERATING_POINTS | 32 | Maximum operating points |

## Dependencies

- Standard C library (stdint.h, stdlib.h, stdio.h, string.h, stdbool.h, stdarg.h)
- Platform threading (pthreads on POSIX, Windows threads on Win32)
- No external library dependencies for the core encoder

## SIMD Functions

The public API itself has no SIMD dependencies. SIMD is used internally by the encoder pipeline. The `use_cpu_flags` configuration parameter allows the application to restrict which SIMD instruction sets the encoder may use, which is useful for testing portable implementations.

For a port, the API layer requires no SIMD reimplementation. All SIMD-dependent code is internal to the encoder pipeline (documented in 00-architecture.md).
