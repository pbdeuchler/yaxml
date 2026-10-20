#!/usr/bin/env -S uv run --script
"""Union per-target coverage exports into one per-file table.

Usage: coverage_merge.py OUT_DIR [SRC_ROOT]  (reads every OUT_DIR/<target>/{lcov.info,export.json})

`llvm-cov report` with several `-object` binaries maps each function from the
first binary that contains it, so a binary that never calls a function hides
the others' counts. Each target is therefore exported against its own binary
and the exports are unioned here: line and branch hits add across lcov
tracefiles (written to OUT_DIR/merged.lcov); a region or function is covered
when any target covers it (regions keyed by file and span, as in llvm-cov's
per-file summary, which takes the best instantiation)."""
import json, sys
from collections import defaultdict
from pathlib import Path

out = Path(sys.argv[1])
targets = sorted(p.parent.name for p in out.glob("*/export.json"))
lines = defaultdict(int)  # (file, line) -> hits
branches = defaultdict(int)  # (file, line, block, branch) -> taken
regions = {}  # (file, span) -> covered
functions = {}  # (file, first-region span) -> covered

for target in targets:
    path = None
    for record in (out / target / "lcov.info").read_text().splitlines():
        tag, _, value = record.partition(":")
        if tag == "SF":
            path = value
        elif tag == "DA":
            line, hits = value.split(",")[:2]
            lines[path, int(line)] += int(hits)
        elif tag == "BRDA":
            line, block, branch, taken = value.split(",")
            branches[path, int(line), block, branch] += 0 if taken == "-" else int(taken)
    data = json.loads((out / target / "export.json").read_text())["data"][0]
    files = {entry["filename"] for entry in data["files"]}
    for function in data["functions"]:
        code = [r for r in function["regions"] if r[7] == 0 and function["filenames"][r[5]] in files]
        for r in code:
            key = (function["filenames"][r[5]], tuple(r[:4]))
            regions[key] = regions.get(key, False) or r[4] > 0
        if code and code[0][5] == 0:
            key = (function["filenames"][0], tuple(code[0][:4]))
            functions[key] = functions.get(key, False) or function["count"] > 0

records = defaultdict(list)
for (file, line, block, branch), taken in sorted(branches.items()):
    records[file].append(f"BRDA:{line},{block},{branch},{taken}")
for (file, line), hits in sorted(lines.items()):
    records[file].append(f"DA:{line},{hits}")
(out / "merged.lcov").write_text("".join(
    f"SF:{file}\n" + "\n".join(entries) + "\nend_of_record\n" for file, entries in sorted(records.items())))

table = defaultdict(lambda: [0] * 8)  # regions, missed, functions, missed, lines, missed, branches, missed
for column, metric in enumerate([regions, functions]):
    for (file, _), covered in metric.items():
        table[file][2 * column] += 1
        table[file][2 * column + 1] += not covered
for (file, _), hits in lines.items():
    table[file][4] += 1
    table[file][5] += hits == 0
for (file, *_), taken in branches.items():
    table[file][6] += 1
    table[file][7] += taken == 0

root = (sys.argv[2] if len(sys.argv) > 2 else str(Path.cwd() / "src")).rstrip("/") + "/"
pct = lambda total, missed: f"{100 * (total - missed) / total:.2f}%" if total else "-"
print(f"{'Filename':<30}{'Regions':>9}{'Missed':>8}{'Cover':>9}{'Functions':>11}{'Missed':>8}{'Cover':>9}"
      f"{'Lines':>8}{'Missed':>8}{'Cover':>9}{'Branches':>10}{'Missed':>8}{'Cover':>9}")
totals = [0] * 8
for file in sorted(table):
    row = table[file]
    totals = [a + b for a, b in zip(totals, row)]
    print(f"{file.removeprefix(root):<30}" + "".join(
        f"{row[i]:>{w}}{row[i + 1]:>8}{pct(row[i], row[i + 1]):>9}" for i, w in zip(range(0, 8, 2), (9, 11, 8, 10))))
print(f"{'TOTAL':<30}" + "".join(
    f"{totals[i]:>{w}}{totals[i + 1]:>8}{pct(totals[i], totals[i + 1]):>9}" for i, w in zip(range(0, 8, 2), (9, 11, 8, 10))))
