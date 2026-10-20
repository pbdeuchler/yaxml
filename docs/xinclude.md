# XInclude and XPointer

`yaxml::xinclude::process(&document, base, resolver, &options)` performs
[XInclude 1.0](https://www.w3.org/TR/xinclude/) as libxml2 2.15.4's
`xinclude.c` does and returns a new `OwnedDocument`; the input is never
changed. `yaxml::xpointer::evaluate(&tree, pointer, &limits)` is the
`xpointer.c` subset 2.15 keeps, over either DOM.

```rust
use yaxml::{OwnedDocument, Request, xinclude};

let main = OwnedDocument::parse(
    "<r xmlns:xi='http://www.w3.org/2001/XInclude'>\
     <xi:include href='t.txt' parse='text'><xi:fallback>none</xi:fallback></xi:include></r>",
)?;
let resolver = |_: Request<'_>| Ok(None); // declines: the fallback is used
let result = xinclude::process(&main, "main.xml", Box::new(resolver), &Default::default())?;
assert_eq!(result.root_element().text_chunks().collect::<String>(), "none");
# Ok::<(), Box<dyn std::error::Error>>(())
```

## libxml2 mapping

| libxml2 | yaxml |
| --- | --- |
| `xmlXIncludeProcess*`, `XML_PARSE_XINCLUDE` | `xinclude::process` on a parsed document |
| `XML_PARSE_NOXINCNODE` | Always: no `XINCLUDE_START`/`END` nodes (the DOM has no such kind) |
| `XML_PARSE_NOBASEFIX` | `Options::base_fixup = false` |
| `xmlXIncludeSetResourceLoader`, `XML_RESOURCE_XINCLUDE{,_TEXT}` | The `Resolver`; `ResourceKind::XInclude`, `XIncludeText` |
| `XINCLUDE_MAX_DEPTH` (40) | `Options::max_depth` |
| `xmlXPtrEval` | `xpointer::evaluate` |

Behavior follows the C code: `parse="xml"` and `parse="text"` (with
`encoding` through the built-in charsets; "UTF-16" is little-endian with any
BOM kept), shorthand, child-sequence, `element()`, `xmlns()`, `xpointer()`
and `xpath1()` pointers, `href` fragments only in the 2001 namespace,
`xi:fallback` (including nested inclusions), per-call caches of documents and
texts by URI, loop detection, `xml:base` fixup with libxml2's relative URIs,
namespace declarations added to copies whose prefixes are declared outside
them, an empty `<!DOCTYPE root>` when an included document had a DTD, and IDs
(including DTD-declared ones) carried into the result. Included documents are
parsed with the caller's resolver, so their external DTDs load too.

Errors carry libxml2's `XML_XINCLUDE_*` category, the document URI and the
element's source offset. Every condition libxml2 counts as an error fails
the call; a resource the resolver declines or fails to supply, or one that
is not well-formed, uses the fallback (`NoFallback` without one).

## Budgets

`Options::limits` applies to each included document's parse.
`max_external_loads` and `max_external_bytes` are cumulative across the call,
including the included documents' DTDs; `max_events` bounds result nodes and
nodes visited while expanding (document nodes are not counted, as in parsing,
so a document without inclusions is accepted whenever its parse is), and
`max_owned_bytes` the result's strings, so
inclusion bombs (documents that include the next level many times) stop with
`LimitExceeded` even though each document loads once. `Options::xpath` bounds
each pointer.

## libxml2 suite

`cargo test --test xinclude` runs `test/XInclude/{docs,without-reader,issue1120}`
(28 inputs) against `result/XInclude`, saving results with `save::to_vec`
(`xmlDocDumpMemory`): **all 28 match** byte for byte, including the
regenerated internal subset and ISO-8859-1 output, and the four failing
inputs fail with libxml2's first error.

The `--noxincludenode` runs of the suite expect the same bytes.

## Differences and gaps

- An unqualified element copied below a default namespace gets `xmlns=""`;
  libxml2 omits it, changing the element's namespace.
- Namespace declarations precede other attributes in the result, as libxml2
  stores them; adjacent Text nodes are not merged.
- With marker nodes, libxml2 writes `<p></p>` for an element left empty by an
  inclusion; the result here is `<p/>`, as with `NOXINCNODE`.
- Entity declarations are not merged into the result's DTD (entities are
  always expanded in yaxml trees); the `ENTITY_DEF_MISMATCH` check is absent.
- `xml:lang` fixup (XInclude 1.0 section 4.5.6) is opt-in through
  `Options::lang_fixup`; libxml2 never performs it.
- An inclusion inside a fallback keeps the base fixup computed for its own
  element, not recomputed against the outer inclusion.
- No streaming (`xmlTextReader`) XInclude, `xmlXIncludeProcessTree` on a
  subtree, error handler callbacks, or `_private` data. `xmlns()` bindings do
  not leak between inclusions, as libxml2's shared XPath context lets them.
- Resolver failures are treated as missing resources; libxml2 counts I/O
  errors other than "not found" and network refusal for `parse="text"`.
