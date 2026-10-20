"""lxml.etree-style XML processing on yaxml, a safe Rust libxml2 rewrite.

The native module ``yaxml._yaxml`` holds the tree, XPath, serializers and
validators; this module adds the parsing entry points, ``tostring``,
``ElementTree`` and the validator wrappers. See ``bindings/python/README.md``.
"""

from __future__ import annotations

import os
from collections.abc import Iterator, Mapping
from typing import IO, Any, Callable, Union

from . import _errors
from ._errors import *  # noqa: F403
from ._errors import DocumentInvalid, ErrorLog, XMLSyntaxError, log
from ._yaxml import (
    CDATA,
    LIBXML2_COMPAT_VERSION,
    PI,
    YAXML_VERSION,
    Comment,
    Element,
    Entity,
    FileResolver,
    ProcessingInstruction,
    SubElement,
    XPath,
    _Attrib,
    _Document,
    _DTD,
    _Element,
    _RelaxNG,
    _Schematron,
    _XMLSchema,
    __version__,
    parse_document as _parse_document,
)
from ._yaxml import iterparse as _iterparse

__all__ = [
    "CDATA",
    "DTD",
    "PI",
    "Comment",
    "DocInfo",
    "Element",
    "ElementTree",
    "Entity",
    "FileResolver",
    "LIBXML2_COMPAT_VERSION",
    "ProcessingInstruction",
    "QName",
    "RelaxNG",
    "Schematron",
    "SubElement",
    "XML",
    "XMLParser",
    "XMLSchema",
    "XPath",
    "c14n",
    "fromstring",
    "iselement",
    "iterparse",
    "parse",
    "tostring",
    *_errors.__all__,
]

Source = Union[bytes, bytearray, str, "os.PathLike[str]", IO[bytes], IO[str]]
Resolver = Union[FileResolver, Callable[[str, "str | None", str], "bytes | str | None"]]

# libxml2's xmlParserOption values (yaxml.options in Rust).
XML_PARSE_RECOVER = 1 << 0
XML_PARSE_NOENT = 1 << 1
XML_PARSE_DTDLOAD = 1 << 2
XML_PARSE_DTDATTR = 1 << 3
XML_PARSE_DTDVALID = 1 << 4
XML_PARSE_NOBLANKS = 1 << 8
XML_PARSE_XINCLUDE = 1 << 10
XML_PARSE_NSCLEAN = 1 << 13
XML_PARSE_NOCDATA = 1 << 14
XML_PARSE_HUGE = 1 << 19
XML_PARSE_NO_XXE = 1 << 23
XML_PARSE_SKIP_IDS = 1 << 27

_OPTIONS = {
    "recover": XML_PARSE_RECOVER,
    "noent": XML_PARSE_NOENT,
    "dtd_load": XML_PARSE_DTDLOAD,
    "dtd_attr": XML_PARSE_DTDATTR,
    "dtd_validation": XML_PARSE_DTDVALID,
    "no_blanks": XML_PARSE_NOBLANKS,
    "xinclude": XML_PARSE_XINCLUDE,
    "ns_clean": XML_PARSE_NSCLEAN,
    "no_cdata": XML_PARSE_NOCDATA,
    "huge": XML_PARSE_HUGE,
    "no_xxe": XML_PARSE_NO_XXE,
    "skip_ids": XML_PARSE_SKIP_IDS,
}


def _flags(options: Mapping[str, Any]) -> int:
    flags = 0
    for name, value in options.items():
        if name not in _OPTIONS:
            raise TypeError(f"unknown parse option {name!r}")
        if value:
            flags |= _OPTIONS[name]
    return flags


