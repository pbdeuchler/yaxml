#!/bin/sh
# Usage: ./oracle.sh [STEP...]
# Runs every libxml2 differential and the W3C conformance run, or the named
# steps:
#   core       yaxml's own tests including the ignored and env-gated
#              differentials, with the oracle environment set
#   harness    this repo's oracle tests (tests/*_oracle.rs, suites, CLIs)
#   replay     fuzz seeds against xmllint (examples/fuzz_replay)
#   structure  leaf insertion/removal against libxml2 (examples/structure_oracle)
#   xmlconf    XMLTS 20130923 on five surfaces; fails on any regression of
#              tests/fixtures/xmlconf-20130923-required.tsv
# Logs and results go to results/oracle-<timestamp>/. Builds use the release
# profile without LTO. The schematron differential needs a libxml2 built with
# schematron, which the pinned oracle is not: set YAXML_SCHEMATRON_HELPER to
# enable it (see yaxml/docs/schematron.md).
set -eu
. "$(dirname "$0")/lib.sh"
help_if "$@"
[ $# -gt 0 ] || set -- core harness replay structure xmlconf
for name in "$@"; do
	case $name in core | harness | replay | structure | xmlconf) ;; *) die "unknown step '$name'; see --help" ;; esac
done
need_yaxml
need_oracle
[ -f "$YAXML_SUITES/xmlconf-catalog.tsv" ] && [ -d "$YAXML_SUITES/xmlconf" ] ||
	die "no XML test suite at $YAXML_SUITES; run ./setup.sh or set YAXML_SUITES"
export YAXML_XMLLINT="$LIBXML2_BUILD/xmllint" YAXML_LIBXML2_SRC="$LIBXML2_SOURCE" \
	YAXML_LIBXML2_SOURCE="$LIBXML2_SOURCE" YAXML_SUITES YAXML_XMLCONF="$YAXML_SUITES/xmlconf"
for pair in CATALOG:catalog DOMEDIT:domedit DTD:dtd ERRORS:errors HTML:html \
	SCHEMA_TYPES:schema_types TEXTREADER:textreader TEXTWRITER:textwriter URI:uri \
	XSD:xsd XMLCONF:xmlconf_catalog PARSE_LIBXML2:parse_bench; do
	export "YAXML_${pair%%:*}_HELPER=$(helper "${pair#*:}")"
done
skips="--skip doubling_input_does_not_quadruple_time"
[ -n "${YAXML_SCHEMATRON_HELPER:-}" ] || skips="$skips --skip libxml2_agrees_in_every_mode"
export CARGO_PROFILE_RELEASE_LTO=false CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16
out=$RESULTS/oracle-$(date +%Y%m%d-%H%M%S)
mkdir -p "$out"
failed=

# step NAME COMMAND...: run COMMAND with its output in $out/NAME.log.
step() {
	name=$1
	shift
	printf '%-10s ' "$name"
	if "$@" >"$out/$name.log" 2>&1; then
		awk '/^test result:/ { p += $4; f += $6; i += $8; n++ }
			/required_pass_regressions=|comparisons|agree/ { line = $0 }
			END { if (n) printf "ok: %d passed, %d failed, %d ignored in %d binaries\n", p, f, i, n
				else print "ok" (line ? ": " line : "") }' "$out/$name.log"
	else
		echo "FAILED (last lines of $out/$name.log):"
		tail -25 "$out/$name.log"
		failed=1
	fi
}

core() {
	cd "$YAXML"
	# shellcheck disable=SC2086 # $skips is a list of flags
	cargo test --release --tests --no-fail-fast -- --include-ignored $skips
}
harness() {
	cd "$HARNESS"
	cargo test --release --tests --no-fail-fast -- --include-ignored --skip child_
}
replay() {
	cd "$HARNESS"
	cargo run --release -q --example fuzz_replay -- --libxml2 "$YAXML_XMLLINT" "$YAXML"/tests/fixtures/fuzz-seeds/*.*
}
structure() {
	cd "$HARNESS"
	cargo run --release -q --example structure_oracle -- --libxml2-helper "$(helper structure)"
}
xmlconf() {
	cd "$HARNESS"
	cargo build --release -q --example xml_conformance &&
		"$TARGET/release/examples/xml_conformance" --manifest "$YAXML_SUITES/xmlconf-catalog.tsv" \
		--suite-root "$YAXML_SUITES/xmlconf" --expected-cases 2585 \
		--required-pass tests/fixtures/xmlconf-20130923-required.tsv >"$out/xmlconf-results.tsv"
}

for name in "$@"; do
	step "$name" "$name"
done
echo "logs: $out"
[ -z "$failed" ] || exit 1
