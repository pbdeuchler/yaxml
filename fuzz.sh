#!/bin/sh
# Usage: ./fuzz.sh [TARGET [SECONDS [LIBFUZZER_FLAG...]]]
# Without TARGET, lists the targets. Otherwise fuzzes TARGET for SECONDS
# (default 60) with the pinned nightly toolchain. New inputs go to
# fuzz/corpus/TARGET/, crashes to fuzz/artifacts/TARGET/; yaxml's
# tests/fixtures/fuzz-seeds/ are read-only extra seeds.
# Example: ./fuzz.sh dom 600 -fork=4
set -eu
. "$(dirname "$0")/lib.sh"
help_if "$@"
need_yaxml
need_fuzz
cd "$HARNESS/fuzz"
targets=$(cargo +"$FUZZ_TOOLCHAIN" fuzz list)
if [ $# -eq 0 ]; then
	echo "$targets"
	exit 0
fi
target=$1
seconds=${2:-60}
shift $(($# < 2 ? $# : 2))
echo "$targets" | grep -qx "$target" || die "unknown target '$target'; ./fuzz.sh lists them"
case $seconds in '' | *[!0-9]*) die "SECONDS must be a whole number, not '$seconds'" ;; esac
mkdir -p "corpus/$target"
exec cargo +"$FUZZ_TOOLCHAIN" fuzz run -O --target-dir "$FUZZ_TARGET" "$target" \
	"corpus/$target" "$YAXML/tests/fixtures/fuzz-seeds" -- -max_total_time="$seconds" -timeout=2 "$@"
