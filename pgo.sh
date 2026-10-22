#!/bin/sh
# Usage: ./pgo.sh
# Profile-guided build of yaxmllint: builds an instrumented binary, trains it
# on the parse_benchmark fixtures under every option set below (DOM, --stream
# TextReader, NOENT/DTDATTR/NOBLANKS/RECOVER, validation, HTML, C14N, XPath,
# save), merges the profiles and rebuilds with them into
# harness/target/pgo/use/release/yaxmllint. On macOS it then prints each
# training run's retired instructions against the plain release build. PGO
# only helps an artifact trained on its own workloads: untrained paths get
# slower (docs/benchmarks.md). Needs llvm-profdata matching rustc's LLVM
# (rustup component add llvm-tools, or Xcode's via xcrun). Settings: REPEAT
# (documents per run, default 20).
set -eu
. "$(dirname "$0")/harness/lib.sh"
help_if "$@"
profdata=$(ls "$(rustc --print sysroot)"/lib/rustlib/*/bin/llvm-profdata 2>/dev/null | head -n 1) || :
[ -n "$profdata" ] || profdata=$(xcrun -f llvm-profdata 2>/dev/null) || profdata=$(command -v llvm-profdata) ||
	die "llvm-profdata not found; run: rustup component add llvm-tools"
pgo=$TARGET/pgo
fixtures=$pgo/fixtures
plain=$TARGET/release/yaxmllint
tuned=$pgo/use/release/yaxmllint
rm -rf "$pgo/raw"
cd "$HARNESS"
cargo build --release -q --example write_fixtures --bin yaxmllint
"$TARGET/release/examples/write_fixtures" "$fixtures"
CARGO_TARGET_DIR=$pgo/gen RUSTFLAGS="-Cprofile-generate=$pgo/raw" cargo build --release -q --bin yaxmllint

# runs: one "options|fixture" line per training run.
runs() {
	for set in "" "--stream" "--noent" "--noent --stream" "--dtdattr" "--loaddtd --valid" "--noblanks" \
		"--recover" "--html" "--c14n" "--xpath count(//*)" "--format"; do
		for file in "$fixtures"/*.xml; do echo "$set|$file"; done
	done
}
# run OPTIONS FIXTURE COMMAND...: COMMAND OPTIONS with REPEAT copies of
# FIXTURE, stdout discarded. Sets that reject a fixture (--valid without a DTD)
# still train.
run() {
	opts=$1 files=$(i=0; while [ $i -lt "${REPEAT:-20}" ]; do echo "$2"; i=$((i + 1)); done)
	case $opts in --c14n | --xpath* | --format) ;; *) opts="$opts --noout" ;; esac
	shift 2
	# shellcheck disable=SC2086 # options and files split on purpose
	"$@" $opts $files >/dev/null || :
}
runs | while IFS='|' read -r set file; do run "$set" "$file" "$pgo/gen/release/yaxmllint" 2>/dev/null; done
"$profdata" merge -o "$pgo/merged.profdata" "$pgo/raw"
CARGO_TARGET_DIR=$pgo/use RUSTFLAGS="-Cprofile-use=$pgo/merged.profdata" cargo build --release -q --bin yaxmllint
echo "built $tuned"

/usr/bin/time -l true 2>&1 | grep -q 'instructions retired' || exit 0
printf '| options | fixture | instructions |\n| --- | --- | ---: |\n'
runs | while IFS='|' read -r set file; do
	count() { run "$set" "$file" /usr/bin/time -l "$1" 2>&1 | awk '/instructions retired/ { print $1 }'; }
	awk -v s="${set:-default}" -v f="$(basename "$file" .xml)" -v p="$(count "$plain")" -v t="$(count "$tuned")" \
		'BEGIN { printf "| %s | %s | %+.2f%% |\n", s, f, 100 * (t / p - 1) }'
done