class XMLParser:
    """lxml's ``XMLParser`` options, mapped onto libxml2 flags.

    Defaults follow lxml: entities are substituted (no external loading
    without a ``resolver``) and CDATA sections become text.
    """

    def __init__(
        self,
        *,
        recover: bool = False,
        resolve_entities: bool = True,
        load_dtd: bool = False,
        attribute_defaults: bool = False,
        dtd_validation: bool = False,
        remove_blank_text: bool = False,
        huge_tree: bool = False,
        ns_clean: bool = False,
        strip_cdata: bool = True,
        xinclude: bool = False,
        collect_ids: bool = True,
        resolver: Resolver | None = None,
        base_url: str | None = None,
    ) -> None:
        self.flags = _flags(
            {
                "recover": recover,
                "noent": resolve_entities,
                "dtd_load": load_dtd,
                "dtd_attr": attribute_defaults,
                "dtd_validation": dtd_validation,
                "no_blanks": remove_blank_text,
                "huge": huge_tree,
                "ns_clean": ns_clean,
                "no_cdata": strip_cdata,
                "xinclude": xinclude,
                "skip_ids": not collect_ids,
            }
        )
        self.resolver = resolver
        self.base_url = base_url
        self.error_log = ErrorLog()


def _read(source: Source) -> tuple[bytes | str, str | None]:
    """The document text of a parse source and its URL (for files)."""
    if isinstance(source, (bytes, bytearray)):
        return bytes(source), None
    if isinstance(source, str) and source.lstrip().startswith("<"):
        return source, None
    if hasattr(source, "read"):
        data = source.read()  # type: ignore[union-attr]
        return data, getattr(source, "name", None)
    path = os.fspath(source)  # type: ignore[arg-type]
    with open(path, "rb") as handle:
        return handle.read(), str(path)


def _document(
    text: bytes | str,
    parser: XMLParser | None,
    base_url: str | None,
    resolver: Resolver | None,
    options: Mapping[str, Any],
) -> _Document:
    flags = _flags(options)
    if parser is not None:
        flags |= parser.flags
        resolver = resolver or parser.resolver
        base_url = base_url or parser.base_url
    try:
        document = _parse_document(text, flags=flags, resolver=resolver, base_url=base_url)
    except XMLSyntaxError as error:
        if parser is not None:
            parser.error_log = error.error_log
        raise
    if parser is not None:
        parser.error_log = log(document.errors())
    return document


def parse(
    source: Source,
    parser: XMLParser | None = None,
    *,
    base_url: str | None = None,
    resolver: Resolver | None = None,
    **options: bool,
) -> _ElementTree:
    """Parses a file name, path, file object or bytes into an ``_ElementTree``.

    A ``str`` is a file name unless it starts with ``<``. Options:
    ``recover``, ``noent``, ``dtd_load``, ``dtd_attr``, ``dtd_validation``,
    ``no_blanks``, ``xinclude``, ``ns_clean``, ``no_cdata``, ``huge``,
    ``no_xxe``, ``skip_ids``. Nothing external is loaded without a
    ``resolver`` (a callable or :class:`FileResolver`).
    """
    text, url = _read(source)
    return _ElementTree(_document(text, parser, base_url or url, resolver, options))


def fromstring(
    text: bytes | bytearray | str,
    parser: XMLParser | None = None,
    *,
    base_url: str | None = None,
    resolver: Resolver | None = None,
    **options: bool,
) -> _Element:
    """Parses a document from bytes or str and returns its root element."""
    if not isinstance(text, (bytes, bytearray, str)):
        raise TypeError("fromstring() takes bytes or str")
    root = _document(text, parser, base_url, resolver, options).getroot()
    if root is None:
        raise XMLSyntaxError("Document has no root element", 4, 0, 0)
    return root


XML = fromstring


def iterparse(
    source: bytes | str | os.PathLike[str] | IO[bytes] | IO[str],
    events: tuple[str, ...] | list[str] = ("end",),
    tag: Any = None,
    *,
    base_url: str | None = None,
    resolver: Resolver | None = None,
    **options: bool,
) -> _iterparse:
    """Streams ``(event, element)`` pairs while building the tree, as lxml.

    Events: ``start``, ``end``, ``start-ns`` (a ``(prefix, uri)`` pair),
    ``end-ns`` (``None``), ``comment``, ``pi``. Entities are substituted
    unless ``noent=False`` is passed. ``recover``, ``dtd_validation`` and
    ``xinclude`` are not available on the streaming reader.
    """
    options = {"noent": True, **options}
    if isinstance(source, (str, os.PathLike)) and not hasattr(source, "read"):
        base_url = base_url or os.fspath(source)
        source = os.fspath(source)
    return _iterparse(
        source, list(events), tag, flags=_flags(options), resolver=resolver, base_url=base_url
    )


