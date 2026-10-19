"""Scan MIR dumps for rustc's SimplifyComparisonIntegral miscompile.

rust-lang/rust#158206 (fixed by #158214; affects stable 1.95/1.96 and
nightlies before the fix): the pass folds `x = Eq(discr, k); switchInt(move x)`
into a switch on `discr` and deletes the assignment of `x`, even when `x` is
also read in the same block (for example copied into a struct field before a
short-circuit `x && y` branch on it). Release builds then read an unassigned
local. yaxml hit it in `ParserState::rereading` written with struct-update
syntax (`Self { rereads, repairs_characters: rereads && characters, ..self }`).

This reports every local that loses a comparison assignment in the pass while
the optimized body still reads it. Usage, with the gate's toolchain:

    RUSTC_BOOTSTRAP=1 CARGO_TARGET_DIR=/private/tmp/scan cargo rustc --release --lib -- \\
        -Zdump-mir=SimplifyComparisonIntegral -Zdump-mir-dir=/private/tmp/scan-mir
    uv run --no-project python -I tooling/simplify_comparison_scan.py /private/tmp/scan-mir

Exit status 1 when a candidate is found.
"""
import glob
import os
import re
import sys

hits = 0
for before in sorted(glob.glob(os.path.join(sys.argv[1], "*SimplifyComparisonIntegral.before.mir"))):
    b = open(before).read()
    a = open(before.replace(".before.", ".after.")).read()
    for local in sorted(set(re.findall(r"^\s+(_\d+) = (?:Eq|Ne|Lt|Le|Gt|Ge)\(", b, re.M))):
        count = lambda text: len(re.findall(rf"^\s+{local} = ", text, re.M))
        reads = re.findall(rf"(?:copy|move) {local}\b|&(?:mut )?{local}\b", a)
        if count(a) < count(b) and reads:
            hits += 1
            name = os.path.basename(before).split(".3-2")[0]
            print(f"{name}: {local} loses a comparison; still read {len(reads)}x")
print(f"{hits} suspicious locals")
sys.exit(1 if hits else 0)
