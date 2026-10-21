# Shared settings for the harness scripts; sourced, not run.
# Locations can be overridden from the environment.

HARNESS=$(cd "$(dirname "$0")" && pwd)
PARENT=$(dirname "$HARNESS")
# yaxml must be the sibling ../yaxml: Cargo.toml and the #[path] includes name
# it. A symlink to a checkout elsewhere works.
YAXML=$PARENT/yaxml
YAXML_ORACLE=${YAXML_ORACLE:-$PARENT/yaxml-oracle}
YAXML_SUITES=${YAXML_SUITES:-$PARENT/yaxml-suites}
FUZZ_TOOLCHAIN=${FUZZ_TOOLCHAIN:-nightly-2026-10-18}
TARGET=${CARGO_TARGET_DIR:-$HARNESS/target}
RESULTS=${RESULTS:-$HARNESS/results}
LIBXML2_SOURCE=$YAXML_ORACLE/libxml2-2.15.4
LIBXML2_BUILD=$YAXML_ORACLE/build
# shellcheck disable=SC2034 # used by the scripts that source this
FUZZ_TARGET=${FUZZ_TARGET_DIR:-$HARNESS/fuzz/target}
export CARGO_TARGET_DIR="$TARGET"

die() {
	echo "error: $*" >&2
	exit 1
}

# help_if "$@": print the script's header comment for -h/--help.
help_if() {
	case ${1:-} in -h | --help)
		sed -n '2,/^[^#]/s/^# \{0,1\}//p' "$0"
		exit 0
		;;
	esac
}

need() {
	command -v "$1" >/dev/null 2>&1 || die "$1 not found; $2"
}

need_yaxml() {
	[ -f "$YAXML/Cargo.toml" ] || die "no yaxml checkout at $YAXML; run ./setup.sh"
}

need_oracle() {
	[ -x "$LIBXML2_BUILD/xmllint" ] && [ -f "$LIBXML2_BUILD/libxml2.a" ] && [ -d "$LIBXML2_SOURCE/test" ] ||
		die "no libxml2 2.15.4 oracle at $YAXML_ORACLE; run ./setup.sh or set YAXML_ORACLE"
}

need_fuzz() {
	need rustup "install it from https://rustup.rs"
	rustup run "$FUZZ_TOOLCHAIN" rustc -V >/dev/null 2>&1 ||
		die "toolchain $FUZZ_TOOLCHAIN missing; run: rustup toolchain install $FUZZ_TOOLCHAIN --component llvm-tools-preview"
	cargo +"$FUZZ_TOOLCHAIN" fuzz --version >/dev/null 2>&1 ||
		die "cargo-fuzz missing; run: cargo install cargo-fuzz --version 0.13.2 --locked"
}

# helper NAME: compile tooling/libxml2_NAME.c against the oracle when stale and
# print the executable's path.
helper() {
	out=$TARGET/helpers/$1
	src=$HARNESS/tooling/libxml2_$1.c
	if [ ! -x "$out" ] || [ "$src" -nt "$out" ] || [ "$LIBXML2_BUILD/libxml2.a" -nt "$out" ]; then
		mkdir -p "$TARGET/helpers"
		${CC:-cc} -std=c11 -O3 -DNDEBUG -D_GNU_SOURCE -Wall -Wextra -Wno-deprecated-declarations \
			-I"$LIBXML2_BUILD" -I"$LIBXML2_SOURCE/include" "$src" "$LIBXML2_BUILD/libxml2.a" \
			-lm -pthread -ldl -o "$out" >&2 || die "cannot compile $src"
	fi
	echo "$out"
}
