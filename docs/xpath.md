# XPath 1.0

`yaxml::xpath` implements the [XPath 1.0 recommendation](https://www.w3.org/TR/1999/REC-xpath-19991116/)
over the borrowed `Document` and the `OwnedDocument`. Where the recommendation
leaves behavior open, the pinned libxml2 2.15.4 `xpath.c` decides.

```rust
use yaxml::OwnedDocument;
use yaxml::xpath::{Context, Value, XPath};

let document = OwnedDocument::parse("<r xmlns:p='urn:p'><p:i n='2'/><p:i n='3'/></r>")?;
let context = Context::new()
    .with_namespace("q", "urn:p")
    .with_variable("min", Value::Number(2.0));
let xpath = XPath::compile("//q:i[@n > $min]")?;
let nodes = xpath.select_nodes(&document, document.root().id(), &context)?;
assert_eq!(nodes.len(), 1);
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Contract

- `XPath::compile` parses once into a flat AST; a compiled expression is
  `Clone + Send + Sync` and reusable across documents. Core-function names and
  arity are checked at compile time. Prefixes, variables and extension
  functions resolve at evaluation, against the `Context`.
- `evaluate(&tree, node, &context)` takes either DOM (the sealed `Tree` trait),
  a context node (`Into<xpath::Node<Id>>`) and returns one of the four value
  types. The context position and size are 1. Node-sets are sorted in document
  order without duplicates. Namespace nodes follow their element and precede
  its attributes.
- `xpath::Node` is `Tree(id)`, `Attribute(owner, index)` or `Namespace(owner,
  index)`. The attribute index addresses the DOM's attribute list, which also
  holds namespace declarations; those are never attribute nodes. Namespace
  index 0 is the implicit `xml` binding, followed by inherited bindings
  outermost first, as libxml2 orders them.
- `Context` holds owned prefix bindings, variables and extension functions,
  keyed by (namespace URI, local name). The `xml` prefix is predefined. Core
  functions take precedence over unprefixed extensions. Caller-supplied
  node-sets are validated against the tree (`InvalidContext`) and re-sorted.
- Errors use libxml2's `xmlXPathError` categories. Compilation errors carry
  the expression byte offset; evaluation errors carry the offset of the
  unknown prefix, variable or function name that caused them.
- CDATA sections are text nodes. `id()` matches `xml:id` attributes, the only
  ID attributes without a DTD.
- Kept entity references (trees parsed without `NOENT`) are invisible, as
  in libxml2: no node test selects them, no axis enters them, and their
  text still counts in string values.
- Number conversion and formatting follow `xmlXPathStringEvalNumber` and
  `xmlXPathFormatNumber`: an optional exponent is accepted, `string()` prints
  integers in `int` range exactly, other values with at most 15 significant
  digits, and magnitudes outside [1e-5, 1e9] in `%e` form (`1e+10`).
  `NaN`, `Infinity` and `-Infinity` are spelled as libxml2 does.
- `substring()` and `round()` use libxml2's rounding: halves round up and
  values in (-0.5, 0) round to negative zero.

## Limits

`Limits` bounds expression bytes (64 KiB), nesting depth (200; this bounds
both parser and evaluator recursion), evaluation operations (50 million; one
per expression node, visited axis node and text chunk, plus one per 64 text
bytes) and node-set length (10 million). Checks run before the work they
bound. Compile limits are passed to `XPath::compile_with_limits`, evaluation
limits through `Context::with_limits`. Unions and steps sort once with a
stable sort, so concatenated sorted runs merge in linear time.

## Known differences from libxml2

These are checked to still differ by `harness/tests/xpath_oracle.rs`:

- `xmllint --xpath` evaluates with an unset context size and position, so a
  top-level `last()` or `position()` fails there; yaxml returns 1.

Not compared, and deliberately different:

- libxml2's `xmlXPathCmpNodes` orders namespace nodes after every other node.
  yaxml (evaluation and `xpath::compare`, its port) orders them by their
  element, after it and before its attributes, so unions mixing namespace
  nodes with other nodes may sort differently.
- `XPath::compile_in` applies `Context::with_flags` (`CHECKNS`, `NOVAR`) as
  `xmlXPathCtxtCompile`, reporting where the parser meets the name test or
  variable, as libxml2 does; lexical errors (unfinished literal, invalid
  character, `$` without a name) come first, since the expression is
  tokenized before it is parsed. `tests/xpath.rs` pins 27 verdicts recorded
  from libxml2.
- `namespace::p:x` and `namespace::p:*` select nothing, since namespace nodes
  have a null namespace URI; libxml2 ignores the test's prefix.

## Gaps

XPointer is [`yaxml::xpointer`](xinclude.md). No `xmlXPathOrderDocElems`. Variable and
function lookup callbacks are static maps. Extension functions receive their
evaluated arguments, not the context node or tree. Entity-reference nodes do
not exist in the current DOMs.

## Differential check

`cargo test --test xpath_oracle -- --ignored --nocapture` compares 235
expressions with `xmllint --xpath` on one fixture covering every axis, node
test, operator, core function and error category. Node-sets are compared
through `count()` and each member's `name()` and `string()` in document order.
Numbers are compared through `string()`, which exercises libxml2's formatting.
Set `YAXML_XMLLINT` to choose the binary.
