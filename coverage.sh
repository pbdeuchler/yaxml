#!/bin/sh
# Usage: ./coverage.sh [TARGET...]
# Replays every target's corpus directory and the seeds through each TARGET
# (default: every target) with branch instrumentation, using fuzz/corpus-full
# when ./setup.sh corpus cloned it, else fuzz/corpus. Reports yaxml library
# (src/) coverage: one TOTAL line per target, then the per-file union of all
# targets exported so far in fuzz/coverage/report.txt and merged.lcov
# (fuzz/coverage_merge.py; a multi-object llvm-cov report undercounts).
# Needs uv.
set -eu
. "$(dirname "$0")/harness/lib.sh"
help_if "$@"
need_fuzz
need uv "install it from https://docs.astral.sh/uv/"
cd "$ROOT/fuzz"
host=$(rustup run "$FUZZ_TOOLCHAIN" rustc -vV | sed -n 's/^host: //p')
llvm=$(rustup run "$FUZZ_TOOLCHAIN" rustc --print sysroot)/lib/rustlib/$host/bin
[ -x "$llvm/llvm-cov" ] || die "llvm-cov missing; run: rustup component add llvm-tools-preview --toolchain $FUZZ_TOOLCHAIN"
build=$FUZZ_TARGET/coverage
out=$ROOT/fuzz/coverage
src=$ROOT/src
targets=${*:-$(cargo +"$FUZZ_TOOLCHAIN" fuzz list)}
for target in $targets; do
	mkdir -p "$CORPUS/$target" "$out/$target"
	RUSTFLAGS="${RUSTFLAGS:-} -Zcoverage-options=branch" \
		cargo +"$FUZZ_TOOLCHAIN" fuzz coverage --target-dir "$build" \
		"$target" "$CORPUS"/*/ "$ROOT/tests/fixtures/fuzz-seeds" >"$out/$target/log" 2>&1 ||
		{ cat "$out/$target/log" >&2; exit 1; }
	binary=$build/$host/release/$target
	profile=$out/$target/coverage.profdata
	printf '%-15s' "$target"
	"$llvm/llvm-cov" report "$binary" -instr-profile="$profile" "$src" | tail -1
	# Export against the target's own binary: a multi-object report maps each
	# function from the first binary containing it, hiding the others' counts.
	"$llvm/llvm-cov" export -format=lcov "$binary" -instr-profile="$profile" "$src" >"$out/$target/lcov.info"
	"$llvm/llvm-cov" export -format=text -skip-expansions "$binary" -instr-profile="$profile" "$src" >"$out/$target/export.json"
done
uv run --script coverage_merge.py "$out" "$src" | tee "$out/report.txt"
