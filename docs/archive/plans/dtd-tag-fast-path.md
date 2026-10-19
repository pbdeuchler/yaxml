# Preserve the ordinary start-tag fast path

Base: `7f855f989e33c441593233626a75043a28f41731`. This remains a
private DTD prototype. It does not accept a DOCTYPE in a public parser, change
public events, or claim that the DTD path has passed its activation gates.

## Evidence and decision

The pinned diagnostic in `/private/tmp/yaxml-dtd-tag-performance` compares the
same Rust compiler, release profile, fixtures, iteration counts and semantic
observations before (`304ab59`, whose `src/parser.rs` is byte-identical to
`f43f90b`) and after (`7f855f9`) staged tag preparation. All 100 paired samples
agree on bytes, counters and digest. Every one of the 20 fixture/mode workloads
is slower after staging: median parse-time regressions range from 16.37% to
96.42%; the two central workload regressions are 26.33% and 26.75%.

The regression matches the changed work. An ordinary tag now builds a raw
attribute vector and lexical-name set, builds a second prepared vector, counts
namespace declarations, builds up to three namespace-overlay collections,
builds an expanded-name set, and finally builds the public attribute vector.
The shipped path scanned and normalized once into the public attribute vector,
published namespace declarations directly, then resolved expanded names.

Restore that exact one-pass algorithm for the existing DTD-free
`process_token` path. Keep `RawTag -> prepare_dtd_tag ->
publish_prepared_tag` for a parser or Writer that actually holds completed DTD
semantic state. This is preferable to redesigning `RawTag` and `PreparedTag`
now: fusing those representations could remove some vectors, but atomic
defaults-before-namespaces still requires a complete attribute list and a
rollback-safe namespace overlay. It would retain avoidable work on every
ordinary document while making the reviewed DTD ownership and diagnostic
contracts harder to assess.

The restored ordinary path may update namespace state before a later error,
as the shipped implementation did. Every current frontend makes parser errors
terminal, so that intermediate state cannot be observed or resumed. The DTD
path keeps atomic publication because defaulted namespaces and generated
attributes make preparation failure and later publication a real composition
boundary.

## API boundary

`ParserState::process_token` continues to call a private one-pass
`start_element` for ordinary `TokenKind::Start`. That method directly scans,
normalizes, publishes namespace declarations, resolves names and constructs the
event. Its behavior, offsets, limits and retention calls match `304ab59`.

DTD-aware consumers use one concrete orchestration method:

```text
ParserState::process_dtd_start(
    raw, start, catalog, literal_source, retain
) -> Result<Event, DtdTagError>
```

It performs the parser's encoding, event, depth and token checks; calls
`collect_raw_tag`, `prepare_dtd_tag` and `publish_prepared_tag` exactly once;
and increments the event count only after successful publication. The future
content controller passes its physical or replacement token directly. Writer
uses the same method only when its private DTD state is complete, after its
existing bounded one-token encoding; ordinary Writer events continue through
`process_token`. No optional catalog argument, policy trait, or generic
transaction layer enters the hot path.

Both paths continue to use `TagScanner`, QName validation, normalization,
retention, namespace storage and unwind machinery. Extract only small pure
namespace-declaration classification/constraint helpers where they let both
paths enforce one rule without adding allocation, dynamic dispatch or a new
lifetime abstraction. Active-scope resolution remains separate: ordinary
resolution reads committed bindings, while DTD publication must consult its
pending overlay.

## Implementation steps

1. **Restore the ordinary path.** Reintroduce the `304ab59` one-pass
   `start_element` and its direct namespace-interning helper. Keep all DTD tag
   types and preparation/publication methods available, but remove them from
   ordinary `process_token` execution. Verify the pre-staging DTD-free error
   ordering as well as successful event values across Reader, Push, Buffered,
   DOM and owned DOM.

2. **Add the narrow DTD entry point.** Add `process_dtd_start` with the checks,
   preparation, atomic publication and event-count commit described above.
   Adapt private DTD tests and Writer/controller work to this method; no caller
   may manually duplicate the three-stage sequence. Prove errors do not publish
   bindings, frames or events, and success commits exactly once.

3. **Guard both routes.** Add focused tests that distinguish the routes rather
   than relying only on output equality: an ordinary start tag must not invoke
   DTD collection, while a DTD-aware start must apply defaults and pending
   namespace bindings. Retain exact quota, declaration-location, replacement
   ownership, duplicate-name and release-build tests. Deliberately route the
   ordinary path through staging and bypass the DTD helper once each; named
   tests or the paired performance gate must catch both faults.

4. **Repeat the pinned diagnostic.** Build before and after with the recorded
   compiler and release settings, rerun the same alternating 20-workload matrix,
   and require byte-identical semantic fields. Treat restoration of the ordinary
   distributions near the `304ab59` baseline as the acceptance gate; report
   medians and raw rows without a universal performance claim. Whole-profile,
   fuzz, XMLTS, public DTD activation and matched-libxml2 performance remain
   separate gates.

## Definition of done

The public DTD-free parser uses the shipped one-pass start-tag path with its
prior behavior and resource accounting. DTD-aware Reader/controller and Writer
code have one shared concrete orchestration entry point whose failures are
atomic and whose success commits one event. Focused debug/release tests, strict
Clippy and formatting pass, deliberate route faults are caught and restored,
and the pinned A/B diagnostic shows semantic identity with the ordinary-path
regression removed or reduced to measurement noise.
