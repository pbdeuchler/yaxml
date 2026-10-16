# Complete DOM serialization plan

This P1 slice serializes complete borrowed and owned DOMs through the existing
bounded UTF-8 `Writer`. It adds no string/byte convenience sink, formatting,
namespace repair, subtree output, UTF-16 output, or save-options framework.

## Public contract

```rust
impl<W: std::io::Write> Writer<W> {
    pub fn write_document(
        &mut self,
        document: &Document<'_>,
    ) -> Result<(), WriteError>;

    pub fn write_owned_document(
        &mut self,
        document: &OwnedDocument,
    ) -> Result<(), WriteError>;
}
```

Both methods require a fresh writer. Any earlier `write_event` call, including
an ignored empty text event, consumes freshness. Misuse produces terminal
`InvalidState` before sink I/O.

A successful call writes the complete document, validates closure, flushes the
sink, and leaves the writer finished. Repeated `finish` remains harmless;
`into_inner` remains available after success or failure. XML validation,
allocation, sink, and recovered-panic failures keep the existing terminal and
partial-output contracts. Output error offsets count prospective or confirmed
UTF-8 output bytes, never original DOM coordinates.

## Fixed output policy

- Emit UTF-8 with no BOM, indentation, inserted separators, or trailing newline.
- Preserve declaration absence, version, and standalone. Rewrite a present
  encoding value to canonical `UTF-8`, including DOMs parsed from UTF-16.
- Preserve qualified and expanded names, explicit namespace declarations,
  attribute order, document-child order, text, CDATA, comments, and PI data.
- Emit every childless element as `<name/>`. The DOM does not retain the lexical
  distinction between `<name/>` and `<name></name>`.
- Preserve empty CDATA, comment, and PI nodes. Top-level whitespace was not
  retained during parsing and is not reconstructed.
- Keep the Writer's established escaping and strict-character policy. Byte
  output intentionally differs from libxml2 where its save layer adds document
  newlines, reorders namespace declarations, or escapes an attribute `>`.

Whole valid documents retain every ancestor namespace declaration, so this
operation needs no namespace repair. Detached subtree serialization is deferred:
it can otherwise emit prefixes whose binding lived outside the subtree.

## Implementation shape

Add an explicit fresh state to `Writer`. Refactor the public operation guards so
document traversal is poisoned before allocation, validation, or I/O, then uses
private event-writing and finish helpers without reopening the lifecycle.

A new private `serialize` module adapts `Node` and `OwnedNode` to borrowed event
views. It walks `first_child`, `next_sibling`, and `parent` links iteratively:
enter a nonempty element with `Start`, emit leaves or `Empty`, and emit `End`
while climbing to the next sibling. Each edge is visited a constant number of
times; no recursion, traversal stack, complete event list, or whole-output
buffer is introduced.

Use one reusable attribute-metadata vector per operation. Check
`WriterLimits::max_attributes` before growth, reserve fallibly, and borrow every
qualified name, namespace URI, and normalized value. Do not clone `Cow::Owned`
strings. Document and sink storage remain caller-owned; temporary metadata is
bounded by the per-element attribute ceiling.

## Five implementation steps

1. **Freeze lifecycle and byte policy.** Add the fresh writer state, private
   guarded helpers, and declaration rewrite tests. Verify invalid reuse, final
   flush failures, panic poisoning, positions, and `into_inner` behavior.
2. **Serialize both immutable tree forms.** Add iterative traversal and borrowed
   node/event adapters. Borrowed and owned forms of the same logical document
   must emit identical bytes without recursive calls or unbounded metadata.
3. **Prove semantics and limits.** Add byte goldens and strict tree comparisons
   for every node kind, prolog/epilog nodes, empty payloads, mixed content,
   normalization, namespace shadowing/reset, Unicode, all writer ceilings, and
   short/interrupted/zero/oversized/erroring/panicking sinks. Exercise depth
   8,192 on a small-stack thread and wide sibling series.
4. **Extend fuzz and measurement.** Serialize accepted borrowed and owned trees,
   require small valid cases to succeed, reparse to complete exhaustion, compare
   strict logical structure, and add exact quota goldens and faulting sink
   schedules. Update the target inventory and benchmark borrowed/owned document
   serialization with counting and buffered sinks; build trees outside timing.
5. **Publish and review evidence.** Update writer/owned/encoding/roadmap docs,
   collect fuzz-only and ordinary raw coverage with no production exclusions,
   run all targets/doctests/fmt/strict Clippy/dependency/unsafe audits, and run an
   independent holistic review with mutations for empty output, sibling/end-tag
   loss, empty CDATA loss, namespace loss, UTF-16 label forwarding, cumulative
   limit reset or double charging, blanket `OwnedBytes` rejection, swallowed
   flush failure, and recursive traversal. The blanket rejection and accounting
   mutations must fail the bounded-valid and exact-quota fuzz assertions,
   including owned UTF-16 input; ordinary-test failures alone do not close those
   fuzz behavior gaps.

## Acceptance boundary

Semantic oracles compare declaration presence, version, and standalone, and
expect `UTF-8` when the input declaration had any encoding value. They also
compare node kinds and exhaustion, qualified/expanded names, ordered attributes
including XMLNS declarations, PI targets/data, comments, CDATA, and logical
values. They exclude original source ranges and original empty-element spelling.
The existing XPath/libxml2 projection runs on input and output as a secondary
oracle, but it cannot replace strict tree and byte checks because it merges
text/CDATA and omits declarations and XMLNS attributes.

Writer `max_events` counts emitted output events after childless-element folding.
`<r></r>` therefore succeeds at one event, `<r><![CDATA[]]></r>` requires Start,
CDATA, and End, and a retained declaration adds one event. Below/exact/above
goldens must use these output counts rather than the source parser's event count.

Performance comparisons with libxml2 require matched `xmlSaveDoc` encoding,
formatting, declaration, empty-element, lifecycle, and sink work. No comparison
uses xmllint process timing or claims byte identity. The pinned references are
[libxml2 save API](https://github.com/GNOME/libxml2/blob/v2.15.4/include/libxml/xmlsave.h),
[document save implementation](https://github.com/GNOME/libxml2/blob/v2.15.4/xmlsave.c#L1225),
[namespace emission](https://github.com/GNOME/libxml2/blob/v2.15.4/xmlsave.c#L769),
and [namespace reconciliation](https://github.com/GNOME/libxml2/blob/v2.15.4/tree.c#L5645).
