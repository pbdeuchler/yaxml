#!/bin/sh
# Replay each target's corpus with branch instrumentation and report library
# (src/) coverage: one TOTAL line per target, then the per-file union of all
# targets (fuzz/coverage/report.txt, merged.lcov) via coverage_merge.py.
# Usage: fuzz/coverage.sh [target...]
set -eu
cd "$(dirname "$0")/.."
toolchain=${TOOLCHAIN:-nightly-2026-10-18}
host=$(rustup run "$toolchain" rustc -vV | sed -n 's/^host: //p')
llvm=$(rustup run "$toolchain" rustc --print sysroot)/lib/rustlib/$host/bin
build=${CARGO_TARGET_DIR:-fuzz/target}/coverage
out=fuzz/coverage
targets=${*:-$(cargo +"$toolchain" fuzz list)}
for target in $targets; do
	mkdir -p "fuzz/corpus/$target" "$out/$target"
	RUSTFLAGS="${RUSTFLAGS:-} -Zcoverage-options=branch" \
		cargo +"$toolchain" fuzz coverage --target-dir "$build" \
		"$target" "fuzz/corpus/$target" fuzz/corpus >"$out/$target/log" 2>&1 ||
		{ cat "$out/$target/log" >&2; exit 1; }
	binary=$build/$host/release/$target
	profile=$out/$target/coverage.profdata
	printf '%-13s' "$target"
	"$llvm/llvm-cov" report "$binary" -instr-profile="$profile" src | tail -1
	# Export against the target's own binary: a multi-object report maps each
	# function from the first binary containing it, hiding the others' counts.
	"$llvm/llvm-cov" export -format=lcov "$binary" -instr-profile="$profile" src >"$out/$target/lcov.info"
	"$llvm/llvm-cov" export -format=text -skip-expansions "$binary" -instr-profile="$profile" src >"$out/$target/export.json"
done
# Union every target exported so far, including earlier runs of other targets.
uv run --script fuzz/coverage_merge.py "$out" | tee "$out/report.txt"