def _escape_text(text: str) -> str:
    return text.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;").replace("\r", "&#13;")


def _is_unicode(encoding: Any) -> bool:
    return encoding is str or (isinstance(encoding, str) and encoding.lower() == "unicode")


def c14n(
    element_or_tree: _Element | _ElementTree,
    *,
    exclusive: bool = False,
    with_comments: bool = False,
    inclusive_ns_prefixes: list[str] | None = None,
    version: str = "1.0",
) -> bytes:
    """Canonical XML 1.0/1.1 or Exclusive C14N of a tree or subtree."""
    target = element_or_tree._doc if isinstance(element_or_tree, _ElementTree) else element_or_tree
    return target._c14n(exclusive, with_comments, inclusive_ns_prefixes, version)


def tostring(
    element_or_tree: _Element | _ElementTree,
    *,
    encoding: str | type[str] | None = None,
    method: str = "xml",
    xml_declaration: bool | None = None,
    pretty_print: bool = False,
    with_tail: bool = True,
    standalone: bool | None = None,
    exclusive: bool = False,
    with_comments: bool = True,
    inclusive_ns_prefixes: list[str] | None = None,
) -> bytes | str:
    """Serializes an element (with its tail) or a whole tree, as lxml does.

    ``encoding=None`` writes ASCII with character references; ``"unicode"``
    or ``str`` returns a ``str``. The XML declaration is written by default
    only for encodings other than ASCII and UTF-8.
    """
    if method == "c14n":
        return c14n(
            element_or_tree,
            exclusive=exclusive,
            with_comments=with_comments,
            inclusive_ns_prefixes=inclusive_ns_prefixes,
        )
    if method == "c14n2":
        raise ValueError("C14N 2.0 is not supported")
    unicode = _is_unicode(encoding)
    if unicode and xml_declaration:
        raise ValueError("Serialisation to unicode must not request an XML declaration")
    label = "UTF-8" if unicode else (encoding or "ASCII")
    assert isinstance(label, str)
    if xml_declaration is None:
        normalized = label.upper().replace("_", "-")
        xml_declaration = not unicode and normalized not in ("US-ASCII", "ASCII", "UTF-8", "UTF8")
    tree = isinstance(element_or_tree, _ElementTree)
    if tree:
        document = element_or_tree._doc
        data = document._serialize(label, pretty_print, method)
    else:
        data = element_or_tree._serialize(label, pretty_print, method)
        tail = element_or_tree.tail if with_tail else None
        if tail:
            escaped = tail if method == "text" else _escape_text(tail)
            data += escaped.encode("utf-8" if method == "text" else label, "xmlcharrefreplace")
        if pretty_print and method == "xml":
            data += b"\n"
    if method == "text":
        text = data.decode("utf-8")
        return text if unicode else text.encode(label if encoding else "ascii", "strict")
    if xml_declaration and method == "xml":
        version = element_or_tree._doc.xml_version if tree else "1.0"
        declaration = f"<?xml version='{version}' encoding='{label}'"
        if standalone is not None:
            declaration += f" standalone='{'yes' if standalone else 'no'}'"
        data = (declaration + "?>\n").encode(label) + data
    return data.decode("utf-8") if unicode else data


def iselement(element: Any) -> bool:
    return isinstance(element, _Element)


class QName:
    """A namespace-qualified name: ``QName(uri, local)`` or ``QName("{uri}local")``."""

    def __init__(self, text_or_uri_or_element: Any, tag: str | None = None) -> None:
        if isinstance(text_or_uri_or_element, _Element):
            text_or_uri_or_element = text_or_uri_or_element.tag
        if tag is not None:
            text = f"{{{text_or_uri_or_element}}}{tag}" if text_or_uri_or_element else tag
        else:
            text = str(text_or_uri_or_element)
        self.text = text
        if text.startswith("{"):
            self.namespace: str | None
            self.namespace, self.localname = text[1:].split("}", 1)
        else:
            self.namespace, self.localname = None, text

    def __str__(self) -> str:
        return self.text

    def __repr__(self) -> str:
        return f"QName({self.text!r})"

    def __eq__(self, other: object) -> bool:
        return self.text == (other.text if isinstance(other, QName) else other)

    def __hash__(self) -> int:
        return hash(self.text)


