# yaxml for Python

An `lxml.etree`-style XML API on [yaxml](../../README.md), a safe Rust
rewrite of libxml2 2.15.4. The native module (`yaxml._yaxml`, PyO3) holds the
trees, XPath, serializers and validators; `yaxml/__init__.py` adds the parse
entry points, `tostring`, `ElementTree` and the validator wrappers. PyO3
contains every unsafe line: this crate sets `unsafe_code = "forbid"` and has
no `unsafe` of its own.

```python
import yaxml as etree

root = etree.fromstring(b"<r xmlns:p='urn:p'><p:a k='v'>text</p:a>tail</r>")
a = root.find("{urn:p}a")
a.set("k", "w")
etree.SubElement(root, "b").text = "more"
print(etree.tostring(root))  # b'<r xmlns:p="urn:p"><p:a k="w">text</p:a>tail<b>more</b></r>'
print(root.xpath("string(//p:a/@k)", namespaces={"p": "urn:p"}))  # 'w'
```

## Build and test

```sh
cd bindings/python
uv sync --group compare   # builds the extension with maturin (release) and installs lxml
uv run pytest -q          # every API, plus the lxml differential when the corpus is found
uv run mypy python/yaxml && uv run python -m mypy.stubtest yaxml._yaxml
uv run python bench/bench_parse.py
```

The crate is a member of the `bindings/` Cargo workspace, so pyo3 never
enters the core crate's dependency graph or lockfile. Rust edits are picked up
by the next `uv sync`/`uv run` (`cache-keys` in `pyproject.toml`), or by
`uv run maturin develop --release --uv`. The differential test reads the
libxml2 corpus from `$YAXML_LIBXML2_SOURCE/test`.

macOS 27 with Apple ld 27037 can leave the extension's symbol string pool
4-byte aligned, which dyld rejects (`mis-aligned LINKEDIT string pool`).
`tooling/fix_macho_strpool.py <the .so>` shifts the pool into its padding and
re-signs ad hoc; Linux is unaffected.

## API

| lxml | yaxml | Notes |
| --- | --- | --- |
| `parse(source, parser)` | same, plus keyword options | `source`: bytes, path, file object, or a `str` starting with `<` (otherwise a file name). Returns `_ElementTree`. |
| `fromstring`, `XML` | same | `str` input is UTF-8 with its declared encoding ignored. |
| `XMLParser(...)` | lxml's option names and defaults | `resolve_entities`, `load_dtd`, `attribute_defaults`, `dtd_validation`, `remove_blank_text`, `huge_tree`, `ns_clean`, `strip_cdata`, `recover`, `xinclude`, `collect_ids`, `resolver`, `base_url`; `parser.error_log` after a parse. |
| keyword options | `recover`, `noent`, `dtd_load`, `dtd_attr`, `dtd_validation`, `no_blanks`, `xinclude`, `ns_clean`, `no_cdata`, `huge`, `no_xxe`, `skip_ids` | libxml2's `XML_PARSE_*`; defaults are libxml2's (entity references and CDATA kept, nothing loaded). |
| resolvers | `resolver=` callable `(uri, public_id, kind) -> bytes \| str \| None`, or `FileResolver(root)` | Nothing external is loaded without one. `kind`: `dtd`, `parameter_entity`, `entity`, `xinclude`, `xinclude_text`, `schema`, `relaxng`. A resolver's exception propagates. |
| `_Element` | `tag` (Clark `{ns}local`), `text`, `tail`, `attrib`, `get/set/keys/values/items`, sequence protocol (`len`, index, slices, `del`, assignment), `append/extend/insert/remove/replace/addnext/addprevious/clear/index`, `getparent/getnext/getprevious/getroottree`, `iter/iterchildren/iterdescendants/iterancestors/itersiblings/itertext`, `find/findall/iterfind/findtext`, `xpath`, `nsmap`, `prefix`, `sourceline`, `base`, `makeelement`, `copy`/`deepcopy` | Comments, PIs and entity references are `_Element`s whose `tag` is `Comment`, `PI` or `Entity`, as in lxml. |
| `Element`, `SubElement`, `Comment`, `PI`/`ProcessingInstruction`, `CDATA`, `QName`, `iselement`, `ElementTree` | same | New namespaced names get lxml's `ns0`, `ns1`, ... unless a binding is in scope or given in `nsmap`. `Entity(name)` cannot create references. |
| `tostring(..., encoding, method, xml_declaration, pretty_print, with_tail, standalone)` | same | Methods `xml`, `html`, `text`, `c14n`. `encoding=None` is ASCII with decimal character references; `"unicode"`/`str` returns `str`. |
| `c14n`, `write_c14n` | `c14n(el_or_tree, exclusive=, with_comments=, inclusive_ns_prefixes=, version="1.0"\|"1.1")` | No C14N 2.0. |
| `iterparse(source, events, tag)` | same | On the streaming `TextReader`: `start`, `end`, `start-ns`, `end-ns`, `comment`, `pi`; the tree grows as you iterate (`.root`). Entities substituted by default, as in lxml. |
| `XPath(path, namespaces=)` | same; `__call__(el_or_tree, **variables)` | Results: elements, `str` (text, attributes), `float`, `bool`, `(prefix, uri)` for namespace nodes. |
| `XMLSchema`, `RelaxNG`, `DTD`, `Schematron` | `validate`, `__call__`, `assertValid`, `assert_`, `error_log` | From a tree or `file=`; `resolver=` for includes and imports. |
| exceptions | `Error`, `XMLSyntaxError` (`code`, `domain`, `lineno`, `offset`, `position`, `error_log`), `DocumentInvalid`, `XPathEvalError`, `XPathSyntaxError`, `XMLSchemaParseError`, `RelaxNGParseError`, `DTDParseError`, `SchematronParseError`, `C14NError`, `SerialisationError`, `StaleElementError` | Codes, domains, levels, lines and messages are libxml2 2.15.4's. |

