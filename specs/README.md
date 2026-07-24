# SVT-AV1 Algorithm Specifications

Comprehensive specifications of the SVT-AV1 encoder, derived from source code and test analysis. Sufficient for an algorithm-for-algorithm port to another language.

**Source:** [gitlab.com/AOMediaCodec/SVT-AV1](https://gitlab.com/AOMediaCodec/SVT-AV1) at commit `003643d4`
**Total:** 19 spec files, 12,403 lines

## Spec Index

### Architecture & API
| Spec | Lines | Description |
|------|------:|-------------|
| [00-architecture](00-architecture.md) | 522 | 15-stage encoding pipeline, threading model, GOP structure, data flow |
| [01-api](01-api.md) | 792 | Public API (80+ config params), encoding lifecycle, all structs/enums |

### Core Encoding Algorithms
| Spec | Lines | Description |
|------|------:|-------------|
| [02-motion-estimation](02-motion-estimation.md) | 1074 | 10-stage ME pipeline, hierarchical search, sub-pel refinement, global motion, RANSAC, hash motion, warped motion |
| [03-mode-decision](03-mode-decision.md) | 681 | 4-stage MD pipeline (MDS0-MDS3), candidate generation, RDO, speed presets, NIC pruning |
| [04-transforms](04-transforms.md) | 795 | 16 transform types, 19 sizes, quantization, RDOQ, scan orders, lambda tables |
| [05-intra-prediction](05-intra-prediction.md) | 443 | All intra modes (DC, directional, smooth, paeth, CfL, palette, filter-intra) |
| [06-inter-prediction](06-inter-prediction.md) | 529 | Inter modes, sub-pel interpolation, compound prediction, OBMC, warped motion |
| [07-entropy-coding](07-entropy-coding.md) | 833 | Arithmetic coder, CDF-based coding, context derivation, all syntax elements |
| [10-encoding-loop](10-encoding-loop.md) | 630 | Main RDO loop, partition search, encode-decode cycle, early termination |

### Post-Processing
| Spec | Lines | Description |
|------|------:|-------------|
| [08-loop-filters](08-loop-filters.md) | 619 | Deblocking (4/6/8/14-tap), CDEF, Wiener/sgrproj restoration, super-resolution |

### Rate Control
| Spec | Lines | Description |
|------|------:|-------------|
| [09-rate-control](09-rate-control.md) | 709 | CQP, CRF, VBR, CBR modes, adaptive QP, multi-pass, lambda calculation |

### Pipeline & Data Management
| Spec | Lines | Description |
|------|------:|-------------|
| [11-picture-management](11-picture-management.md) | 488 | PCS lifecycle, buffer management, picture analysis, reference frames, packetization |
| [17-temporal-filtering](17-temporal-filtering.md) | 569 | Alt-ref frame generation, temporal denoising, motion-compensated averaging |

### Advanced Features
| Spec | Lines | Description |
|------|------:|-------------|
| [12-film-grain](12-film-grain.md) | 472 | Noise model estimation, AR model fitting, grain synthesis |
| [13-segmentation](13-segmentation.md) | 220 | Variance-based segmentation, adaptive QP mapping |

### Infrastructure
| Spec | Lines | Description |
|------|------:|-------------|
| [14-utilities](14-utilities.md) | 724 | Memory, threading, logging, PSNR/SSIM, resize, math, hash, k-means |
| [15-rtcd](15-rtcd.md) | 466 | Runtime CPU dispatch, ~1100 function pointers, feature detection |
| [16-data-structures](16-data-structures.md) | 1043 | Every major struct/enum field-by-field (BlockSize, PCS, BlkStruct, SeqHeader, etc.) |

### Testing
| Spec | Lines | Description |
|------|------:|-------------|
| [18-testing](18-testing.md) | 794 | Complete test inventory, SIMD testing pattern, E2E framework, coverage gaps, porting recommendations |

## Coverage

- **Source/Lib/Codec/**: 111/111 .c files referenced (100% after gap fixes)
- **Source/API/**: 7/7 headers (100%)
- **Source/Lib/C_DEFAULT/**: 11/11 files (100%)
- **Source/Lib/Globals/**: 3/3 files (100%)
- **Test files**: 78/78 catalogued in spec 18
- **SIMD variants**: ~1100 dispatched functions inventoried in spec 15

## Porting Notes

Each spec includes:
- **Source Files** table linking to every implementation file
- **Test Coverage** table linking to every relevant test
- **Data Structures** with field-by-field descriptions
- **Algorithms** with step-by-step descriptions
- **SIMD Functions** listing all functions needing portable reimplementation
- **Dependencies** showing inter-module relationships

Start with [18-testing](18-testing.md) to establish the test infrastructure, then port modules bottom-up following the dependency graph in [00-architecture](00-architecture.md).