class DocInfo:
    """Document information: lxml's ``tree.docinfo``."""

    def __init__(self, document: _Document) -> None:
        self._doc = document

    URL = property(lambda self: self._doc.url)
    encoding = property(lambda self: self._doc.encoding)
    xml_version = property(lambda self: self._doc.xml_version)
    standalone = property(lambda self: self._doc.standalone)
    root_name = property(lambda self: self._doc.root_name)
    public_id = property(lambda self: self._doc.public_id)
    system_url = property(lambda self: self._doc.system_url)
    internalDTD = property(lambda self: self._doc.internal_subset)

    @property
    def doctype(self) -> str:
        if self._doc.root_name is None or (
            self.public_id is None and self.system_url is None and self.internalDTD is None
        ):
            return ""
        doctype = f"<!DOCTYPE {self.root_name}"
        if self.public_id is not None:
            doctype += f' PUBLIC "{self.public_id}"'
            if self.system_url is not None:
                doctype += f' "{self.system_url}"'
        elif self.system_url is not None:
            doctype += f' SYSTEM "{self.system_url}"'
        return doctype + ">"


class _ElementTree:
    """A document with an optional context element: lxml's ``_ElementTree``."""

    def __init__(self, document: _Document | None, context: _Element | None = None) -> None:
        self._doc = document
        self._context = context

    def getroot(self) -> _Element | None:
        if self._context is not None:
            return self._context
        return self._doc.getroot() if self._doc is not None else None

    def _setroot(self, root: _Element) -> None:
        self._doc, self._context = root._document(), root

    def _root(self) -> _Element:
        root = self.getroot()
        if root is None:
            raise ValueError("ElementTree has no root")
        return root

    @property
    def docinfo(self) -> DocInfo:
        if self._doc is None:
            raise ValueError("ElementTree has no document")
        return DocInfo(self._doc)

    @property
    def parser(self) -> None:
        return None

    def _relative(self, path: str) -> str:
        return "." + path if path.startswith("/") else path

    def find(self, path: str, namespaces: Mapping[str | None, str] | None = None) -> _Element | None:
        return self._root().find(self._relative(path), namespaces)

    def findall(self, path: str, namespaces: Mapping[str | None, str] | None = None) -> list[_Element]:
        return self._root().findall(self._relative(path), namespaces)

    def iterfind(self, path: str, namespaces: Mapping[str | None, str] | None = None) -> Iterator[_Element]:
        return self._root().iterfind(self._relative(path), namespaces)

    def findtext(
        self, path: str, default: Any = None, namespaces: Mapping[str | None, str] | None = None
    ) -> Any:
        return self._root().findtext(self._relative(path), default, namespaces)

    def iter(self, *tags: Any, tag: Any = None) -> Iterator[_Element]:
        return self._root().iter(*tags, tag=tag)

    def getpath(self, element: _Element) -> str | None:
        return self.docinfo._doc.getpath(element)

    def xpath(self, _path: str, namespaces: Mapping[str, str] | None = None, **variables: Any) -> Any:
        if self._context is not None:
            return self._context.xpath(_path, namespaces, **variables)
        assert self._doc is not None
        return self._doc.xpath(_path, namespaces, **variables)

    def write(
        self,
        file: str | os.PathLike[str] | IO[bytes],
        *,
        encoding: str | None = None,
        method: str = "xml",
        xml_declaration: bool | None = None,
        pretty_print: bool = False,
        with_tail: bool = True,
        standalone: bool | None = None,
    ) -> None:
        data = tostring(
            self,
            encoding=encoding,
            method=method,
            xml_declaration=xml_declaration,
            pretty_print=pretty_print,
            with_tail=with_tail,
            standalone=standalone,
        )
        payload = data.encode("utf-8") if isinstance(data, str) else data
        if hasattr(file, "write"):
            file.write(payload)  # type: ignore[union-attr]
        else:
            with open(os.fspath(file), "wb") as handle:  # type: ignore[arg-type]
                handle.write(payload)

    def write_c14n(
        self,
        file: str | os.PathLike[str] | IO[bytes],
        *,
        exclusive: bool = False,
        with_comments: bool = True,
        inclusive_ns_prefixes: list[str] | None = None,
    ) -> None:
        data = c14n(
            self,
            exclusive=exclusive,
            with_comments=with_comments,
            inclusive_ns_prefixes=inclusive_ns_prefixes,
        )
        if hasattr(file, "write"):
            file.write(data)  # type: ignore[union-attr]
        else:
            with open(os.fspath(file), "wb") as handle:  # type: ignore[arg-type]
                handle.write(data)

    def __copy__(self) -> _ElementTree:
        if self._doc is None:
            return _ElementTree(None)
        return _ElementTree(self._doc.__copy__())

    def __deepcopy__(self, memo: Any) -> _ElementTree:
        return self.__copy__()


