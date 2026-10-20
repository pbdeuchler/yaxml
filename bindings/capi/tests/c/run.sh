#!/bin/sh
# Builds the C ABI test against the staticlib with ASan and UBSan, runs it
# (LeakSanitizer included on Linux), and on macOS also runs an uninstrumented
# build under `leaks`, since LeakSanitizer is unavailable there.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
capi=$(cd "$here/../.." && pwd)
target=${CARGO_TARGET_DIR:-$capi/../target}
out=$target/abi-test
mkdir -p "$out"
(cd "$capi" && cargo build --release --lib)
lib=$target/release/libyaxml_capi.a
cflags="-std=c11 -Wall -Wextra -Werror -pedantic -g -I$capi/include"
case "$(uname)" in
  Darwin) libs="-lSystem -lc -lm" ;;
  *) libs="-lgcc_s -lutil -lrt -lpthread -lm -ldl -lc" ;;
esac
${CC:-cc} $cflags -fsanitize=address,undefined -fno-sanitize-recover=all \
  "$here/abi_test.c" "$lib" $libs -o "$out/abi_test_sanitized"
ASAN_OPTIONS=${ASAN_OPTIONS:-detect_leaks=$( [ "$(uname)" = Darwin ] && echo 0 || echo 1 )} \
  UBSAN_OPTIONS=print_stacktrace=1:halt_on_error=1 "$out/abi_test_sanitized"
if [ "$(uname)" = Darwin ]; then
  ${CC:-cc} $cflags "$here/abi_test.c" "$lib" $libs -o "$out/abi_test"
  MallocStackLogging=1 leaks --atExit -- "$out/abi_test"
fi
