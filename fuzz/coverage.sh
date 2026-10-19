#!/bin/sh
# Replay each target's corpus with branch instrumentation and report library
# (src/) coverage: one TOTAL line per target, then the merged per-file table.
# Usage: fuzz/coverage.sh [target...]
set -eu
cd "$(dirname "$0")/.."
toolchain=${TOOLCHAIN:-nightly-2026-06-06}
host=$(rustup run "$toolchain" rustc -vV | sed -n 's/^host: //p')
llvm=$(rustup run "$toolchain" rustc --print sysroot)/lib/rustlib/$host/bin
build=${CARGO_TARGET_DIR:-fuzz/target}/coverage
out=fuzz/coverage
objects=
profiles=
for target in ${*:-$(cargo +"$toolchain" fuzz list)}; do
	mkdir -p "fuzz/corpus/$target" "$out/$target"
	RUSTFLAGS="${RUSTFLAGS:-} -Zcoverage-options=branch" \
		cargo +"$toolchain" fuzz coverage --target-dir "$build" \
		"$target" "fuzz/corpus/$target" fuzz/corpus >"$out/$target/log" 2>&1 ||
		{ cat "$out/$target/log" >&2; exit 1; }
	binary=$build/$host/release/$target
	printf '%-13s' "$target"
	"$llvm/llvm-cov" report "$binary" -instr-profile="$out/$target/coverage.profdata" src | tail -1
	objects="${objects:+$objects -object }$binary"
	profiles="$profiles $out/$target/coverage.profdata"
done
# shellcheck disable=SC2086 # word-split argument lists
"$llvm/llvm-profdata" merge -sparse $profiles -o "$out/merged.profdata"
# shellcheck disable=SC2086 # word-split argument lists
"$llvm/llvm-cov" report $objects -instr-profile="$out/merged.profdata" src | tee "$out/report.txt"
