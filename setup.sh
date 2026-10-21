#!/bin/sh
# Usage: ./setup.sh [STEP...]
# Prepares what the other scripts need, skipping what already exists. Steps
# (default: oracle suites):
#   oracle   libxml2 2.15.4 source and a static build with xmllint in
#            $YAXML_ORACLE (default ../yaxml-oracle, beside this checkout;
#            checksum-verified tarball); needs curl, tar, cmake, a C compiler
#   suites   the W3C XML Test Suite 20130923 and its catalog TSV in
#            $YAXML_SUITES (default ../yaxml-suites); needs the oracle
#   corpus   optional: clones the full fuzz corpus from $YAXML_CORPUS_URL
#            (default https://github.com/pbdeuchler/yaxml-corpus.git) into
#            fuzz/corpus-full, which fuzz.sh and coverage.sh then use instead
#            of the minimized fuzz/corpus; needs git
set -eu
. "$(dirname "$0")/harness/lib.sh"
help_if "$@"
[ $# -gt 0 ] || set -- oracle suites
for name in "$@"; do
	case $name in oracle | suites | corpus) ;; *) die "unknown step '$name'; see --help" ;; esac
done

LIBXML2_URL=https://download.gnome.org/sources/libxml2/2.15/libxml2-2.15.4.tar.xz
LIBXML2_SHA256=98087fd181d9070724f3fbc65c7377db03038eb92bd882374daff44940138821
XMLTS_URL=https://www.w3.org/XML/Test/xmlts20130923.tar.gz
XMLTS_SHA256=9b61db9f5dbffa545f4b8d78422167083a8568c59bd1129f94138f936cf6fc1f

# fetch URL SHA256 FILE: download unless present, then verify the checksum.
fetch() {
	if [ ! -f "$3" ]; then
		curl --fail --silent --show-error --location --retry 3 --max-time 300 --output "$3.part" "$1"
		mv "$3.part" "$3"
	fi
	if command -v sha256sum >/dev/null 2>&1; then
		actual=$(sha256sum "$3" | cut -d' ' -f1)
	else
		actual=$(shasum -a 256 "$3" | cut -d' ' -f1)
	fi
	[ "$actual" = "$2" ] || die "$3: sha256 $actual, expected $2 (delete it to download again)"
}

oracle() {
	if [ -x "$LIBXML2_BUILD/xmllint" ] && [ -f "$LIBXML2_BUILD/libxml2.a" ] && [ -d "$LIBXML2_SOURCE/test" ]; then
		echo "oracle: $YAXML_ORACLE ($("$LIBXML2_BUILD/xmllint" --version 2>&1 | head -1))"
	else
		need curl "install curl"
		need cmake "install cmake (https://cmake.org)"
		mkdir -p "$YAXML_ORACLE"
		fetch "$LIBXML2_URL" "$LIBXML2_SHA256" "$YAXML_ORACLE/libxml2-2.15.4.tar.xz"
		tar -xJf "$YAXML_ORACLE/libxml2-2.15.4.tar.xz" -C "$YAXML_ORACLE"
		# The pinned oracle configuration: static, no iconv/ICU/ISO8859X/zlib.
		cmake -S "$LIBXML2_SOURCE" -B "$LIBXML2_BUILD" \
			-DCMAKE_BUILD_TYPE=Release -DBUILD_SHARED_LIBS=OFF \
			-DLIBXML2_WITH_ICONV=OFF -DLIBXML2_WITH_ISO8859X=OFF \
			-DLIBXML2_WITH_ICU=OFF -DLIBXML2_WITH_ZLIB=OFF \
			-DLIBXML2_WITH_PYTHON=OFF -DLIBXML2_WITH_DOCS=OFF \
			-DLIBXML2_WITH_TESTS=OFF -DLIBXML2_WITH_PROGRAMS=ON \
			-DLIBXML2_WITH_XPATH=ON -DLIBXML2_WITH_OUTPUT=ON >"$YAXML_ORACLE/configure.txt"
		cmake --build "$LIBXML2_BUILD" --target xmllint --parallel 4 >"$YAXML_ORACLE/build.txt"
		"$LIBXML2_BUILD/xmllint" --version >"$YAXML_ORACLE/version.txt" 2>&1
		echo "oracle: built $YAXML_ORACLE"
	fi
}

suites() {
	need_oracle
	if [ -f "$YAXML_SUITES/xmlconf/xmlconf.xml" ]; then
		echo "suite: $YAXML_SUITES/xmlconf"
	else
		need curl "install curl"
		mkdir -p "$YAXML_SUITES"
		fetch "$XMLTS_URL" "$XMLTS_SHA256" "$YAXML_SUITES/xmlts20130923.tar.gz"
		tar -xzf "$YAXML_SUITES/xmlts20130923.tar.gz" -C "$YAXML_SUITES"
		echo "suite: extracted $YAXML_SUITES/xmlconf"
	fi

	catalog=$YAXML_SUITES/xmlconf-catalog.tsv
	if [ -f "$catalog" ]; then
		echo "catalog: $catalog"
	else
		# Metadata comes from libxml2 expanding the suite's trusted descriptors.
		XML_CATALOG_FILES='' SGML_CATALOG_FILES='' "$(helper xmlconf_catalog)" \
			"$YAXML_SUITES/xmlconf/xmlconf.xml" "$YAXML_SUITES/xmlconf" >"$catalog.part"
		[ "$(wc -l <"$catalog.part")" -eq 2586 ] || die "unexpected catalog size in $catalog.part"
		mv "$catalog.part" "$catalog"
		echo "catalog: generated $catalog"
	fi
}

corpus() {
	full=$ROOT/fuzz/corpus-full
	url=${YAXML_CORPUS_URL:-https://github.com/pbdeuchler/yaxml-corpus.git}
	if [ -d "$full" ]; then
		echo "corpus: $full"
		return
	fi
	need git "install git"
	rm -rf "$full.part"
	if GIT_TERMINAL_PROMPT=0 git clone --quiet "$url" "$full.part"; then
		mv "$full.part" "$full"
		echo "corpus: cloned $url into $full"
	else
		rm -rf "$full.part"
		echo "corpus: could not clone $url (set YAXML_CORPUS_URL?); fuzz.sh and coverage.sh keep using the minimized fuzz/corpus" >&2
	fi
}

for name in "$@"; do
	"$name"
done
