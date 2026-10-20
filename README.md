# yaxml

A safe Rust XML processor pursuing libxml2 behavior, with `memchr` as its only
dependency. This is an early implementation, not a libxml2 drop-in replacement.

The current library parses an explicitly bounded subset: XML 1.0 Fifth Edition
in UTF-8, UTF-16 and libxml2's built-in single-byte encodings, namespaces, attributes, predefined/numeric references, comments,
processing instructions, CDATA, a borrowed DOM, and an owned DOM with bounded
value edits, insertion, moves, attribute and namespace edits, copies within
and across documents and permanent subtree removal. Internal DTD subsets
are processed (entities, attribute normalization and, on request, default
attributes) and the declaration is reported as `Event::Doctype`; entity
references are kept or substituted as libxml2 options select. `Document::dtd`/`OwnedDocument::dtd` expose the parsed declarations as
borrowed views (`valid::Entity`, `valid::Element`, `valid::Attribute`,
`valid::Notation`), and `yaxml::chars` the XML character classes. External subsets and entities
load only through a caller-supplied `Resolver` (see below). Other encodings
return structured errors.
`Node::text_chunks` and `OwnedNode::text_chunks` visit string values without
allocating: containers yield descendant Text/CDATA, and leaves yield their
own payload. Callers can collect the chunks or stream them to a sink.
`Reader` and `Document` borrow complete UTF-8 input.
`OwnedDocument`, `PushParser`, and `BufferedReader` detect UTF-8 and UTF-16LE/BE,
and decode a declared US-ASCII, ISO-8859-1..16 or windows-1252 document.
`PushParser` consumes arbitrary byte chunks; `read_from` drives it from a
caller-provided `std::io::Read`. `BufferedReader` lends one event at a time from
caller-provided `BufRead` input without consuming the unread suffix. Both retain
a bounded token and active document/namespace state. `Writer` validates UTF-8
events and serializes them to a caller-provided `std::io::Write`; fresh writers
can also serialize complete borrowed or owned DOMs. `save::to_writer` (or
`to_vec`) writes a document or any node of either DOM as libxml2's
`xmlSaveDoc`/`xmlSaveTree` do, streaming to any `Write`: `xmlSaveOption` flags
(format, XHTML, ...), subtree levels, UTF-16 and single-byte output encodings,
and the internal subset regenerated from its declarations
([writer.md](docs/writer.md#libxml2-tree-output-save)). `TextWriter` is
libxml2's `xmlTextWriter`, byte for byte
([writer.md](docs/writer.md#libxml2-text-writer-textwriter)).

```rust
use std::ops::ControlFlow;
use yaxml::{Document, Event, PushParser};

fn main() -> Result<(), yaxml::Error> {
    let xml = "<root xmlns='urn:example'><item key='v'>text&amp;more</item></root>";
    let document = Document::parse(xml)?;
    for node in document.root_element().descendants() {
        if let Some(name) = node.name() {
            println!("{} {:?}", name.local(), name.namespace());
        }
    }

    let mut parser = PushParser::new();
    for chunk in xml.as_bytes().chunks(7) {
        parser.feed(chunk, |event, _origin| {
            if let Event::Text(value) = event {
                assert_eq!(value, "text&more");
            }
            ControlFlow::Continue(())
        })?;
    }
    parser.finish(|_, _| ControlFlow::Continue(()))?;
    Ok(())
}
```

Names use `Name::new(qualified, namespace)` for validated construction and
`qualified()`, `prefix()`, `local()` and `namespace()` for access. Borrowed
QName inputs are not copied; owned strings move into the name. Namespace
metadata is preserved, including `Some("")`; Writer checks it against the
actual namespace scope. For example:

```rust
use std::borrow::Cow;
use yaxml::Name;

let name = Name::new(String::from("p:item"), Some(Cow::Borrowed("urn:example")))?;
assert_eq!(name.prefix(), Some("p"));
assert_eq!(name.local(), "item");
# Ok::<(), yaxml::Error>(())
```

This replaces the earlier public name fields. Getter references borrow the
name view, which may own its QName: keep that view alive while using them.
Processing-instruction event and borrowing-DOM targets now use `Cow<str>` too;
construct a borrowed target with `Cow::Borrowed("pi")` or move an owned string.
`Element::attributes` is an `Attributes` list, owned (`Vec::into()`) or
borrowed; it dereferences to a slice. `PushParser` callbacks receive
`&Event`: the event and its values are lent from storage the parser reuses,
so copy what you keep beyond the callback.

`Limits` controls input bytes, raw token bytes, element depth, attributes per
element, emitted events, and cumulative requested string capacity for normalized
values, namespace copies, and retained streaming names, plus DOCTYPE bytes and
entity expansion bytes, references and nesting depth. Defaults are finite and
stop billion-laughs and quadratic-blowup documents. The string budget counts work
across an entire operation, including released buffers; it is not a precise
process memory quota. `DecodeLimits` separately bounds decoded UTF-8 bytes and
decoded token bytes, including on the UTF-8 owning/streaming paths. Scratch token
storage is bounded by both token ceilings. Streaming copies spend more of the string budget than borrowing slices.
Configuration structs are `#[non_exhaustive]` with public fields: start from `Limits::DEFAULT` (or `Default::default()`) and chain the field-named setters, e.g. `Limits::DEFAULT.max_depth(64).max_attributes(10)`.
Parsing opens no files or network connections; `read_from` reads only its supplied
source and preserves underlying I/O errors in `ReadError`.

`BufferedReader::read_event` returns the typed event and a `SourceOrigin`.
The event can borrow the retained token, so it must be dropped before advancing
again. Successful EOF repeats without I/O. XML failures are sticky; an I/O error
is returned once and later calls return `InvalidState` without touching the
source. See the [streaming contract](docs/streaming.md) for exact consumption,
lookahead, and limit behavior.

`SourceOrigin::input_range()` returns an optional exact original-byte span,
including lexical delimiters. Normalization changes values without changing
these spans. Push callbacks receive origins too. `Reader::event_origin()` and
`PushParser::event_origin()` are absent before the first event and retain the
last delivered origin after EOF or failure. Their `event_range()` convenience
method now returns `Option<Range<usize>>`; `Node::range()` does too.
`OwnedNode::origin()` is absent for constructed nodes. `position()` still counts
physical input consumption, which can include lookahead beyond the event span.
Values produced by entity expansion have no input range; `physical_context()`
names the outermost entity reference and `replacement_range()` the span inside
its replacement text.

An empty feed means temporary exhaustion; `finish` means final EOF. A callback's
`Break(())` cancels permanently with `Aborted`. Errors are sticky, successful finish
is idempotent, and feeding after finish returns `InvalidState`. Text event spans
are independent of chunk boundaries. Callback values borrow only for the call.
Incremental validation can deliver valid prefix events before discovering a later
error; slice construction validates all input characters before emitting events.

Declarations with a `1.x` version use XML 1.0 processing rules, as required by
that specification; this does not enable XML 1.1 semantics. Encoding names are
syntax-checked before an unsupported encoding is reported. The
[encoding contract](docs/encoding.md) defines BOM detection, declaration agreement,
surrogate rejection, original byte offsets, and intentional libxml2 differences. The
[conformance regressions](docs/conformance.md) and
[independent oracle protocol](docs/oracle.md) describe the validation scope.

Copy reader events to a writer with explicit namespace declarations:

```rust
use yaxml::{Reader, Writer};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let input = br#"<p:root xmlns:p="urn:example" key="a&amp;b"/>"#;
    let mut reader = Reader::new(input)?;
    let mut writer = Writer::new(Vec::new());
    while let Some(event) = reader.read_event()? {
        writer.write_event(&event)?;
    }
    writer.finish()?;
    assert_eq!(writer.into_inner().as_slice(), input);
    Ok(())
}
```

The [writer contract](docs/writer.md) defines its six resource ceilings,
escaping, namespace checks, and partial-output behavior. Each event is validated
before writing. A sink failure can leave a partial event; later calls return
`InvalidState`. `finish` requires a closed document and flushes the sink.
`write_document` and `write_owned_document` traverse complete trees iteratively,
rewrite a present declaration encoding to `UTF-8`, flush on success, and leave
the writer finished. Namespace repair, subtree output, formatting, and UTF-16
output remain pending.

Use `OwnedDocument` when a tree must outlive its source:

```rust
use yaxml::{EditLimits, OwnedDocument};

let mut document = {
    let input = String::from("<root a='value'>text</root>");
    OwnedDocument::parse(&input)?
};
assert_eq!(document.root_element().name().unwrap().local(), "root");
assert_eq!(document.root_element().first_child().unwrap().text(), Some("text"));
let text = document.root_element().first_child().unwrap().id();
document.edit(EditLimits::default())?.set_node_value(text, "updated")?;
assert_eq!(document.node(text).unwrap().text(), Some("updated"));
let root = document.root_element().id();
let mut editor = document.edit(EditLimits::default())?;
let note = editor.append_comment(root, "reviewed")?;
assert_eq!(editor.document().node(note).unwrap().range(), None);
editor.remove_subtree(text)?;
assert!(editor.document().node(text).is_none());
# Ok::<(), Box<dyn std::error::Error>>(())
```

The [owned DOM contract](docs/owned-dom.md) describes pool-backed views,
source ranges and aggregate string accounting. Retained copies share the
normalization budget, so owned parsing can reach its ceiling before borrowing
parsing. `Document` continues to borrow unchanged input.
`OwnedDocument::create` builds a complete rooted document from borrowed name,
attribute and namespace specifications. `Editor::append_element` adds fresh
elements with initial attributes, preserving expanded names and checked handles.
`Editor` also replaces existing leaf or ordinary-attribute values, appends fresh
Text/CDATA/comment/PI leaves, and permanently removes non-root subtrees.
Text and CDATA require an element parent; comments and PIs may also append after
the document's epilog. These appends never merge Text nodes. Removal makes all
subtree handles stale and protects the synthetic root and document element.
Parsed survivors retain their source ranges; new nodes have none. `EditLimits`
bounds each supplied string, cumulative pool-capacity requests, and successful
created slots/attributes across editors. Construction also bounds aggregate
supplied bytes, element depth/items and namespace lookup work. Removal refunds
no cumulative counters. The editor also moves nodes with libxml2's semantics
(`append_child` merges Text like `xmlAddChild`; `insert_before`,
`insert_after`, `unlink` with detached subtrees, `replace`), adds and removes
attributes and namespace declarations, renames elements, and `compact`s edit
garbage without renumbering handles. Every edit keeps namespace prefixes
declared where they are used. Both DOMs offer libxml2's accessors:
`attribute`, `content`, `list_string`, `path`, `base_uri`, `lang`,
`space_preserve`, `search_ns`, `namespaces` and `previous_sibling`.
`Editor::import` copies a node from another document (`xmlDocCopyNode`),
`Editor::copy` within one and `OwnedDocument::try_clone` a whole document
(`xmlCopyDoc`); copies arrive detached, with the namespaces they need declared
on their root. Moving between documents is `import` plus `remove_subtree`,
since a handle names one document.

The `allocator_api` cargo feature (Rust 1.100) charges owned-document node
and attribute storage to a caller's allocator:
`OwnedDocument::parse_in(input, options, allocator)` and `create_in` build an
`OwnedDocument<A>` for any `A: Allocator + Clone`, for example an arena
`&A`, `System`, or `Arc<dyn Allocator + Send + Sync>` for language bindings.
Navigation, editing, XPath, C14N, validation and `Writer` work on any `A`.
The string pool, `save`, HTML output and XInclude stay on the global heap.
Without the feature the crate builds on Rust 1.97 and existing code is
unchanged; see [allocator strategy](docs/design.md#allocator-strategy).

`yaxml::xpath` evaluates XPath 1.0 over either DOM: all axes, node tests,
predicates, unions, the four value types, the 27 core functions, and
caller-supplied prefixes, variables and extension functions. Number
conversion and formatting, and `substring`/`round` edge cases, follow libxml2.
Compilation and evaluation are bounded by finite `xpath::Limits`. The
[XPath contract](docs/xpath.md) lists the remaining differences from libxml2.

```rust
let document = yaxml::Document::parse("<r><i k='1'/><i k='2'/></r>")?;
let sum = yaxml::xpath::XPath::compile("sum(//i/@k)")?;
let value = sum.evaluate(&document, document.root().id(), &yaxml::xpath::Context::new())?;
assert_eq!(value, yaxml::xpath::Value::Number(3.0));
# Ok::<(), Box<dyn std::error::Error>>(())
```

Nothing outside the input is read unless the caller passes a `Resolver`
(any `FnMut(Request) -> io::Result<Option<Vec<u8>>> + Send` closure, or the
root-confined `FileResolver`) in `ParseOptions`; the flags decide what loads
(`DTDLOAD` the external subset, `NOENT` or `DTDVALID` external entities,
never with `NO_XXE`): `ParseOptions::new(DTDLOAD | NOENT).with_resolver(base,
resolver)` loads both, like libxml2's `XML_PARSE_DTDLOAD | XML_PARSE_NOENT`
with a custom resource loader. A resolver without loading flags is never
consulted. Budgets are `Limits::max_external_loads` and `max_external_bytes`. DTD `ID` attributes
and `xml:id` back `element_by_id` and XPath `id()`.

```rust
let dtd = "<!ATTLIST r id ID #IMPLIED><!ENTITY e 'loaded'>";
let resolver = move |request: yaxml::Request<'_>| {
    Ok((request.uri == "dtd/r.dtd").then(|| dtd.as_bytes().to_vec()))
};
let input = b"<!DOCTYPE r SYSTEM 'r.dtd'><r id='a'>&e;</r>";
use yaxml::options::{DTDLOAD, NOENT};
let options = yaxml::ParseOptions::new(DTDLOAD | NOENT).with_resolver("dtd/doc.xml", Box::new(resolver));
let document = yaxml::Document::parse_with(input, options)?;
assert_eq!(document.element_by_id("a").unwrap().text_chunks().collect::<String>(), "loaded");
# Ok::<(), Box<dyn std::error::Error>>(())
```

`Document::validate` and `OwnedDocument::validate` check a tree against its
DTD like libxml2's `XML_PARSE_DTDVALID`; `yaxml::valid::validate` checks it
against a separate `Dtd` (`xmllint --dtdvalid`). Validity diagnostics are
never fatal and carry libxml2's `XML_DTD_*` numbers; content models compile
to bounded deterministic automata. See [DTD validation](docs/archive/validation-dtd.md).

```rust
let xml = "<!DOCTYPE r [<!ELEMENT r (a)><!ATTLIST r id ID #REQUIRED>]><r/>";
let diagnostics = yaxml::Document::parse(xml)?.validate();
let codes: Vec<_> = diagnostics.iter().map(|diagnostic| diagnostic.code).collect();
use yaxml::valid::Code::{ContentModel, MissingAttribute};
assert_eq!(codes, [ContentModel, MissingAttribute]);
assert!(!yaxml::valid::is_valid(&diagnostics));
# Ok::<(), yaxml::Error>(())
```

`TextReader` is libxml2's `xmlTextReader` cursor (`xmlreader.h`) over any
`BufRead`: node types and depths numbered as libxml2, attribute navigation,
namespace/`xml:lang`/`xml:base` scope, `next`, `read_string`, inner/outer XML
and `expand` to an `OwnedDocument`, in bounded memory. See the
[TextReader contract](docs/textreader.md) for libxml2 agreement and gaps.

`yaxml::pattern` compiles libxml2's XPath pattern subset (`a/b`, `//a`, `.//a`,
`a|b`, `@x`, `p:*`) and matches a node of either DOM or evaluates it
incrementally over element events in memory bounded by pattern and depth; see
[patterns](docs/pattern.md).

```rust
use yaxml::{NodeType, TextReader};

let mut reader = TextReader::new(b"<r xmlns:p='urn:p'><p:i k='v'>x</p:i></r>".as_slice());
while reader.read()? {
    if reader.node_type() == NodeType::Element && reader.local_name() == Some("i") {
        assert_eq!((reader.depth(), reader.get_attribute("k")), (1, Some("v")));
        assert_eq!(reader.read_outer_xml()?.unwrap(), r#"<p:i xmlns:p="urn:p" k="v">x</p:i>"#);
    }
}
# Ok::<(), yaxml::ReadError>(())
```

`yaxml::c14n` writes Canonical XML 1.0 and 1.1 and Exclusive XML
Canonicalization 1.0 for either DOM, whole or as an XPath node-set, with
libxml2's bytes; see the [C14N contract](docs/c14n.md).

```rust
let document = yaxml::Document::parse("<r xmlns:p='urn:p'><p:e b='2' a='1'/></r>")?;
let options = yaxml::c14n::Options::new(yaxml::c14n::Mode::Exclusive10);
let canonical = yaxml::c14n::to_string(&document, &options)?;
assert_eq!(canonical, r#"<r><p:e xmlns:p="urn:p" a="1" b="2"></p:e></r>"#);
# Ok::<(), Box<dyn std::error::Error>>(())
```

`yaxml::xinclude::process` performs XInclude 1.0 as libxml2's `xinclude.c`
does (XML and text inclusion, fallbacks, `xml:base` fixup, XPointer
selection, loop/depth detection) and returns a new `OwnedDocument`; resources
load only through the supplied `Resolver`, within cumulative budgets.
`yaxml::xpointer::evaluate` is libxml2 2.15's XPointer subset (shorthand,
`element()`, `xmlns()`, `xpointer()`). See the [XInclude contract](docs/xinclude.md).

```rust
let main = yaxml::OwnedDocument::parse(
    "<r xmlns:xi='http://www.w3.org/2001/XInclude'><xi:include href='a.xml' xpointer='element(/1/2)'/></r>",
)?;
let resolver = |_: yaxml::Request<'_>| Ok(Some(b"<a><b/><c/></a>".to_vec()));
let result = yaxml::xinclude::process(&main, "main.xml", Box::new(resolver), &Default::default())?;
assert_eq!(result.root_element().first_child().unwrap().name().unwrap().local(), "c");
# Ok::<(), Box<dyn std::error::Error>>(())
```

`yaxml::schema_types` validates, compares and constrains the 45 XML Schema
built-in datatypes as libxml2's `xmlschemastypes.c` does, and `yaxml::regexp`
compiles XML Schema patterns to budgeted automata matched in linear time. Both
are the foundation for XML Schema and RELAX NG validation; see
[schema types](docs/schema-types.md).

```rust
use yaxml::{regexp::Regex, schema_types::{Comparison, Type}};

let pattern = Regex::new("[A-Z]{2}\\d{3}")?;
assert!(pattern.is_match("AB123") && !pattern.is_match("AB12"));
let late = Type::DateTime.parse("2001-10-26T21:32:52Z", None).unwrap();
let early = Type::DateTime.parse("2001-10-26T23:32:52+04:00", None).unwrap();
assert_eq!(early.compare(&late), Comparison::Less);
# Ok::<(), yaxml::regexp::Error>(())
```

`yaxml::xsd` compiles XML Schema 1.0 documents as libxml2's `xmlschemas.c`
does (includes, imports and redefines through a caller `Resolver`, derivation
and Unique Particle Attribution checks, counted content models, identity
constraints) into a reusable, thread-safe `Schema` that validates either DOM,
or parser events without a tree (`validate_stream`, `StreamValidator`), with
libxml2's error codes; verdicts agree on all 233 libxml2 `test/schemas`
instances. See [XML Schema](docs/xsd.md).

```rust
use yaxml::xsd::{self, Code, Schema};

let schema = Schema::parse(br#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
  <xs:element name="r"><xs:complexType><xs:sequence>
    <xs:element name="n" type="xs:int" maxOccurs="3"/>
  </xs:sequence></xs:complexType></xs:element></xs:schema>"#, "r.xsd")?;
let diagnostics = schema.validate(&yaxml::Document::parse("<r><n>1</n><n>x</n></r>")?);
assert!(!xsd::is_valid(&diagnostics));
assert_eq!(diagnostics[0].code, Code::CvcDatatypeValid1_2_1);
# Ok::<(), Box<dyn std::error::Error>>(())
```

`yaxml::relaxng` compiles RELAX NG schemas (XML syntax, `include` and
`externalRef` through a `Resolver`, full section 4 simplification and section
7 restrictions, built-in and XML Schema datatypes) and validates either DOM,
or the events of any streaming reader (`Schema::stream`, also behind
`TextReader::relaxng_set_schema`), with derivatives over interned patterns,
bounded by a work budget, where libxml2's `relaxng.c` backtracks. All 159
libxml2 `test/relaxng` verdicts agree; see [RELAX NG](docs/relaxng.md).

```rust
use yaxml::relaxng::{Limits, Schema};

let rng = br#"<element name="r" xmlns="http://relaxng.org/ns/structure/1.0"
    datatypeLibrary="http://www.w3.org/2001/XMLSchema-datatypes">
  <oneOrMore><element name="i"><data type="int"/></element></oneOrMore>
</element>"#;
let none = |_: yaxml::Request<'_>| Ok(None);
let schema = Schema::parse(rng, "r.rng", Box::new(none), &Limits::default())?;
assert!(schema.validate(&yaxml::Document::parse("<r><i>1</i><i>2</i></r>")?).is_empty());
let [error] = schema.validate(&yaxml::Document::parse("<r><i>x</i></r>")?).try_into().unwrap();
assert_eq!(error.code, yaxml::relaxng::Code::Datatype);
# Ok::<(), Box<dyn std::error::Error>>(())
```

`yaxml::schematron` ports libxml2's Schematron (ISO and 1.5 namespaces,
`pattern`/`rule`/`let`/`assert`/`report`, `name` and `value-of` messages,
pattern rule contexts, XPath tests): `Schema` validates either DOM with the
`XML_SCHEMATRON_OUT_*` modes and prints libxml2's report text, identical on
all 42 libxml2 `test/schematron` results; see [Schematron](docs/schematron.md).

```rust
use yaxml::schematron::{Limits, OUT_TEXT, Schema};

let sct = br#"<schema xmlns="http://purl.oclc.org/dsdl/schematron"><pattern id="p">
  <rule context="i"><assert test="@k">no key</assert></rule></pattern></schema>"#;
let none = |_: yaxml::Request<'_>| Ok(None);
let schema = Schema::parse(sct, "s.sct", Box::new(none), &Limits::default())?;
let input = b"<r><i k='1'/><i/></r>";
let validation = schema.validate(&yaxml::Document::parse_bytes(input)?, OUT_TEXT, input);
assert_eq!(validation.render("r.xml"), "Pattern: p\n/r/i[2] line 1: no key\n");
# Ok::<(), Box<dyn std::error::Error>>(())
```

`yaxml::html` parses HTML the way libxml2 2.15.4 does (HTML5-style
tokenization, libxml2's legacy tree construction with implied elements and
auto-close tables, recovery with diagnostics, meta/BOM/windows-1252 encoding
selection) into an `OwnedDocument` marked as HTML, incrementally through
`html::PushParser` with chunk-independent results, and serializes it like
`htmlDocDumpMemory` (`html::to_html`) or `xmllint --html` (`html::save`):

```rust
use yaxml::html;

let parsed = html::parse(b"<title>T</title><p>a<p>b &amp; c", html::Options::default())?;
let output = html::save(&parsed.document, false)?;
assert!(output.ends_with(b"<body><p>a</p><p>b &amp; c</p></body></html>\n"));
# Ok::<(), yaxml::Error>(())
```

The [HTML contract](docs/html.md) lists options, encodings, limits, the
xmllint differential and known differences.

`yaxml::catalog` resolves public, system and URI identifiers through OASIS XML
or SGML catalogs as libxml2's `catalog.c` does (rewrite, delegate,
`nextCatalog`, `prefer`, `urn:publicid:`), with no global or default catalog.
A `Catalog` is parsed from bytes or loaded through a `Resolver`, which also
fetches referenced catalogs lazily within `catalog::Limits`;
`CatalogResolver::new(catalog, inner)` is itself a `Resolver`, so any parser
gains catalog support unchanged. See the [catalog contract](docs/catalog.md).

```rust
use yaxml::catalog::{Catalog, CatalogResolver};

let entries = br#"<catalog xmlns="urn:oasis:names:tc:entity:xmlns:xml:catalog">
  <rewriteSystem systemIdStartString="http://example.com/" rewritePrefix="mirror/"/>
</catalog>"#;
let none = |_: yaxml::Request<'_>| Ok(None);
let catalog = Catalog::parse(entries, Default::default(), "", Box::new(none))?;
let inner = |request: yaxml::Request<'_>| {
    Ok((request.uri == "mirror/r.dtd").then(|| b"<!ENTITY e 'local'>".to_vec()))
};
let resolver = CatalogResolver::new(catalog, Box::new(inner));
let input = b"<!DOCTYPE r SYSTEM 'http://example.com/r.dtd'><r>&e;</r>";
let options = yaxml::ParseOptions::new(yaxml::options::DTDLOAD | yaxml::options::NOENT);
let document = yaxml::Document::parse_with(input, options.with_resolver("d.xml", Box::new(resolver)))?;
assert_eq!(document.root_element().text_chunks().collect::<String>(), "local");
# Ok::<(), Box<dyn std::error::Error>>(())
```

`yaxmllint` (`cargo run --bin yaxmllint -- FILE`) is `xmllint`'s command line over
these modules: `--noout --format --encode --xpath --c14n --html --valid
--schematron --relaxng --schema --xinclude --stream --catalogs` and the rest of the table
in [yaxmllint](docs/yaxmllint.md), with xmllint's exit codes and stdout.

The core uses `#![forbid(unsafe_code)]`; standard-library and `memchr` internals
are separate boundaries. Native C ABI compatibility and language adapters need
their own scope decision because libxml2 exposes raw pointers and public mutable
structs. No adapter may weaken the core's restrictions.

The [compatibility catalogue](docs/compatibility.md), [design](docs/design.md),
[roadmap](docs/roadmap.md), and [security plan](docs/security.md) describe the full
target and known gaps. Allocator-aware owned storage is the opt-in `allocator_api`
feature; the default build uses ordinary safe vectors. No performance advantage
over libxml2 or complete fuzz execution coverage is claimed yet.
Coverage-guided cargo-fuzz targets and a coverage report script are described in
[fuzz/README.md](fuzz/README.md).

Run `cargo test`, `cargo clippy --all-targets -- -D warnings`,
`cargo bench --bench parse`, `cargo bench --bench pull`,
`cargo bench --bench serialize`, `cargo bench --bench mutation`, and
`cargo bench --bench structural_mutation`; with Rust 1.100, also run clippy
and the tests with `--features allocator_api`. These runners report yaxml-only DOM,
slice-event, push, buffered-pull, complete-tree serialization, value-edit and
structural-edit measurements. The [structural validation report](docs/archive/validation-owned-structure.md)
records the pinned libxml2 conformance results, independent review and measured
fuzz coverage. A separate [matched structural benchmark](docs/archive/validation-owned-structure-matched.md)
compares edit lifecycles with libxml2, including slower workloads and the
differences in Text topology, validation and buffering.

Every parser and DOM takes one [`ParseOptions`](docs/compatibility.md#option-compatibility):
libxml2's `xmlParserOption` bits (`yaxml::options::NOENT`, `DTDATTR`, ...,
with libxml2's numeric values, so bindings can pass an existing `int`), the
`Limits`, and an optional base and `Resolver`. `Reader::with_options`,
`PushParser::with_options`, `BufferedReader::with_options`,
`TextReader::with_options`, `Document::parse_with` and
`OwnedDocument::parse_with` accept it; the other constructors are
conveniences with no flags. A flag a surface does not implement is
`ErrorKind::UnsupportedOption`, never silently ignored.

The default is libxml2's: entity references stay in the tree and no DTD
defaults are added. A reference in content becomes a `NodeKind::EntityRef`
(`OwnedNodeKind::EntityRef`) node holding the replacement's nodes; streams
bracket the replacement's events with `Event::EntityStart`/`EntityEnd`.
String values see through references, XPath never selects them or their
content, `save` writes `&name;` and the `Writer` writes the replacement.
`NOENT` substitutes them, `DTDATTR` adds default attributes; see the
[DOCTYPE section](docs/compatibility.md#document-type-declarations) for exact
semantics and known differences:

```rust
use yaxml::{Document, ParseOptions, options};

let xml = "<!DOCTYPE r [<!ENTITY e '<b>x</b>'><!ATTLIST r a CDATA 'd'>]><r>&e;</r>";
let kept = Document::parse(xml)?;
let reference = kept.root_element().first_child().unwrap();
assert_eq!(reference.entity_name(), Some("e"));
assert!(kept.root_element().attributes().is_empty());
assert_eq!(kept.root_element().text_chunks().collect::<String>(), "x");
let complete = Document::parse_with(xml.as_bytes(), ParseOptions::new(options::NOENT | options::DTDATTR))?;
assert_eq!(complete.root_element().attributes()[0].value, "d");
assert_eq!(complete.root_element().first_child().unwrap().name().unwrap().local(), "b");
# Ok::<(), yaxml::Error>(())
```

`RECOVER` (XML_PARSE_RECOVER) builds what is parsable of a malformed
document, as libxml2 does: an end tag closes the open element, the end of
input closes the rest, an unbound prefix leaves a name in no namespace, and
so on ([the repairs](docs/compatibility.md#recovery-recover)). Both DOMs
accept it and list the errors recovered from; resource limits stay fatal, and
a document without any element is still an error:

```rust
use yaxml::{Document, ErrorKind, ParseOptions, options, save};

let document = Document::parse_with(b"<r><a>text</b><p:c/>", ParseOptions::new(options::RECOVER))?;
let kinds: Vec<_> = document.errors().iter().map(|error| error.kind).collect();
assert_eq!(kinds, [ErrorKind::MismatchedEndTag, ErrorKind::Namespace, ErrorKind::UnexpectedEof]);
let saved = save::to_vec(&document, &Default::default()).unwrap();
assert_eq!(saved, b"<?xml version=\"1.0\"?>\n<r><a>text</a><p:c/></r>\n");
# Ok::<(), yaxml::Error>(())
```

Errors stay the cheap `Error { kind, offset }`; a caller that wants
libxml2's `xmlError` describes one from the input it parsed, or a recovered
document's list: domain, code, level, line, column and libxml2's message,
and `render` prints xmllint's report ([error mapping](docs/bindings.md#error-mapping)).
`LineIndex` also gives node lines (`xmlGetLineNo`) and the validators'
diagnostics convert to the same shape:

```rust
use yaxml::{Document, LineIndex, ParseOptions, options};

let input = b"<r><a>text</b><p:c/>";
let document = Document::parse_with(input, ParseOptions::new(options::RECOVER))?;
let lines = LineIndex::new(input);
let found: Vec<_> = lines.describe_all(document.errors()).into_iter().map(|d| d.message).collect();
assert_eq!(found, ["Opening and ending tag mismatch: a line 1 and b", "Namespace prefix p on c is not defined"]);
assert_eq!(document.root_element().line(&lines), Some(1));
# Ok::<(), yaxml::Error>(())
```