def ElementTree(
    element: _Element | None = None,
    *,
    file: Source | None = None,
    parser: XMLParser | None = None,
) -> _ElementTree:
    """A tree around ``element``, or parsed from ``file``."""
    if file is not None:
        return parse(file, parser)
    if element is None:
        return _ElementTree(None)
    return _ElementTree(element._document(), element)


def _schema_source(etree: Any, file: Any) -> tuple[bytes, str]:
    if etree is not None:
        tree = etree if isinstance(etree, _ElementTree) else ElementTree(etree)
        data = tostring(tree, encoding="UTF-8", xml_declaration=False)
        url = tree.docinfo.URL if tree._doc is not None else None
        return data if isinstance(data, bytes) else data.encode(), url or ""
    if file is not None:
        text, url = _read(file)
        return (text.encode() if isinstance(text, str) else text), url or ""
    raise ValueError("pass a parsed schema document (etree) or a file")


class _Validator:
    def __init__(self) -> None:
        self._error_log = ErrorLog()
        self._impl: Any = None

    @property
    def error_log(self) -> ErrorLog:
        return self._error_log

    def validate(self, etree: _Element | _ElementTree) -> bool:
        target = etree._doc if isinstance(etree, _ElementTree) else etree
        valid, rows = self._impl.validate(target)
        self._error_log = log(rows)
        return bool(valid)

    __call__ = validate

    def assertValid(self, etree: _Element | _ElementTree) -> None:
        if not self.validate(etree):
            first = self._error_log.last_error or (self._error_log[0] if self._error_log else None)
            raise DocumentInvalid(first.message if first else "Document is invalid", self._error_log)

    assert_ = assertValid


class XMLSchema(_Validator):
    """XML Schema (XSD 1.0). Includes and imports load only through ``resolver``."""

    def __init__(self, etree: Any = None, *, file: Any = None, resolver: Resolver | None = None) -> None:
        super().__init__()
        data, url = _schema_source(etree, file)
        self._impl = _XMLSchema(data, url, resolver)


class RelaxNG(_Validator):
    """RELAX NG (XML syntax). ``include``/``externalRef`` load only through ``resolver``."""

    def __init__(self, etree: Any = None, *, file: Any = None, resolver: Resolver | None = None) -> None:
        super().__init__()
        data, url = _schema_source(etree, file)
        self._impl = _RelaxNG(data, url, resolver)


class DTD(_Validator):
    """An external DTD from a file, file object or bytes (``DTD(file)``)."""

    def __init__(self, file: Any = None, *, external_id: Any = None) -> None:
        super().__init__()
        if external_id is not None:
            raise ValueError("DTD(external_id=...) needs a catalog; pass the file instead")
        if file is None:
            raise ValueError("DTD needs a file")
        text = file if isinstance(file, (bytes, bytearray)) else _read(file)[0]
        self._impl = _DTD(text.encode() if isinstance(text, str) else bytes(text))

    @property
    def name(self) -> str | None:
        return self._impl.name

    def elements(self) -> list[str]:
        return self._impl.elements()


class Schematron(_Validator):
    """ISO Schematron as libxml2 implements it (XPath 1.0 queries)."""

    def __init__(self, etree: Any = None, *, file: Any = None, resolver: Resolver | None = None) -> None:
        super().__init__()
        data, url = _schema_source(etree, file)
        self._impl = _Schematron(data, url, resolver)
