# Streaming contract and implementation

`PushParser` accepts byte chunks without retaining the complete input. `feed`
does not mean EOF; `finish` flushes pending text and requires a completed root.
Empty feeds are harmless before finish. Text has one event per lexical run,
independent of chunk sizes. Markup spans include delimiters. Callbacks receive
`&Event<'_>` and a `SourceOrigin`: the event, its names, attribute list and
normalized values are lent from parser storage for the call and reused after
it, so delivery neither moves nor allocates; copy what you keep.
`origin.input_range()` gives the optional exact original-byte span, including
markup delimiters. All currently supported parsed origins have an input span.

`ControlFlow::Break(())` delivers that event, then returns `ErrorKind::Aborted`
at its end offset. Cancellation and parser errors are sticky. Successful finish
is idempotent and delivers no further events. Any feed after successful finish,
including an empty chunk, starts a sticky `InvalidState` error. A failed feed can
leave an input suffix unconsumed. `position()` includes encoding detection and
one decoded scalar of lookahead when `<` terminates text (text in a
DOCTYPE document instead consumes a terminating general entity reference `&name;`,
which must be read to tell it from a predefined reference); origin input ranges identify the
logical event precisely.

`event_origin()` is absent before the first delivery and retains the latest
delivered origin after EOF, cancellation or failure. `event_range()` derives its
optional exact input span. Neither accessor controls physical source progress.
Events produced by entity expansion have no `input_range()`; their
`physical_context()` is the outermost reference and `replacement_range()` the
span inside the innermost replacement text.

A callback panic propagates normally through Rust unwinding. If the caller
recovers it, the parser remains poisoned with `InvalidState`; partially delivered
tokens cannot be resumed. Ordinary returned cancellation uses `Aborted` instead.

`PushParser::read_from` consumes its parser and a caller-provided `Read`, using an
8 KiB stack buffer. It retries `Interrupted`, preserves other I/O failures in
`ReadError::Io`, and reports XML/cancellation errors in `ReadError::Xml`. It stops
reading immediately after a parser failure. Parsing opens no document-controlled
resources. A parser already in a terminal error returns that error before touching
the reader. A successfully finished parser returns `InvalidState` before I/O.

`BufferedReader<R: BufRead>` owns a caller-provided buffered source and lends one
event plus its `SourceOrigin` from `read_event`. It consumes exactly the
raw prefix accepted by the shared decoder. A large visible suffix remains in
`R`; callers with only `Read` wrap it in `std::io::BufReader`. `get_ref` permits
inspection without mutation, while `into_inner` returns the source at the
parser-consumed position without draining or validating the remainder.
`dtd` returns the DTD's declarations from the `Doctype` event on, for
validators that need notations and unparsed entities.

The pull reader retries `Interrupted`. It returns an original I/O error once,
then becomes `InvalidState` without further I/O. XML errors remain sticky and
successful EOF repeats without source access. It validates trailing input only
when advanced to final EOF; returning the root end event does not establish a
complete document. At the exact input-byte ceiling it probes the source once:
empty input finalizes, an I/O error is preserved, and a nonempty slice produces
`InputBytes` without consuming the excess byte. A caught source panic poisons the
reader. Async adapters remain separate work.

## Options and entity references