## Proxies, identity and moves

An element proxy holds an `Arc` to its document and an `OwnedNodeId`. Each
document keeps a registry of weak references, so a node has at most one live
proxy and `is`, `==` and `hash` are identity, as in lxml. Moving a node to
another document imports it, inserts the copy, removes the original and
points the live proxies of the moved subtree at their copies
([docs/bindings.md](../../docs/bindings.md#moving-nodes-between-documents)).
Moving a document's root element copies it (an arena document keeps its
element). A proxy whose node no longer exists raises `StaleElementError` (a
`ValueError`), never crashes; today that happens to a reference to an entity
the destination does not declare, which a move replaces by its content.

Tails follow lxml: they move with their element, survive `remove`, and are
written by `tostring(element)`. An unlinked or top-level node keeps its tail
in a side table, since an arena tree cannot link siblings without a parent.

## Threads and free-threading

Each document sits behind an `RwLock` (taken with PyO3's
`read_py_attached`/`write_py_attached`, which detach while blocked); proxies
keep their location in a `Mutex`. Parsing, serialization, `xpath()`, C14N and
validation run with the GIL released. The module declares `gil_used = false`:
on free-threaded CPython 3.15t (3.15.0b1) the whole suite, threading tests
included, passes with the GIL disabled. CI does not build free-threaded
wheels yet; they need a per-interpreter (non-abi3) build, which is what this
crate already produces.

## Differences from lxml

- `parse`/`fromstring` keep libxml2's defaults; `XMLParser()` gives lxml's.
  Without a resolver nothing is loaded, even with `load_dtd`.
- With the default ASCII encoding, a non-ASCII name, comment or PI makes
  `tostring` write that document as UTF-8 with hexadecimal references in text
  (libxml2 would write references inside names and comments, changing them).
- `iter*`, `itertext` and `iterfind` iterate over a snapshot taken when called.
- iterparse elements, copies and moved roots have no `sourceline`;
  `recover`, `dtd_validation` and `xinclude` are not available to iterparse.
- Removed children a proxy can still see are unlinked and kept until the
  document is freed; unobserved ones are deleted, but each leaves a tombstone
  record, so `iterparse` with `clear()` still grows with the node count.
- Not provided: `etree.HTML` parsing, XSLT, `iterwalk`, custom element
  classes, `XInclude()` after parsing, XPath extension functions, C14N 2.0,
  `DTD(external_id=)`, PI pseudo-attributes.
