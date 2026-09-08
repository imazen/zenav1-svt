# Historical GitHub issue #3: Consolidate svtav1-rs: 8 crates → 4 (delete dead disjoint-mut + cuda stubs)

Snapshot before the 2026-09-08 handoff cleanup. State at capture: **CLOSED**.
Original: https://github.com/imazen/zenav1-svt/issues/3. Current disposition: [issue audit](../../../OPEN-ISSUES-AUDIT-2026-09-08.md).

## Original body

## Problem

`svtav1-rs/` is **8 crates**. Target: **4 or fewer publishable crates.** Two of
the eight are dead stubs that should just be deleted.

## Current inventory

| crate | LOC | role |
|---|---:|---|
| svtav1-types | 2016 | shared types |
| svtav1-tables | 350 | const tables |
| svtav1-dsp | 10700 | pixel kernels (already consolidated) |
| svtav1-entropy | 5697 | range coder |
| svtav1-encoder | 6093 | encoder |
| svtav1-disjoint-mut | 226 | **dead stub — delete** |
| svtav1-cuda | 4 | **unused stub — delete or `publish=false`** |
| svtav1 | 1045 | facade |

## Target: 4 published crates

1. **`svtav1-types`** — absorb `svtav1-tables` (tables are just data; 350 LOC).
2. **`svtav1-dsp`** — pixel kernels, unchanged.
3. **`svtav1-encoder`** — absorb `svtav1-entropy`. This is an encoder-only port,
   so the range coder has exactly one consumer; folding it in keeps `svtav1-dsp`
   purely pixel-DSP.
4. **`svtav1`** — existing facade.

## Delete

- **`svtav1-disjoint-mut`** (226 LOC): dead *and* misleading. It's declared as a
  dependency of `svtav1-encoder` but referenced nowhere in the workspace. Its
  `DisjointMut` is a `&mut self` `Vec<T>` wrapper — **not** the rav1d-style
  `&self` / `UnsafeCell` disjoint-aliasing type its doc comment claims ("multiple
  threads write to non-overlapping regions of a shared buffer simultaneously"),
  and the `BorrowTracker` it also claims to use is wired into nothing. The
  encoder's tile parallelism uses `std::thread::scope` with per-tile owned
  buffers merged after `join()` (partition-own-merge), so the type isn't needed.
- **`svtav1-cuda`** (4 LOC): unused NVDEC/NVENC FFI intent-stub, `#![allow(unsafe_code)]`,
  depends only on `svtav1-types`, referenced nowhere. Delete (recoverable from
  git) — or keep it `publish = false` and outside the count if the GPU bridge is
  a real roadmap item.

## Future-work note (not this issue)

If SVT-AV1-style **segment (wavefront) parallelism** is ever added to cut
per-image latency (the C encoder saturates cores via `EncDecSegments` +
pipeline stages, all bit-exact — today our encoder is single-threaded, see
`pipeline.rs:224` `tile_rows = 1`), it *will* need a **real** DisjointMut
(rav1d-style, `UnsafeCell`-backed, `&self` mutation) for the shared
reconstruction buffer. That's the legitimate version of what this stub was
gesturing at — track it separately if/when threading work starts.

## Timing

Release-prep, low urgency. `svtav1-dsp` is already the hard part and it's done.

