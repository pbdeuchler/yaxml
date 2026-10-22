# Patterns

`yaxml::pattern` ports libxml2 2.15.4 `pattern.c`: the XPath subset behind
`xmlTextReaderPreservePattern`, XSD identity-constraint selectors and fields,
and XSLT-like `match` patterns. It matches a DOM node or evaluates
incrementally over element start and end events.

```rust
use yaxml::{Document, pattern::Pattern};

let pattern = Pattern::compile("p:a/b|//c", &[("p", "urn:p")], 0)?;
let document = Document::parse("<r xmlns:p='urn:p'><p:a><b/></p:a><c/></r>")?;
let hits = document.nodes().filter(|n| pattern.matches(&document, n.id())).count();
assert_eq!(hits, 2);

let mut stream = pattern.stream().expect("streamable");
assert_eq!(stream.push("r", None), Ok(false));
assert_eq!(stream.push("a", Some("urn:p")), Ok(false));
assert_eq!(stream.push("b", None), Ok(true));
stream.pop();
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Syntax

Steps joined by `/` and `//`: `name`, `p:name`, `*`, `p:*`, `.`, `@name`,
`@p:name`, `@*`, `@p:*`, `child::` and `attribute::`, with a leading `/`, `//`
or `.//`, and alternatives joined by `|`. Unprefixed names have no namespace
(the default namespace cannot be bound); prefixes come from the bindings given
to `Pattern::compile`, `xml` is predefined, an unbound prefix is an error.
Flags: `0` (pattern: unanchored paths match at any depth), `XPATH`,
`XS_SELECTOR` (no attribute axis) and `XS_FIELD`; the last three anchor
relative paths at the first pushed node as `xmlStreamPush` does. No
predicates, other axes or functions.

## Matching

`Pattern::matches(&tree, node)` takes an element id or an `xpath::Node`
(`Attribute(owner, index)` for attributes) from either DOM. It walks the node's
ancestor chain with a set of live positions per step, so work is bounded by
steps times depth (no backtracking explosion).

`Pattern::stream()` returns a `StreamMatcher` (or `None` when an alternative
cannot be streamed, as `xmlPatternGetStreamCtxt`). Feed `push(local, ns)` for
each start tag, `push_attr` for attributes, `push_other` for text, comment and
PI nodes (only if `wants_any_node()`), `pop` for each end; `push*` return
whether the node matches, `reset` returns to the document node. State is a
list of `(step, level)` pairs, deduplicated, pruned on `pop`, so memory is at
most steps times depth and never depends on document size.

The stream shape is queryable without building a matcher: `streamable()`
(`xmlPatternStreamable`), `min_depth()`, `max_depth()` (`Some(None)` = unbounded,
libxml2's -2) and `from_root()`. Each is `None` where libxml2 returns -1 (not
streamable). `tests/pattern.rs` `depth_queries_match_libxml2` pins values
probed from libxml2 2.15.4.

## Limits

`Limits { max_bytes: 64 KiB, max_steps: 256 per alternative, max_depth: 1024 }`
bound compilation and stream nesting; exceeding them is
`ErrorKind::LimitExceeded`. Text after a NUL byte is ignored, as in C.

## Verification

`tests/pattern.rs` reproduces libxml2's `runtest.c` `patternTest` output for
all four `test/pattern` inputs (conj, multiple, namespaces, simple: 4 of 4,
byte-identical, including the stream-versus-DOM agreement check), plus
table-driven compile, match and stream cases on both DOMs. The `pattern` fuzz
target (`tests/support/fuzz/pattern.rs`) asserts, for every element and attribute, that the
streamed result equals the DOM match in pattern mode, and that both DOMs agree.

## Differences from libxml2

- `matches` is the XPath meaning where libxml2's DOM matcher disagrees with its
  own stream: `.` steps are dropped (`a/.` is `a`, not "a child of `a`"),
  `*//b` and `p:*//b` search ancestors (libxml2 never matches), `x//.` is
  ancestor-or-self, and an attribute test needs an equal namespace (libxml2
  lets `@a` match `p:a`). A pattern libxml2 answers `-1` for is a plain match
  or mismatch here.
- A step after an attribute test (`@a b`) is a syntax error; libxml2 accepts it.
- `/.//.` also selects the document's descendants in a stream; libxml2 selects
  only the document.
- `child::p:n` keeps libxml2's reading as the single name `p:n` (no namespace).
- Not ported: dictionaries,
  `xmlStreamPushNode` node-type values beyond `push_other`, and the
  `xmlTextReaderPreservePattern` reader integration (see [textreader.md](textreader.md)).
