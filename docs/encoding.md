# Input encoding contract

`PushParser`, `BufferedReader` and the byte constructors of `OwnedDocument`
detect UTF-8, UTF-16LE and UTF-16BE, and follow a BOM-less declaration naming
one of libxml2 2.15.4's built-in single-byte encodings: US-ASCII, ISO-8859-1
through ISO-8859-16 (no 12) and windows-1252. `Reader` and the borrowing
`Document` take UTF-8; they read other input through [`Transcoded`](#transcoded-input).
Stored strings and event values always use UTF-8.
`Writer` emits UTF-8 only; `save` also writes UTF-16 and these single-byte
encodings ([writer.md](writer.md)).

UTF-16 BOMs select byte order; a following XML declaration is optional. Without
a BOM, only the four-byte signatures for `<?` select UTF-16, and the leading
declaration must name the matching `UTF-16LE` or `UTF-16BE`. Generic `UTF-16`
requires a BOM. Four UTF-32 BOM forms and four UTF-32 `<` signatures are checked
first, then the EBCDIC `<?xm` signature; all return `UnsupportedEncoding`, as
libxml2 has no built-in decoder for them. No encoding is inferred from an arbitrary
sequence of zero bytes. An additional decoded U+FEFF is content, not a second BOM.

Labels resolve through libxml2's `xmlEncTable` aliases (`latin1`, `l2`, `utf8`,
`csUnicode`, ...), compared without ASCII case sensitivity and without trimming.
Its quirk mapping `iso8859-10`, `-13`, `-14` and `-15` to ISO-8859-1 is preserved.
Labels libxml2 decodes only through iconv/ICU (Shift_JIS, EUC-JP, ISO-2022-JP,
UCS-2, UCS-4, IBM037, HTML) and unknown labels return `UnsupportedEncoding`.
The pinned oracle (`yaxml-oracle`) is built without iconv, ICU and ISO8859X: its
UCS-4, UCS-2 and IBM037 handlers have no converter, so xmllint rejects those
labels and the UCS-4/EBCDIC signatures, and yaxml matches by rejecting. A
libxml2 built with iconv (most distributions) would accept them; supporting that
is a possible low-priority future item.
An auto-detected encoding (UTF-8 BOM, UTF-16 BOM or `<?` signature) outranks the
declared label, as in `xmlSetDeclaredEncoding`: UTF-8 allows `UTF-8`/`UTF8`,
UTF-16LE `UTF-16`/`UTF-16LE`/`UTF16`, UTF-16BE `UTF-16`/`UTF-16BE`/`UTF16`. Any
other well-formed label, supported or not, makes libxml2 warn ("Encoding 'X'
doesn't match auto-detected 'UTF-8'") and keep the detected encoding; yaxml
keeps the same warning (`ErrorKind::Warning(Warning::EncodingMismatch)` in
`errors()`) and `Declaration.encoding` records the
detected name (`UTF-8`, `UTF-16LE`, `UTF-16BE`), not the label. Malformed
declaration grammar returns `InvalidDeclaration`. BOM-less UTF-16 still requires
a declaration with an encoding (`EncodingMismatch`). An undetected source
(no BOM) switches to a declared single-byte charset; in the borrowing reader any
non-UTF-8 label returns `UnsupportedEncoding` (transcode it first). Declaration
views preserve the original spelling.

A single-byte encoding applies to the bytes after the ASCII declaration, which
was forwarded as UTF-8. Bytes below 0x80 are forwarded in bulk; others map
through tables generated from libxml2's `codegen/charset.inc` by
`tooling/gen_charsets.py`. A byte libxml2 leaves unmapped (every high byte in
US-ASCII) returns `UnmappedByte` at that byte. `tests/fixtures/charsets.tsv`,
recorded from an ISO8859X-enabled xmllint by `tooling/charset_oracle.py`,
independently checks every table. There is no caller encoding override and no
pluggable converter (`xmlCtxtSetCharEncConvImpl`).

This follows the [XML encoding agreement rules](https://www.w3.org/TR/2008/REC-xml-20081126/#charencoding)
and [autodetection signatures](https://www.w3.org/TR/2008/REC-xml-20081126/#sec-guessing).
Matching endian labels with a BOM are an explicit tolerance: producers of those
labels should omit the BOM under [RFC 2781](https://www.rfc-editor.org/rfc/rfc2781#section-3.3).
Mismatches follow [libxml2 2.15.4's detected-encoding path](https://raw.githubusercontent.com/GNOME/libxml2/v2.15.4/parserInternals.c)
and were checked against the pinned xmllint (XMLTS `hst-lhs-007`: libxml2
accepts, the suite expects not-well-formed).

Unpaired surrogates and odd final byte counts return `InvalidUtf16`. A surrogate
pair can span any feed boundary. EOF checks decoder truncation before checking
XML closure. A surrogate mismatch identifies the leading high code unit; a
lone low surrogate identifies itself; an odd final byte identifies that byte.
If EOF leaves both a pending high surrogate and an odd byte, the high surrogate
is reported first.

Public positions, available exact input ranges, and parse errors, returned or
kept in `errors()`, count original source bytes. Event metadata uses `SourceOrigin`; `input_range()`
returns its optional exact input span. All currently supported parsed origins
have such a span. Semantic errors are translated once from the unnormalized token; consumer
errors already use source coordinates. A declaration's source range includes its
original label. `position()` counts consumed signature and incomplete code-unit
bytes and can include one decoded scalar of text-delimiter lookahead. It need
not consume the remainder of a failed chunk.

`Limits::max_input_bytes` includes transport BOM bytes; `max_token_bytes` measures
raw source tokens without that BOM. `DecodeLimits::max_decoded_bytes` defaults to
64 MiB and `max_decoded_token_bytes` to 1 MiB. Both measure decoded UTF-8 bytes,
excluding the transport BOM, even for UTF-8 input. Use
`PushParser::with_decode_limits(limits, decode_limits)` or
`OwnedDocument::with_decode_limits(input, limits, decode_limits)` to change them.
Their errors are `LimitExceeded(DecodedBytes)` at offset zero and
`LimitExceeded(DecodedTokenBytes)` at the original token start. Limits apply
before retaining excess decoded data; they do not establish a process memory quota.

The decoder uses constant carry and never retains a complete decoded source or
a document-wide offset map. The tokenizer retains one bounded UTF-8 token; the
owned tree copies logical values into its existing pool under the shared string
budget. `Writer::write_owned_document` accepts such a tree, emits UTF-8, and
rewrites a present declaration encoding to `UTF-8`; `save::to_vec` writes the
declared or requested encoding. iconv-only encodings remain open work. Event-by-event callers remain responsible for
supplying a declaration compatible with UTF-8 output.

## Transcoded input

`Transcoded::new(input, &options)` decodes complete input once for
`Document::parse_transcoded` and `Reader::transcoded`, with the decoder and
rules above; UTF-8 input (with or without a BOM) is borrowed, not copied. It
checks `max_input_bytes` against the input and `max_decoded_bytes` while
decoding, so the copy is bounded before it is made; `max_decoded_token_bytes`
applies to the parse, and `max_token_bytes` there measures decoded tokens. The
parser starts in the detected encoding, so declarations are checked as a stream
checks them. `IGNORE_ENC` ignores a declared charset, as streams do; under
`RECOVER` a declaration whose errors drop its label does not switch.

A decoding error (`InvalidUtf16`, `UnmappedByte`, `UnsupportedEncoding`) is
returned by `Transcoded::new` at its input offset, before any later markup
error, as the slice reader reports invalid UTF-8 first. Under `RECOVER` it ends
the text instead, and the parse recovers from it at that point, keeping the
text decoded before it as `xmllint --recover` does (the streams drop text
pending at the error).

Errors, `errors()`, origins and `position()` of a transcoded parse count bytes
of the UTF-8 text, not of the input, unlike the streams and `OwnedDocument`.
`Transcoded::source_offset` maps one to the input (linear in the offset; no
offset map is kept), and `LineIndex::transcoded` describes them with libxml2's
lines and columns, which are the same for the decoded text.
