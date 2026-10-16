# Input encoding contract

`PushParser` and the byte constructors of `OwnedDocument` detect UTF-8,
UTF-16LE and UTF-16BE. `Reader` and the borrowing `Document` remain UTF-8-only.
Stored strings and event values always use UTF-8. The writer emits UTF-8 only.

UTF-16 BOMs select byte order; a following XML declaration is optional. Without
a BOM, only the four-byte signatures for `<?` select UTF-16, and the leading
declaration must name the matching `UTF-16LE` or `UTF-16BE`. Generic `UTF-16`
requires a BOM. Four UTF-32 BOM forms and four UTF-32 `<` signatures are checked
first and return `UnsupportedEncoding`. No encoding is inferred from an arbitrary
sequence of zero bytes. An additional decoded U+FEFF is content, not a second BOM.

Registered labels are compared without ASCII case sensitivity. A detected
UTF-16 source can declare `UTF-16` with a BOM or its matching endian label.
Conflicting supported labels return `EncodingMismatch`; unknown labels return
`UnsupportedEncoding`. Malformed declaration grammar returns `InvalidDeclaration`
before label agreement. Existing UTF-8-context non-UTF-8 declarations retain
their `UnsupportedEncoding` classification. Unhyphenated `UTF8`/`UTF16` aliases
are not accepted. Declaration views preserve the original spelling.

This follows the [XML encoding agreement rules](https://www.w3.org/TR/2008/REC-xml-20081126/#charencoding)
and [autodetection signatures](https://www.w3.org/TR/2008/REC-xml-20081126/#sec-guessing).
Matching endian labels with a BOM are an explicit tolerance: producers of those
labels should omit the BOM under [RFC 2781](https://www.rfc-editor.org/rfc/rfc2781#section-3.3).
Strict mismatch rejection deliberately differs from the warning behavior in
[libxml2 2.15.4's detected-encoding path](https://raw.githubusercontent.com/GNOME/libxml2/v2.15.4/parserInternals.c).
These source observations do not establish a pinned binary compatibility pass.

Unpaired surrogates and odd final byte counts return `InvalidUtf16`. A surrogate
pair can span any feed boundary. EOF checks decoder truncation before checking
XML closure. A surrogate mismatch identifies the leading high code unit; a
lone low surrogate identifies itself; an odd final byte identifies that byte.
If EOF leaves both a pending high surrogate and an odd byte, the high surrogate
is reported first.

Public positions, available exact input ranges, and current parse errors count
original source bytes. Event metadata uses `SourceOrigin`; `input_range()`
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
rewrites a present declaration encoding to `UTF-8`. UTF-16 output and other
input encodings remain open work. Event-by-event callers remain responsible for
supplying a declaration compatible with UTF-8 output.
