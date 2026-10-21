#!/usr/bin/env -S uv run --script
"""Usage: fuzz/corpus_cap.py MIN_DIR OUT_DIR BUDGET
Copies each `cargo fuzz cmin` result MIN_DIR/<target>/ to OUT_DIR/<target>/,
capped at BUDGET bytes per target: inputs are ranked by new edge features per
byte, read from the FT lines of MIN_DIR/<target>.ctl (the cmin run's
-merge_control_file; features first seen at that input, smallest inputs
first). Prints one row per target."""
import shutil, sys
from pathlib import Path

src, out, budget = Path(sys.argv[1]), Path(sys.argv[2]), int(sys.argv[3])
total_in = total_out = 0
for ctl in sorted(src.glob("*.ctl")):
    target = ctl.stem
    lines = ctl.read_text().splitlines()
    n = int(lines[0])
    paths = lines[2 : 2 + n]
    ft = {}
    for line in lines[2 + n :]:
        if line.startswith("FT "):
            parts = line.split()
            ft[int(parts[1])] = len(parts) - 2
    kept = {p.name for p in (src / target).iterdir() if p.is_file()}
    rows = []
    for i, path in enumerate(paths):
        name = Path(path).name
        if name in kept:
            size = (src / target / name).stat().st_size
            rows.append((ft.get(i, 0), size, name))
    size_all = sum(r[1] for r in rows)
    feats_all = sum(r[0] for r in rows) or 1
    if size_all > budget:
        rows.sort(key=lambda r: (-r[0] / max(r[1], 1), r[1], r[2]))
        chosen, used = [], 0
        for r in rows:
            if used + r[1] <= budget:
                chosen.append(r)
                used += r[1]
    else:
        chosen = rows
    dest = out / target
    dest.mkdir(parents=True, exist_ok=True)
    for _, _, name in chosen:
        shutil.copyfile(src / target / name, dest / name)
    size_kept = sum(r[1] for r in chosen)
    feats_kept = sum(r[0] for r in chosen)
    total_in += size_all
    total_out += size_kept
    print(f"{target:14} cmin {len(rows):5} files {size_all:9} B -> {len(chosen):5} files {size_kept:7} B, new-edge features kept {feats_kept / feats_all:.2%}")
print(f"total cmin {total_in} B -> {total_out} B")
