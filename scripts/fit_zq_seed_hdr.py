#!/usr/bin/env python3
"""S1/S2 zensim-target seed anchors for the svt HDR loop (registration:
benchmarks/zensim_hdr_target_wave_2026-08-27.md SEED ARM section; rule
frozen pre-fit). Era-B zensim ONLY; census scenes excluded; qp via the
fleet's svt_q_to_qp. Emits seed_s1.tsv / seed_s2.tsv (t, tier, qp0)."""
import re, sys, collections, statistics
import pyarrow.parquet as pq

ERA = "/mnt/v/output/hdrgrid-2026-08-06/zensim_scores_by_judge_era.parquet"
HARVEST = "/mnt/v/output/hdrgrid-2026-08-06/harvest-2026-08-26/scores.parquet"
CENSUS_TSV = "/home/lilith/work/zen/zensim/benchmarks/hdr_instrument_refs_2026-08-27.tsv"
OUT = "/home/lilith/work/zen/zenav1-svt/benchmarks"
TARGETS = [70.0, 80.0, 88.0]

def q_to_qp(q):  # fleet svt_q_to_qp verbatim
    q = max(0.0, min(100.0, float(q)))
    return max(1, min(63, round(63.0 - q * 63.0 / 100.0)))

census_scenes = set()
for line in open(CENSUS_TSV):
    line = line.strip()
    if not line or line.startswith("#") or line.startswith("scene\t"):
        continue
    census_scenes.add(line.split("\t")[0])

era = pq.read_table(ERA, columns=["encode_sha", "rendition", "codec", "zensim_score", "judge_era"])
zmap = {}
for sha, rend, codec, z, e in zip(*[era[c].to_pylist() for c in era.column_names]):
    if codec == "zenav1-svt" and e.startswith("B-"):
        zmap[sha] = (rend, z)
print(f"era-B svt cells: {len(zmap)}")

hv = pq.read_table(HARVEST, columns=["image_path", "codec", "q", "encode_sha"])
cells = collections.defaultdict(list)  # rendition -> [(qp, zensim)]
matched = 0
for ip, codec, q, sha in zip(*[hv[c].to_pylist() for c in hv.column_names]):
    if codec != "zenav1-svt" or sha not in zmap:
        continue
    rend = ip.rsplit("/", 1)[-1]
    scene = rend.split(".scale")[0]
    if scene in census_scenes:
        continue
    cells[rend].append((q_to_qp(q), float(zmap[sha][1])))
    matched += 1
print(f"joined cells (non-census): {matched} over {len(cells)} renditions")

def pixels(rend):
    m = re.search(r"\.scale(\d+)x(\d+)\.", rend)
    return int(m.group(1)) * int(m.group(2)) if m else 0

pix_sorted = sorted(pixels(r) for r in cells)
t1 = pix_sorted[len(pix_sorted) // 3]
t2 = pix_sorted[2 * len(pix_sorted) // 3]
def tier_of(rend):
    p = pixels(rend)
    return "small" if p <= t1 else ("mid" if p <= t2 else "large")

oracle = collections.defaultdict(list)  # (t, tier) -> [qp*]
for rend, pts in cells.items():
    tier = tier_of(rend)
    for t in TARGETS:
        best = min(pts, key=lambda pz: abs(pz[1] - t))
        qps = best[0] if any(z >= t for _, z in pts) else 1
        oracle[(t, tier)].append(qps)
        oracle[(t, "*")].append(qps)

with open(f"{OUT}/zq_seed_s1_2026-08-27.tsv", "w") as f:
    f.write("t\ttier\tqp0\n")
    for t in TARGETS:
        v = oracle[(t, "*")]
        f.write(f"{t:.0f}\t*\t{round(statistics.median(v))}\n")
        print(f"S1 t{t:.0f}: qp0={round(statistics.median(v))} (n={len(v)}, IQR {sorted(v)[len(v)//4]}..{sorted(v)[3*len(v)//4]})")
with open(f"{OUT}/zq_seed_s2_2026-08-27.tsv", "w") as f:
    f.write("t\ttier\tqp0\n")
    for t in TARGETS:
        for tier in ("small", "mid", "large"):
            v = oracle[(t, tier)]
            f.write(f"{t:.0f}\t{tier}\t{round(statistics.median(v))}\n")
            print(f"S2 t{t:.0f} {tier}: qp0={round(statistics.median(v))} (n={len(v)})")
print("terciles: <=", t1, "<=", t2)
