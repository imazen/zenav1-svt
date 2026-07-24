# 14. Utilities

This chapter documents the utility subsystems in SVT-AV1: memory management, threading, logging, PSNR calculation, frame resizing, hashing, generic containers, math helpers, random number generation, interpolation filters, and k-means clustering. Each is an infrastructure component that a port must reimplement or map to equivalent platform facilities.

---

## 14.1 Memory Management (`svt_malloc.h`, `svt_malloc.c`)

### 14.1.1 Purpose

Provides all heap allocation for the encoder with three capabilities:

1. **Aligned allocation** -- Every allocation is aligned to `DEFAULT_ALIGNMENT` (2 * sizeof(void*), typically 16 bytes). This is critical because SIMD instructions require aligned memory.
2. **Overflow protection** -- Allocations are capped at `AOM_MAX_ALLOCABLE_MEMORY` (8 GB). Overflow of `nmemb * size` is detected before calling `malloc`.
3. **Debug memory tracking** -- When `DEBUG_MEMORY_USAGE` is enabled (non-NDEBUG builds), every allocation and free is tracked in a global hash table of 4M+1 entries. Leak detection runs when the last encoder component is destroyed.

### 14.1.2 Aligned Allocation Algorithm

```
svt_aom_memalign(align, size):
    aligned_size = size + align - 1 + ADDRESS_STORAGE_SIZE
    addr = malloc(aligned_size)
    x = align_addr(addr + ADDRESS_STORAGE_SIZE, align)
    store addr at (x - sizeof(size_t))    // save real malloc address
    return x

svt_aom_free(memblk):
    addr = load from (memblk - sizeof(size_t))
    free(addr)
```

The actual `malloc` address is stored immediately before the aligned pointer. `svt_aom_malloc(size)` calls `svt_aom_memalign(DEFAULT_ALIGNMENT, size)`.

### 14.1.3 Allocation Macros

All allocation in the encoder goes through macros, never raw `malloc`/`calloc`. Key macros:

| Macro | Behavior |
|-------|----------|
| `EB_MALLOC(ptr, size)` | `malloc` + track + check |
| `EB_CALLOC(ptr, count, size)` | `calloc` + track + check |
| `EB_FREE(ptr)` | untrack + `free` + set NULL |
| `EB_MALLOC_ARRAY(pa, count)` | `EB_MALLOC(pa, sizeof(*pa) * count)` |
| `EB_CALLOC_ARRAY(pa, count)` | `EB_CALLOC(pa, count, sizeof(*pa))` |
| `EB_REALLOC_ARRAY(pa, count)` | `realloc` + update tracking |
| `EB_MALLOC_2D(p2d, w, h)` | Allocate row-pointer array + contiguous data block |
| `EB_FREE_2D(p2d)` | Free data block then row-pointer array |
| `EB_MALLOC_ALIGNED(ptr, size)` | Platform-specific aligned alloc (ALVALUE=64 bytes) |
| `EB_FREE_ALIGNED(ptr)` | Platform-specific aligned free |
| `EB_ALLOC_PTR_ARRAY(pa, count)` | `EB_CALLOC` for array of pointers |
| `EB_FREE_PTR_ARRAY(pa, count)` | Free each element then free array |

The `_NO_CHECK` variants skip the NULL-return-error-code path (they just log the failure).

### 14.1.4 Debug Memory Tracking

When `DEBUG_MEMORY_USAGE` is defined:

- **Hash table**: A static array of 4,194,305 `MemoryEntry` slots, indexed by a hash of the pointer value. Collisions are resolved by linear probing (open addressing).
- **Entry structure**: `{ ptr, count (bytes), file, line, type }` where type is one of: `EB_N_PTR` (malloc), `EB_C_PTR` (calloc), `EB_A_PTR` (aligned), `EB_MUTEX`, `EB_SEMAPHORE`, `EB_THREAD`.
- **Thread safety**: All access is protected by a lazily-initialized global mutex (`g_malloc_mutex`), created via `svt_run_once`.
- **Component counting**: `svt_increase_component_count()` / `svt_decrease_component_count()` track encoder instances. When the count reaches zero, all remaining entries are reported as leaks.
- **Memory profiling**: When `PROFILE_MEMORY_USAGE` is defined, the top 10 allocation sites by total bytes are printed.