`PushParser::with_options` and `BufferedReader::with_options` (like
`Reader::with_options`) take the [`ParseOptions`](compatibility.md#option-compatibility)
flags a stream can honor; `DTDVALID`, `NOBLANKS` and `XINCLUDE` need a tree
and are `UnsupportedOption`. `NOCDATA`, `NSCLEAN` and `IGNORE_ENC` change
tokens on every surface.

Without `NOENT` a general entity reference in content is reported as
`Event::EntityStart(name)`, then the replacement text's events, then
`Event::EntityEnd`; with `NOENT` only the replacement's events appear. This
is libxml2's SAX `reference` callback plus the content its tree builder
attaches to the reference node, in one stream. A lone reference event would
force DOM and reader consumers to parse each entity again, as libxml2 does
with `xmlCtxtParseEntity`; markers keep one expansion engine, identical
limits, errors and origins in both modes, and let a consumer that ignores
the two events see exactly the substituted stream. A reference libxml2 keeps
without content (an unloaded external entity, or an undeclared one it
tolerates) is a start directly followed by its end. Both markers carry the
reference's origin: the `&name;` span, or for a reference inside replacement
text its replacement range under the outermost reference. They do not count
toward `max_events`; `max_entity_references` bounds them. `Writer` writes
nothing for them, so copying events writes the replacement text.

## Shared semantics and storage

The slice reader and push parser share document phases, name/reference validation,
normalization, namespace scope, and event budgets through `ParserState`. Its
retention callback preserves borrowed slice strings or copies names needed after
streaming token storage is reused. Namespace identities avoid rehashing long URIs
per attribute. Reference counts release interned URIs when their last retained
binding leaves scope, including aliases and shadowed ancestor bindings.

The decoder retains at most four signature bytes, one partial UTF-16 code unit,
one high surrogate, and one decoded scalar suffix across pull suspension. It
forwards borrowed UTF-8 chunks or complete UTF-16 scalars through a stack buffer.
The lexer retains at most one decoded token. It
tracks partial comment, CDATA, PI and quoted-tag delimiters across chunks. Ordinary
byte runs use `memchr` searches and bulk copies. It never restarts scanning from
the beginning of an accumulated token. Requested scratch capacity grows
geometrically up to the decoded ceiling and the maximum decoded size implied by
the source ceiling, with limits checked before reservation. The latter is the
raw ceiling for UTF-8 and `3 * floor(raw / 2)` for UTF-16, capped without overflow.

Remaining parser state depends on open elements and active/shadowed namespace
bindings; metadata capacities retain their high-water marks. Completed sibling
namespace declarations do not accumulate strings or interner entries. Callbacks
that save their own snapshots necessarily retain additional caller-owned memory.

For push parsing, `max_input_bytes` counts all supplied chunks and is checked
against each complete chunk before processing. Pull parsing counts only the raw
prefix accepted by the decoder. Depth, attributes, events and raw token limits
apply as for the slice reader. `max_owned_bytes` counts cumulative requested capacity for
normalization, namespace copies, and retained structural strings, even after
buffers are freed. Streaming therefore spends more of it than borrowing slices.
This is an allocation-work budget, not an exact resident-memory quota: lent
normalized values are charged although their storage is reused. Ordinary
unchanged token bytes spend scratch capacity without a cumulative string charge.
`DecodeLimits` bounds the decoded UTF-8 total and each decoded token independently
of source bytes. Transport BOMs count toward source input and neither decoded
counter. See [encoding](encoding.md) for constructors and exact detection policy.

## Error ordering and compatibility

The slice constructor validates UTF-8 and XML characters across the whole input.
The push parser validates each completed token, then applies the shared grammar.
It can emit valid prefix events before seeing a later error. When input contains
several defects or simultaneously exceeds several limits, the first error and
its discovery position can differ between frontends. Final acceptance and valid
logical events agree under sufficient budgets; isolated errors have split-point
regressions. A truncated token reports final EOF, while an incomplete feed waits.

All APIs process the internal DTD subset and emit one `Doctype` event at its
end; DTD-free documents stay on the plain token loop, where options cost a
flag test per CDATA section, declaration and namespace declaration. Streaming retains the
declaration text (charged to `max_owned_bytes`) only until that event is
delivered. Entity expansion resumes across arbitrary chunk splits, so events
and errors do not depend on chunk boundaries. Complete-input borrowing remains UTF-8-only; push, pull,
and owned parsing also support UTF-16. UTF-16 output, other encodings, external entities,
allocator-backed collections and recovery modes remain open
items in [the roadmap](roadmap.md). Streaming support does not establish complete
libxml2 reader/SAX compatibility or the performance goal.
