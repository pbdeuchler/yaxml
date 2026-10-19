# URI module

`yaxml::uri` (`Uri`, `UriError`, `resolve`, `relative`, `escape`, `unescape`,
`normalize_path`, `canonic_path`) reimplements libxml2 2.15.4 `uri.c` for Unix.
Behavior is libxml2's, not RFC 3986's, wherever they differ. `xmlPathToURI` is
`canonic_path`; `Uri::parse_raw` is `xmlParseURIRaw(s, 1)`.

## Differential

```sh
cc -std=c11 -Wall -Wextra -Werror -Itarget/oracle/build \
  -Itarget/oracle/libxml2-2.15.4/include tooling/libxml2_uri.c \
  target/oracle/build/libxml2.a -lm -lz -o target/oracle/uri-helper
YAXML_URI_HELPER="$PWD/target/oracle/uri-helper" cargo test --test uri_oracle -- --nocapture
```

The test (skipped without the variable) sends about 120,000 requests (curated,
RFC 3986 section 5.4 and generated token soup) for parse, raw parse, save,
`xmlBuildURI`, `xmlBuildRelativeURI`, escape, unescape, normalize and canonic
path, and requires byte-identical results: 0 mismatches at last run.

## Retained libxml2 behavior

- `../../../g` resolves to `http://a/../g`, and `/./g`, `/../g` are not
  normalized (RFC: `http://a/g`); `normalize_path("/..")` is `/..`.
- The IPv4 branch of the host parser never matches (missing `cur++`); digits
  are an ordinary reg-name, so `1.2.3.4x` is a host and `256` is accepted.
- Port 0 equals no port; ports above `i32::MAX` fail; `//` with no host or port
  after a scheme is `empty_authority`, but `///x` as a relative reference is not.
- An empty userinfo (`http://@/`) is unescaped with length 0, i.e. as the whole
  remaining string.
- Hosts are saved without escaping; `query` is saved as written.
- A reference without a scheme against a base without `://` is a filesystem
  path (unescaped, normalized, fragment kept verbatim).

## Intentional differences

- Strings are UTF-8. Percent-decoding to non-UTF-8 bytes is
  `UriError::InvalidUtf8` (libxml2 returns the bytes); the differential skips
  those cases (about 1%). Decoded NUL truncates a component, as in C.
- No `opaque`/`authority` fields (never set by the parser), no `xmlURIEscape`,
  no 1 MiB output cap, no `NULL` base (pass the reference through yourself) and
  no Windows drive-letter handling.
- `UriError::Syntax` carries the furthest offset reached; libxml2 returns 1.