### 14.1.5 Porting Notes

A port needs:
- Aligned allocation (16-byte minimum, 64-byte for SIMD scratch buffers)
- Overflow-safe size computation
- Optional leak detection (debug mode only)
- All macros can be replaced with language-native equivalents (e.g., Rust's `Vec`, `Box`, aligned allocators)

---

## 14.2 Threading Primitives (`svt_threads.h`, `svt_threads.c`)

### 14.2.1 Purpose

Platform-independent wrappers for threads, semaphores, mutexes, condition variables, atomic operations, and one-time initialization. Hides Windows vs. POSIX differences.

### 14.2.2 Threads

```
svt_create_thread(thread_function, thread_context) -> EbHandle
svt_destroy_thread(thread_handle) -> EbErrorType
```

- **Linux/macOS**: Creates `pthread_t` with 1 MiB minimum stack. If running as root (and not under thread sanitizer), sets realtime priority 99.
- **Windows**: `CreateThread` with default stack and attributes.
- `svt_destroy_thread` joins the thread (`pthread_join` / `WaitForSingleObject`).

Macros `EB_CREATE_THREAD`, `EB_DESTROY_THREAD`, `EB_CREATE_THREAD_ARRAY`, `EB_DESTROY_THREAD_ARRAY` add memory tracking.

### 14.2.3 Semaphores

```
svt_create_semaphore(initial_count, max_count) -> EbHandle
svt_post_semaphore(handle) -> EbErrorType
svt_block_on_semaphore(handle) -> EbErrorType
svt_destroy_semaphore(handle) -> EbErrorType
```

- **Linux**: POSIX `sem_t` (unnamed semaphore). `max_count` is ignored.
- **macOS**: `dispatch_semaphore_t`. `max_count` is ignored.
- **Windows**: Win32 `CreateSemaphore` with both `initial_count` and `max_count`.
- `block_on_semaphore` on Linux retries on `EINTR`.

### 14.2.4 Mutexes

```
svt_create_mutex() -> EbHandle
svt_block_on_mutex(handle) -> EbErrorType
svt_release_mutex(handle) -> EbErrorType
svt_destroy_mutex(handle) -> EbErrorType
```

- **Linux/macOS**: `pthread_mutex_t` with default attributes.
- **Windows**: Win32 `CreateMutex`.

### 14.2.5 Condition Variables

```c
typedef struct CondVar {
    int32_t val;
    // platform-specific mutex + condvar
} CondVar;

svt_create_cond_var(cond_var) -> EbErrorType
svt_set_cond_var(cond_var, newval) -> EbErrorType
svt_wait_cond_var(cond_var, input) -> EbErrorType
```

The condition variable wraps a mutex-protected integer. `svt_wait_cond_var` blocks until `val != input`. `svt_set_cond_var` sets `val` and broadcasts to all waiters.

### 14.2.6 Atomic Operations

```
svt_aom_atomic_set_u32(AtomicVarU32* var, uint32_t in)
```

Implemented by locking `var->mutex`, setting `var->obj = in`, and unlocking. This is a simple mutex-guarded atomic, not a hardware atomic.

### 14.2.7 One-Time Initialization

```
svt_run_once(OnceType* once_control, OnceFn init_routine)
```

- **Linux/macOS**: `pthread_once`
- **Windows**: `InitOnceExecuteOnce`

The `DEFINE_ONCE_MUTEX(name)` macro creates a lazily-initialized mutex with automatic cleanup via `atexit`.

### 14.2.8 Porting Notes

The threading API is intentionally thin. A port maps these to the target language's threading primitives. The key contract: semaphores provide bounded counting synchronization; mutexes are non-recursive; condition variables wrap a simple integer state.

---

## 14.3 System Resource Manager (`sys_resource_manager.h`, `sys_resource_manager.c`)

### 14.3.1 Purpose

Implements the pipeline buffer management system. Every inter-process buffer pool in the encoder (input queue, reference frames, reconstruction buffers, results buffers) is an `EbSystemResource`. This is the core flow-control mechanism.

### 14.3.2 Architecture

```
EbSystemResource
  ├── wrapper_ptr_pool[N]     -- Array of EbObjectWrapper (one per pooled object)
  ├── empty_queue (EbMuxingQueue)  -- Pool of available (empty) objects
  └── full_queue  (EbMuxingQueue)  -- Queue of completed (full) objects

EbMuxingQueue
  ├── lockout_mutex
  ├── object_queue   (EbCircularBuffer)  -- Pending objects
  ├── process_queue  (EbCircularBuffer)  -- Pending consumer FIFOs
  └── process_fifo_ptr_array[M]          -- One FIFO per producer/consumer
```

### 14.3.3 EbObjectWrapper

Each pooled object is wrapped in an `EbObjectWrapper`:

| Field | Description |
|-------|-------------|
| `object_ptr` | Pointer to the actual data object |
| `live_count` | Number of active references in the pipeline. When 0 and `release_enable` is true, the wrapper returns to the empty pool. Value `~0u` means "released". |
| `release_enable` | Controls whether release is permitted. Used by long-lived objects like SequenceControlSet. |
| `system_resource_ptr` | Back-pointer to the owning SystemResource |
| `next_ptr` | Linked-list pointer for FIFO membership |
| `object_destroyer` | Custom destructor, or NULL to use the default `dctor`-based destruction |

### 14.3.4 Key Operations

**Getting an empty object** (`svt_get_empty_object`):
1. Register the calling process FIFO with the empty queue
2. Block on the FIFO's counting semaphore (waits until an object is available)
3. Lock the FIFO's mutex, pop the front wrapper
4. Reset `live_count` to 0, enable release
5. Unlock and return the wrapper

**Posting a full object** (`svt_post_full_object`):
1. Lock the full queue's mutex
2. Push the wrapper onto the full queue's object circular buffer
3. Run assignation (match objects to waiting consumer FIFOs)
4. Unlock

**Getting a full object** (`svt_get_full_object`):
1. Register the calling process FIFO with the full queue
2. Block on the FIFO's semaphore
3. Lock, pop front, unlock
4. Returns `EB_NoErrorFifoShutdown` if `quit_signal` is set

**Releasing an object** (`svt_release_object`):
1. Lock the empty queue's mutex
2. Decrement `live_count` (unless already 0)
3. If `release_enable && live_count == 0`: mark as released (`~0u`), push to front of empty queue
4. Unlock

### 14.3.5 Muxing Queue Assignation

The muxing queue is the core multiplexer. It holds two circular buffers: one for pending objects and one for pending process FIFOs. `svt_muxing_queue_assignation` loops: while both queues are non-empty, pop a process FIFO and an object, push the object onto that FIFO, and post its semaphore. This distributes work round-robin among consumers.

### 14.3.6 Shutdown

`svt_shutdown_process` sets `quit_signal = true` on each consumer FIFO and posts their semaphores. This causes blocked `svt_get_full_object` calls to return `EB_NoErrorFifoShutdown`.

### 14.3.7 Porting Notes

This is the backbone of the pipeline. A port needs equivalent producer-consumer queues with semaphore-based blocking and reference counting. The `live_count` / `release_enable` mechanism allows objects to be shared across multiple pipeline stages without premature recycling.

---

## 14.4 Logging (`svt_log.h`, `svt_log.c`)

### 14.4.1 Purpose

Leveled logging with support for custom callbacks.

### 14.4.2 Log Levels

`SVT_AV1_LOG_ALL` < `SVT_AV1_LOG_DEBUG` < `SVT_AV1_LOG_INFO` < `SVT_AV1_LOG_WARN` < `SVT_AV1_LOG_ERROR` < `SVT_AV1_LOG_FATAL`

### 14.4.3 Macros

| Macro | Level | Prefix |
|-------|-------|--------|
| `SVT_LOG(fmt, ...)` | ALL | none |
| `SVT_DEBUG(fmt, ...)` | DEBUG | `LOG_TAG[debug]: ` |
| `SVT_INFO(fmt, ...)` | INFO | `LOG_TAG[info]: ` |
| `SVT_WARN(fmt, ...)` | WARN | `LOG_TAG[warn]: ` |
| `SVT_ERROR(fmt, ...)` | ERROR | `LOG_TAG[error]: ` |
| `SVT_FATAL(fmt, ...)` | FATAL | `LOG_TAG[fatal]: ` |

Each source file defines `LOG_TAG` before including `svt_log.h` (defaults to `"Svt"`).

### 14.4.4 Implementation

- The logger is lazily initialized via `svt_run_once`.
- Default logger writes to `stderr` (or to a file specified by `SVT_LOG_FILE` env var).
- Log level is controlled by `SVT_LOG` env var (integer, default `SVT_AV1_LOG_INFO`).
- A custom callback can be set via `svt_aom_log_set_callback` before any logging occurs.
- When `CONFIG_LOG_QUIET` is defined, all macros expand to no-ops that suppress unused-variable warnings.
- Cleanup is registered via `atexit`.

---

## 14.5 PSNR / SSE Calculation (`svt_psnr.h`, `svt_psnr.c`)

### 14.5.1 Purpose

Computes Sum of Squared Errors (SSE) between original and reconstructed frames for PSNR metric reporting.

### 14.5.2 Key Functions

```
svt_aom_get_sse(a, a_stride, b, b_stride, width, height) -> int64_t
```

Algorithm:
1. Handle right-edge remainder (`width % 16`) pixels with scalar variance loop
2. Handle bottom-edge remainder (`height % 16`) pixels with scalar variance loop
3. Process aligned 16x16 blocks using `svt_aom_mse16x16` (RTCD-dispatched, may use SIMD)
4. Sum all partial SSE values

### 14.5.3 High Bit Depth

`svt_aom_highbd_get_sse` operates on `uint16_t` samples (cast from `uint8_t*` via `CONVERT_TO_SHORTPTR`). Same tiling strategy but uses `svt_aom_highbd_mse16x16`.

### 14.5.4 Per-Plane Helpers

- `svt_aom_get_y_sse_part` / `svt_aom_get_u_sse_part` / `svt_aom_get_v_sse_part` -- SSE for a rectangular region of Y/U/V planes from `Yv12BufferConfig` structures.

### 14.5.5 Variance Functions

The `HIGHBD_VAR` macro generates `svt_aom_highbd_10_varianceWxH_c` functions for all standard block sizes. These compute 10-bit variance using the formula: `var = sse - (sum*sum)/(W*H)`, where SSE and sum are scaled down by powers of 2 for 10-bit precision.

---

## 14.6 Frame Resize (`resize.h`, `resize.c`)

### 14.6.1 Purpose

Implements frame scaling for super-resolution and reference frame resizing. This is one of the more complex utility modules.

### 14.6.2 Downsampling Filters

Two half-band FIR filters for factor-of-2 downsampling:
- **Symmetric even** (`svt_aom_av1_down2_symeven_half_filter`): `{56, 12, -3, -1}` -- 8-tap symmetric
- **Symmetric odd** (`av1_down2_symodd_half_filter`): `{64, 35, 0, -3}` -- 7-tap symmetric

### 14.6.3 Interpolation Filters

Four sets of 8-tap polyphase interpolation kernels, each with 64 phases (1 << RS_SUBPEL_BITS = 6, so 64 subpixel positions):

| Filter Set | Bandwidth | Usage |
|-----------|-----------|-------|
| `filteredinterp_filters500` | 0.5 | Scale ratio <= 0.5 |
| `filteredinterp_filters625` | 0.625 | Scale ratio ~0.625 |
| `filteredinterp_filters750` | 0.75 | Scale ratio ~0.75 |
| `filteredinterp_filters875` | 0.875 | Scale ratio ~0.875 |
| `filteredinterp_filters1000` | 1.0 (normative) | No scaling / super-res |

Filter selection is based on the scaling ratio. Each kernel sums to 128 (7-bit precision, `FILTER_BITS = 7`).

### 14.6.4 Frame Resize API

```
svt_aom_resize_frame(src, dst, bd, num_planes, ss_x, ss_y, is_packed, buffer_enable_mask, is_2bcompress)
```

Resizes an entire frame (all enabled planes) from source to destination dimensions. Handles both 8-bit and high-bit-depth, packed and planar formats.

### 14.6.5 Super-Resolution

Super-resolution scales the encoding width down by a denominator (8..16, where 8 = no scaling and 16 = half width). Key functions:

- `scale_pcs_params` -- Updates picture parameters for the scaled dimensions
- `svt_aom_init_resize_picture` -- Initializes resize for both super-res and reference scaling
- `svt_aom_reset_resized_picture` -- Resets after resize processing
- `coded_to_superres_mi` -- Converts MI column index from coded to super-res coordinates

### 14.6.6 Reference Scaling

- `scale_source_references` -- Scales source-domain references
- `svt_aom_scale_rec_references` -- Scales reconstructed-domain references
- `svt_aom_use_scaled_rec_refs_if_needed` -- Selects appropriate scaled reference
- `svt_aom_use_scaled_source_refs_if_needed` -- Selects scaled source reference at full, quarter, and sixteenth resolution

### 14.6.7 Porting Notes

The interpolation filter tables are normative -- they must be reproduced exactly. The resize algorithm is separable (horizontal then vertical) with proper rounding. High bit depth requires 32-bit intermediate precision.

---

## 14.7 Common Utilities (`common_utils.h`, `common_utils.c`)

### 14.7.1 Purpose

Provides AV1 specification lookup tables and inline utility functions used throughout the encoder. These are normative data from the AV1 spec.

### 14.7.2 Key Lookup Tables

| Table | Indexed by | Returns |
|-------|-----------|---------|
| `block_size_wide[BLOCK_SIZES_ALL]` | BlockSize | Width in pixels |
| `block_size_high[BLOCK_SIZES_ALL]` | BlockSize | Height in pixels |
| `mi_size_wide[BLOCK_SIZES_ALL]` | BlockSize | Width in 4x4 MI units |
| `mi_size_high[BLOCK_SIZES_ALL]` | BlockSize | Height in 4x4 MI units |
| `tx_size_wide[TX_SIZES_ALL]` | TxSize | Width in pixels |
| `tx_size_high[TX_SIZES_ALL]` | TxSize | Height in pixels |
| `tx_size_wide_log2[TX_SIZES_ALL]` | TxSize | log2(width) |
| `tx_size_high_log2[TX_SIZES_ALL]` | TxSize | log2(height) |
| `tx_depth_to_tx_size[3][BLOCK_SIZES_ALL]` | (depth, bsize) | TxSize for luma |
| `txsize_sqr_map[TX_SIZES_ALL]` | TxSize | Nearest square TxSize (round down) |
| `txsize_sqr_up_map[TX_SIZES_ALL]` | TxSize | Nearest square TxSize (round up) |
| `svt_aom_ss_size_lookup[BLOCK_SIZES_ALL][2][2]` | (bsize, ss_x, ss_y) | Chroma plane BlockSize |
| `svt_aom_subsize_lookup[EXT_PARTITION_TYPES][6]` | (partition, sqr_bsize_idx) | Sub-partition BlockSize |
| `av1_num_ext_tx_set[EXT_TX_SET_TYPES]` | TxSetType | Number of transforms in set |
| `av1_ext_tx_used[EXT_TX_SET_TYPES][TX_TYPES]` | (set, type) | 1 if type is in set |

### 14.7.3 Key Inline Functions

- `get_ext_tx_set_type(tx_size, is_inter, use_reduced_set)` -- Determines which transform set to use based on size and inter/intra
- `get_plane_block_size(bsize, ss_x, ss_y)` -- Chroma block size for given subsampling
- `av1_get_adjusted_tx_size(tx_size)` -- Clamps TX sizes > 32 (64x64->32x32, etc.)
- `av1_get_max_uv_txsize(bsize, ss_x, ss_y)` -- Maximum UV transform size
- `partition_mi_offset(bsize, shape, nsi, &mi_row, &mi_col)` -- Computes MI origin of sub-partition
- `is_chroma_reference(mi_row, mi_col, bsize, ss_x, ss_y)` -- Whether this block has chroma info

### 14.7.4 Partition-to-Shape Mapping

Two direction tables convert between the AV1 spec's `PartitionType` and the encoder's internal `Part` (shape) enum:
- `from_shape_to_part[EXT_PARTITION_TYPES]` -- Part -> PartitionType
- `from_part_to_shape[PART_S + 1]` -- PartitionType -> Part

### 14.7.5 UV Mode Mapping

`g_uv2y[16]` maps UV prediction modes to their luma equivalents. Notably, `UV_CFL_PRED` maps to `DC_PRED`.

---

## 14.8 Block Geometry and Utility Functions (`utility.h`, `utility.c`)

### 14.8.1 Purpose

Defines the block geometry table used by mode decision, coded block statistics for the legacy CU tree, mini-GOP structure tables, and various numeric macros.

### 14.8.2 BlockGeom Structure

```c
typedef struct BlockGeom {
    uint8_t   sq_size;        // Parent square size (4..128)
    uint8_t   bwidth, bheight; // Block dimensions in pixels
    uint8_t   bwidth_uv, bheight_uv; // Chroma dimensions (4:2:0)
    BlockSize bsize;          // AV1 BlockSize enum
    BlockSize bsize_uv;       // Chroma BlockSize
    uint16_t  d1_depth_offset; // Offset to next d1 (same-depth) block
    uint16_t  ns_depth_offset; // Offset to skip all NSQ blocks at this depth
} BlockGeom;
```

### 14.8.3 Geometry Index (GeomIndex)

The encoder supports 11 different block geometry configurations, controlling which partition shapes and minimum block sizes are available:

| GeomIndex | SB Size | Min Block | NSQ Shapes | Total Blocks |
|-----------|---------|-----------|-----------|-------------|
| GEOM_0 | 64 | 16x16 | None | 21 |
| GEOM_1 | 64 | 16x16 | H, V only | 41 |
| GEOM_2 | 64 | 8x8 | None | 85 |
| GEOM_3 | 64 | 8x8 | H, V (not small) | 105 |
| GEOM_4 | 64 | 8x8 | H, V (not smallest) | 169 |
| GEOM_5 | 64 | 4x4 | H, V | 425 |
| GEOM_6 | 64 | 4x4 | H, V | 681 |
| GEOM_7 | 64 | 4x4 | H, V, H4, V4 | 849 |
| GEOM_8 | 64 | 4x4 | All | 1101 |
| GEOM_9 | 128 | 4x4 | All | 4421 |
| GEOM_10 | 128 | 8x8 | H, V, H4, V4 | 2377 |

### 14.8.4 Block Geometry Construction

`svt_aom_build_blk_geom(geom, blk_geom_table)` fills the geometry table recursively:

1. Set `max_sb`, `max_depth`, `max_part` based on `GeomIndex`
2. Count total active blocks to verify
3. Recursively scan all blocks (`md_scan_all_blks`):
   - For each square block at position (x, y) of size `sq_size`:
     - Enumerate partition shapes (0..max_part)
     - For each shape, enumerate sub-blocks (1..4 depending on shape)
     - Record bwidth/bheight using `ns_quarter_size_mult` tables
     - Look up BlockSize from `hvsize_to_bsize[h_idx][v_idx]`
   - Recurse into four quadrant children if `halfsize >= min_size`

### 14.8.5 MiniGop Statistics

```c
typedef struct MiniGopStats {
    uint8_t hierarchical_levels;
    uint8_t start_index, end_index;
    uint8_t length;
} MiniGopStats;
```

31 entries define the hierarchical mini-GOP structure for up to 6 levels. Each entry covers a contiguous range of pictures within the GOP.

### 14.8.6 Numeric Macros

Key macros defined in `utility.h`:

| Macro | Description |
|-------|-------------|
| `DIVIDE_AND_ROUND(x, y)` | `(x + y/2) / y` |
| `CLIP3(min, max, a)` | Clamp value to [min, max] |
| `SQR(x)` | `x * x` |
| `FLOAT2FP(x, bits, type)` | Float to fixed-point |
| `FP2FLOAT(x, bits, fpt, ft)` | Fixed-point to float |
| `CEILING(x, base)` | Round up to multiple of base |
| `FLOOR_POW2(x)` | Round down to power of 2 |
| `CEIL_POW2(x)` | Round up to power of 2 |
| `LOG2F_8(x)` | Fast log2 for values < 128 |
| `MAX_CU_COUNT(d)` | Total partitions: `(4^d - 1) / 3` |

### 14.8.7 Log2

```
svt_aom_log2f_32(x) -> uint32_t
```
Computes floor(log2(x)) for 32-bit integers using a binary search (NLZ algorithm): checks bits 16, 8, 4, 2, 1 in sequence.

---

## 14.9 CRC-32C Hash (`hash.h`, `hash.c`)

### 14.9.1 Purpose

CRC-32C (Castagnoli polynomial 0x82F63B78) used for block hash matching in IntraBC and hash-based motion estimation.

### 14.9.2 Table Construction

`svt_av1_crc32c_calculator_init(CRC32C* p)` builds an 8x256 lookup table for byte-at-a-time (with 8-byte-at-a-time fast path) CRC computation:

1. First table (`table[0]`): Standard bit-by-bit CRC for each byte value 0..255
2. Tables 1..7: Composed from `table[0]` for multi-byte processing

### 14.9.3 CRC Computation

`svt_av1_get_crc32c_value_c(context, buf, len)` (C reference implementation):
1. Process unaligned leading bytes one at a time
2. Process aligned 8-byte chunks using all 8 tables simultaneously (Sarwate's algorithm extended to 8 bytes)
3. Process trailing bytes one at a time
4. XOR with 0xFFFFFFFF at start and end

RTCD-dispatched: hardware CRC32C instructions (SSE4.2) are used when available.

---

## 14.10 Dynamic Array (`vector.h`, `vector.c`)

### 14.10.1 Purpose

A simple generic dynamic array (similar to C++ `std::vector`). Used for palette mode candidate lists and similar variable-length collections.

### 14.10.2 Structure

```c
typedef struct Vector {
    uint32_t size;          // Number of elements
    uint32_t capacity;      // Allocated capacity
    uint32_t element_size;  // Size of each element in bytes
    void*    data;          // Contiguous storage
} Vector;
```

### 14.10.3 Operations

| Function | Description |
|----------|-------------|
| `svt_aom_vector_setup(v, cap, elem_size)` | Allocate with initial capacity (min 2) |
| `svt_aom_vector_destroy(v)` | Free storage |
| `svt_aom_vector_push_back(v, elem)` | Append element; grow by 2x if full |
| `svt_aom_vector_byte_size(v)` | `size * element_size` |
| `svt_aom_vector_begin(v)` | Iterator to first element |
| `svt_aom_vector_iterator(v, index)` | Iterator to element at index |

Growth uses a factor of 2. Reallocation copies via `svt_memcpy` (RTCD-dispatched, may use SIMD).

### 14.10.4 Iterator

```c
typedef struct Iterator {
    void*  pointer;
    size_t element_size;
} Iterator;
```

`svt_aom_iterator_increment` advances by `element_size` bytes. `ITERATOR_GET_AS(type, iter)` dereferences with a cast.

---

## 14.11 Math Utilities (`mathutils.h`)

### 14.11.1 Linear System Solver

`linsolve(n, A, stride, b, x)` solves Ax = b using Gaussian elimination with partial pivoting:
1. Forward elimination with row swapping for numerical stability
2. Backward substitution
3. Returns 0 if a near-zero pivot is encountered (singular matrix)

Threshold for near-zero: `1.0E-16`.

### 14.11.2 Least Squares

Three-step API to avoid materializing the full A matrix:

```
least_squares_init(mat, y, n)         -- Zero accumulator matrices
least_squares_accumulate(mat, y, a, b, n)  -- Add equation: a . x = b
least_squares_solve(mat, y, x, n)     -- Solve A'A x = A'b
```

This computes `mat += a * a'` and `y += a * b` incrementally, then calls `linsolve`.

### 14.11.3 Matrix Multiply

`multiply_mat(m1, m2, res, m1_rows, inner_dim, m2_cols)` -- Standard triple-loop matrix multiply. Used in global motion estimation.

---

## 14.12 Random Number Generation (`random.h`)

### 14.12.1 Algorithm

Linear Congruential Generator (LCG):
```
state = state * 1103515245 + 12345   (mod 2^32)
```

### 14.12.2 Functions

| Function | Range | Notes |
|----------|-------|-------|
| `lcg_next(state)` | Full 32-bit | Raw LCG step |
| `lcg_rand16(state)` | [0, 32768) | `(next / 65536) % 32768` |
| `lcg_randint(state, n)` | [0, n) | `(next * n) >> 32` (uses high bits) |
| `lcg_randrange(state, lo, hi)` | [lo, hi) | `lo + randint(hi - lo)` |
| `lcg_pick(n, k, out, seed)` | k distinct from [0,n) | Rejection sampling |

`lcg_randint` uses multiplication-and-shift rather than modulo to avoid bias and use higher-quality top bits.

---

## 14.13 Interpolation Filter Definitions (`filter.h`)

### 14.13.1 Constants

| Constant | Value | Description |
|----------|-------|-------------|
| `FILTER_BITS` | 7 | Filter coefficient precision |
| `SUBPEL_BITS` | 4 | Subpixel precision bits |
| `SUBPEL_SHIFTS` | 16 | Number of subpel positions |
| `SUBPEL_TAPS` | 8 | Filter tap count |
| `SCALE_SUBPEL_BITS` | 10 | Scaling subpixel precision |

### 14.13.2 Bilinear Filters

8 phases of 2-tap bilinear filters (`bilinear_filters_2t`), coefficients sum to 128.

### 14.13.3 InterpFilters Packing

Two `InterpFilter` values (horizontal and vertical) are packed into a single `uint32_t`:
- Bits 0..15: vertical filter
- Bits 16..31: horizontal filter

Functions: `av1_extract_interp_filter`, `av1_make_interp_filters`, `av1_broadcast_interp_filter`.

### 14.13.4 Subpel Kernel Access

`av1_get_interp_filter_subpel_kernel(filter_params, subpel)` returns a pointer to the 8-tap kernel for a given subpixel position: `filter_ptr + taps * subpel`.

---

## 14.14 K-Means Clustering (`k_means_template.h`)

### 14.14.1 Purpose

Template-based k-means implementation for palette mode. The template parameter `AV1_K_MEANS_DIM` controls dimensionality (1 for luma, 2 for luma+chroma).

### 14.14.2 Algorithm

Standard Lloyd's k-means:

```
svt_av1_k_means(data, centroids, indices, n, k, max_itr):
    calc_indices(data, centroids, indices, n, k)  // Assign each point to nearest centroid
    this_dist = calc_total_dist(...)
    for i in 0..max_itr:
        save previous centroids and indices
        calc_centroids(data, centroids, indices, n, k)  // Update centroids
        calc_indices(data, centroids, indices, n, k)     // Reassign
        this_dist = calc_total_dist(...)
        if this_dist > pre_dist:  // Diverged
            restore previous state
            break
        if centroids unchanged:
            break
```

### 14.14.3 Centroid Calculation

For each cluster, sum all member data points and divide by count. If a cluster becomes empty, assign it a random data point (using `lcg_rand16`).

### 14.14.4 Distance

Squared Euclidean distance in the configured dimensionality: `sum((p1[i] - p2[i])^2)`.

### 14.14.5 Constraints

- `n <= 32768` (max data points, typically `MAX_SB_SQUARE`)
- `k <= PALETTE_MAX_SIZE` (max 8 colors)
- Pre-indices buffer is `MAX_SB_SQUARE` elements

---

## 14.15 Object Lifecycle (`object.h`)

### 14.15.1 Purpose

Defines the constructor/destructor pattern used by all encoder objects.

### 14.15.2 Pattern

Every object that needs cleanup has an `EbDctor dctor` as its first field. The destructor is set during construction.

### 14.15.3 Macros

| Macro | Description |
|-------|-------------|
| `EB_NEW(pobj, ctor, ...)` | `calloc` + call constructor + delete on error |
| `EB_DELETE(pobj)` | Call dctor if non-null, then free |
| `EB_DELETE_PTR_ARRAY(pa, count)` | Delete each element, then free array |

The `EB_NEW` macro handles the common pattern of allocating, constructing, and returning `EB_ErrorInsufficientResources` on failure. Construction errors automatically trigger destruction.

---

## 14.16 Test Coverage

The following test files exercise these utilities:

| Test File | What It Tests |
|-----------|--------------|
| `test/MemTest.cc` | Memory allocation, leak detection, tracking |
| `test/PsnrTest.cc` | SSE/PSNR computation correctness |
| `test/ResizeTest.cc` | Frame resize accuracy |
| `test/VarianceTest.cc` | 8-bit variance functions |
| `test/HbdVarianceTest.cc` | High bit depth variance |
| `test/compute_mean_test.cc` | Mean computation kernels |
| `test/SpatialFullDistortionTest.cc` | Spatial distortion metric |
| `test/PackUnPackTest.cc` | Pixel packing/unpacking (8<->16 bit) |
| `test/BlockErrorTest.cc` | Transform block error metrics |
| `test/frame_error_test.cc` | Frame-level error metrics |
| `test/ssim_test.cc` | SSIM block-level scoring (`svt_ssim_8x8`, `svt_ssim_4x4`, `_hbd` variants): validates SIMD against C reference with random data, zero, and extreme-value inputs for 8-bit and 10-bit |
