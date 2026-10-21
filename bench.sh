#!/bin/sh
# Usage: ./bench.sh [FIXTURE...]
# Matched parse comparison of yaxml against the pinned libxml2 2.15.4 on the
# parse_benchmark fixtures (default: all of tiny flat text attributes
# namespaces normalized unicode deep mixed dtd-decl dtd-refs dtd-nested
# dtd-attrs dtd), in every mode. Each sample pair runs fresh processes in
# alternating order and must observe identical logical values. Prints the
# ratio table, libxml2 median / yaxml median of parse plus destruction time
# per document (above 1 favors yaxml); raw rows and the table go to
# results/bench-<timestamp>.txt. Settings: MODES (default "dom owned reader
# push buffered"), SAMPLES (5), ITERATIONS (documents per sample, 20),
# CHUNK (push/buffered chunk bytes, 8192).
set -eu
. "$(dirname "$0")/lib.sh"
help_if "$@"
need_yaxml
need_oracle
fixtures=${*:-tiny flat text attributes namespaces normalized unicode deep mixed dtd-decl dtd-refs dtd-nested dtd-attrs dtd}
modes=${MODES:-dom owned reader push buffered}
c=$(helper parse_bench)
cd "$HARNESS"
cargo build --release -q --example parse_bench --example parse_benchmark
mkdir -p "$RESULTS"
log=$RESULTS/bench-$(date +%Y%m%d-%H%M%S).txt
{
	printf '| fixture |'
	for mode in $modes; do printf ' %s |' "$mode"; done
	printf '\n| --- |'
	for mode in $modes; do printf ' ---: |'; done
	printf '\n'
} | tee -a "$log"
for fixture in $fixtures; do
	row="| $fixture |"
	for mode in $modes; do
		"$TARGET/release/examples/parse_benchmark" --libxml2-helper "$c" \
			--yaxml-helper "$TARGET/release/examples/parse_bench" --fixture "$fixture" --mode "$mode" \
			--samples "${SAMPLES:-5}" --iterations "${ITERATIONS:-20}" --chunk-size "${CHUNK:-8192}" \
			>"$log.run" || { cat "$log.run" >&2; die "parse_benchmark failed on $fixture $mode"; }
		cat "$log.run" >>"$log"
		ratio=$(awk -F'\t' '
			function median(list, n,  i, j, v) {
				for (i = 2; i <= n; i++) for (j = i; j > 1 && list[j - 1] > list[j]; j--) {
					v = list[j]; list[j] = list[j - 1]; list[j - 1] = v
				}
				return n % 2 ? list[(n + 1) / 2] : (list[n / 2] + list[n / 2 + 1]) / 2
			}
			/^sample=/ {
				for (i = 1; i <= NF; i++) { split($i, kv, "="); f[kv[1]] = kv[2] }
				t = (f["parse_ns"] + f["drop_ns"]) / f["iterations"]
				if (f["engine"] == "yaxml") y[++ny] = t; else l[++nl] = t
			}
			END { if (ny && nl) printf "%.2f", median(l, nl) / median(y, ny); else print "-" }' "$log.run")
		row="$row $ratio |"
	done
	echo "$row" | tee -a "$log"
done
rm -f "$log.run"
echo "raw samples and table: $log"
