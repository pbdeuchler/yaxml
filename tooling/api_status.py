#!/usr/bin/env python3
"""Fill `status` and `rust` in docs/libxml2-api.tsv, or verify them.

  uv run --no-project tooling/api_status.py            # regenerate the TSV
  uv run --no-project tooling/api_status.py --check    # verify only (no libxml2 source needed)

Regeneration reads libxml2 2.15.4's headers for numeric error codes
(`YAXML_LIBXML2_SOURCE`, default `yaxml-oracle/libxml2-2.15.4` next to the
worktrees). The identifier, kind, marker and priority columns are inputs;
`status` and `rust` are rewritten. tests/api_inventory.rs repeats the
`--check` rules without Python.

Status: implemented | partial | not-started | not-applicable | removed-upstream.
`rust` for implemented/partial is `path[, path][ - note]`, every path resolved
in src/ (a module, type, fn, const, enum variant or `pub` field); for the other
statuses it is a free-text reason or the nearest API.

Rule order, first hit wins: TABLE exact names, header/prefix rules, tree field
table, TABLE globs, struct-name fallback, field defaults, `deprecated_marker ==
yes` -> removed-upstream. A row no rule covers prints as `fallback` and becomes
not-started, so none stays unclassified silently.
"""
import csv
import fnmatch
import os
import re
import sys
from collections import Counter, defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TSV = ROOT / "docs" / "libxml2-api.tsv"
SRC = ROOT / "src"
STATUS = {
    "I": "implemented",
    "P": "partial",
    "N": "not-started",
    "A": "not-applicable",
    "R": "removed-upstream",
}
PATH_RE = re.compile(r"^[A-Za-z_]\w*(::\w+)*$")


# --------------------------------------------------------------------------
# Rust path resolution (mirrored by tests/api_inventory.rs)

FILES = [p for p in sorted(SRC.rglob("*.rs")) if p.relative_to(SRC).parts[0] != "bin"]
TEXT = {p: p.read_text() for p in FILES}
SCOPE_KW = r"(?:fn|struct|enum|trait|type|mod|union)"
ANY_KW = r"(?:fn|const|static|struct|enum|trait|type|mod|union)"


def family(f):
    """`f` plus the files of the module it roots (`f` stem directory, or its own for mod.rs)."""
    out = {f}
    d = f.with_suffix("")
    if d.is_dir():
        out |= {p for p in FILES if d in p.parents}
    if f.name == "mod.rs":
        out |= {p for p in FILES if f.parent in p.parents}
    return out & set(FILES)


def scope_of(name, files):
    """Files that a declaration of `name` in `files` makes the next lookup scope."""
    out = set()
    pat = re.compile(rf"\b({SCOPE_KW})\s+{re.escape(name)}\b(\s*;)?")
    for f in files:
        for m in pat.finditer(TEXT[f]):
            if m.group(1) != "mod":
                out |= family(f)
            elif m.group(2):
                base = f.parent if f.name in ("lib.rs", "mod.rs") else f.with_suffix("")
                out |= family(base / f"{name}.rs") | family(base / name / "mod.rs") | ({base / f"{name}.rs"} & set(FILES))
            else:
                out.add(f)
    return out


def declared(name, files):
    pat = re.compile(
        rf"\b{ANY_KW}\s+{re.escape(name)}\b|\bpub\s+{re.escape(name)}\s*:"
        + (rf"|^\s*{re.escape(name)}\s*(?:[,({{=]|$)" if name[:1].isupper() else ""),
        re.M,
    )
    return any(pat.search(TEXT[f]) for f in files)


def resolves(path):
    segs = path.split("::")
    while segs and segs[0] in ("yaxml", "crate"):
        segs = segs[1:]
    if not segs:
        return False
    scope = set(FILES)
    for seg in segs[:-1]:
        scope = scope_of(seg, scope)
        if not scope:
            return False
    return declared(segs[-1], scope)


def paths_of(rust):
    return [p.strip() for p in rust.split(" - ", 1)[0].split(",")]


def check_rows(rows):
    errors = []
    for r in rows:
        s, rust = r["status"], r["rust"]
        if s not in STATUS.values():
            errors.append(f"{r['identifier']}: bad status {s!r}")
        elif not rust:
            errors.append(f"{r['identifier']}: empty rust")
        elif s in ("implemented", "partial"):
            for p in paths_of(rust):
                if not PATH_RE.match(p):
                    errors.append(f"{r['identifier']}: not a path {p!r}")
                elif not resolves(p):
                    errors.append(f"{r['identifier']}: {p} not found in src/")
    return errors


# --------------------------------------------------------------------------
# libxml2 error code values (regeneration only)

def source_dir():
    env = os.environ.get("YAXML_LIBXML2_SOURCE")
    cands = [Path(env)] if env else [ROOT.parent / "yaxml-oracle" / "libxml2-2.15.4", ROOT.parent.parent / "yaxml-oracle" / "libxml2-2.15.4"]
    for c in cands:
        if (c / "include" / "libxml" / "xmlerror.h").exists():
            return c
    sys.exit("set YAXML_LIBXML2_SOURCE to the libxml2-2.15.4 source tree")


def enum_values(header, start, end):
    text = re.sub(r"/\*.*?\*/", "", header.read_text(), flags=re.S)
    text = re.sub(r"^\s*#.*$", "", text, flags=re.M)
    a = text.index(start)
    out, n = {}, -1
    for item in text[a: text.index(end, a)].split(","):
        m = re.fullmatch(r"\s*(\w+)(?:\s*=\s*(\w+))?\s*", item)
        if m:
            name, val = m.groups()
            n = int(val, 0) if val and val[0].isdigit() else out[val] if val else n + 1
            out[name] = n
    return out


def rust_codes(file, enum):
    """{discriminant: Variant} of `pub enum <enum> { A = 1, ... }` in src/<file>."""
    text = TEXT[SRC / file]
    body = text[text.index(f"pub enum {enum}"):]
    body = body[: body.index("\n}")]
    return {int(n): v for v, n in re.findall(r"^\s*(\w+)\s*=\s*(\d+)", body, re.M)}


# --------------------------------------------------------------------------
# Name tables

OPT = {  # XML_PARSE_<x> -> (status, note)
    "SAX1": ("P", "accepted; no SAX1 handlers"),
    "XINCLUDE": ("P", "OwnedDocument only"),
    "RECOVER": ("P", "every surface; repair differences in compatibility.md#recovery-recover"),
    "DTDVALID": ("P", "DOMs and TextReader; valid::StreamValidator for other streams"),
    "NOBLANKS": ("P", "every surface but the slice Reader"),
    "PEDANTIC": ("P", "accepted; libxml2 only prints more warnings"),
    "OLD10": ("N", "constant defined, rejected with UnsupportedOption"),
    "OLDSAX": ("N", "constant defined, rejected with UnsupportedOption"),
    "UNZIP": ("N", "constant defined, rejected with UnsupportedOption"),
    "CATALOG_PI": ("P", "acts only through a resolver that takes the instruction (catalog::CatalogResolver)"),
}
OPT_ALL = "RECOVER NOENT DTDLOAD DTDATTR DTDVALID NOERROR NOWARNING PEDANTIC NOBLANKS SAX1 XINCLUDE NONET NODICT NSCLEAN NOCDATA NOXINCNODE COMPACT OLD10 NOBASEFIX HUGE OLDSAX IGNORE_ENC BIG_LINES NO_XXE UNZIP NO_SYS_CATALOG CATALOG_PI SKIP_IDS".split()
HTML_OPT = {"HTML5": ("I", "token-level events; trees follow them unbalanced, as in libxml2")}
FEATURE = {
    "THREAD": "Thread", "TREE": "Tree", "OUTPUT": "Output", "PUSH": "Push", "READER": "Reader",
    "PATTERN": "Pattern", "WRITER": "Writer", "SAX1": "Sax1", "FTP": "Ftp", "HTTP": "Http",
    "VALID": "Valid", "HTML": "Html", "LEGACY": "Legacy", "C14N": "C14n", "CATALOG": "Catalog",
    "XPATH": "Xpath", "XPTR": "Xptr", "XINCLUDE": "Xinclude", "ICONV": "Iconv", "ISO8859X": "Iso8859x",
    "UNICODE": "Unicode", "REGEXP": "Regexp", "AUTOMATA": "Automata", "EXPR": "Expr", "SCHEMAS": "Schemas",
    "SCHEMATRON": "Schematron", "MODULES": "Modules", "DEBUG": "Debug", "DEBUG_MEM": "DebugMem",
    "DEBUG_RUN": "DebugRun", "ZLIB": "Zlib", "ICU": "Icu", "LZMA": "Lzma", "RELAXNG": "Relaxng",
}
SCHEMA_TYPES = {
    "ANYSIMPLETYPE": "AnySimpleType", "STRING": "String", "NORMSTRING": "NormalizedString",
    "DECIMAL": "Decimal", "TIME": "Time", "GDAY": "GDay", "GMONTH": "GMonth", "GMONTHDAY": "GMonthDay",
    "GYEAR": "GYear", "GYEARMONTH": "GYearMonth", "DATE": "Date", "DATETIME": "DateTime",
    "DURATION": "Duration", "FLOAT": "Float", "DOUBLE": "Double", "BOOLEAN": "Boolean", "TOKEN": "Token",
    "LANGUAGE": "Language", "NMTOKEN": "NmToken", "NMTOKENS": "NmTokens", "NAME": "Name", "QNAME": "QName",
    "NCNAME": "NCName", "ID": "Id", "IDREF": "IdRef", "IDREFS": "IdRefs", "ENTITY": "Entity",
    "ENTITIES": "Entities", "NOTATION": "Notation", "ANYURI": "AnyUri", "INTEGER": "Integer",
    "NPINTEGER": "NonPositiveInteger", "NINTEGER": "NegativeInteger", "NNINTEGER": "NonNegativeInteger",
    "PINTEGER": "PositiveInteger", "INT": "Int", "UINT": "UnsignedInt", "LONG": "Long",
    "ULONG": "UnsignedLong", "SHORT": "Short", "USHORT": "UnsignedShort", "BYTE": "Byte",
    "UBYTE": "UnsignedByte", "HEXBINARY": "HexBinary", "BASE64BINARY": "Base64Binary",
}
FACETS = {
    "LENGTH": "Length", "MINLENGTH": "MinLength", "MAXLENGTH": "MaxLength", "PATTERN": "Pattern",
    "ENUMERATION": "Enumeration", "WHITESPACE": "WhiteSpace", "MININCLUSIVE": "MinInclusive",
    "MAXINCLUSIVE": "MaxInclusive", "MINEXCLUSIVE": "MinExclusive", "MAXEXCLUSIVE": "MaxExclusive",
    "TOTALDIGITS": "TotalDigits", "FRACTIONDIGITS": "FractionDigits",
}
XPATH_ERR = {  # XPATH_<x> / XML_XPATH_<x> -> xpath::ErrorKind
    "UNFINISHED_LITERAL_ERROR": "UnfinishedLiteral", "START_LITERAL_ERROR": "StartLiteral",
    "VARIABLE_REF_ERROR": "VariableRef", "UNDEF_VARIABLE_ERROR": "UndefinedVariable",
    "INVALID_PREDICATE_ERROR": "InvalidPredicate", "EXPR_ERROR": "Expression", "UNCLOSED_ERROR": "Unclosed",
    "UNKNOWN_FUNC_ERROR": "UnknownFunction", "INVALID_TYPE": "InvalidType", "INVALID_ARITY": "InvalidArity",
    "UNDEF_PREFIX_ERROR": "UndefinedPrefix", "INVALID_CHAR_ERROR": "InvalidChar",
    "INVALID_CTXT": "InvalidContext", "OP_LIMIT_EXCEEDED": "LimitExceeded",
    "RECURSION_LIMIT_EXCEEDED": "LimitExceeded",
}
XINCLUDE_ERR = {
    "NO_HREF": "NoHref", "PARSE_VALUE": "InvalidParse", "HREF_URI": "InvalidHref",
    "FRAGMENT_ID": "FragmentIdentifier", "RECURSION": "Recursion", "TEXT_DOCUMENT": "TextDocument",
    "UNKNOWN_ENCODING": "UnknownEncoding", "INVALID_CHAR": "InvalidCharacter", "XPTR_FAILED": "XPointer",
    "XPTR_RESULT": "XPointerResult", "NO_FALLBACK": "NoFallback", "MULTIPLE_ROOT": "MultipleRoots",
    "INCLUDE_IN_INCLUDE": "IncludeInInclude", "FALLBACKS_IN_INCLUDE": "MultipleFallbacks",
    "FALLBACK_NOT_IN_INCLUDE": "FallbackNotInInclude",
}
PARSER_ERR = {  # XML_ERR_<x> -> ErrorKind (category only)
    "ATTRIBUTE_REDEFINED": "DuplicateAttribute", "TAG_NAME_MISMATCH": "MismatchedEndTag",
    "INVALID_CHAR": "InvalidCharacter", "INVALID_CHARREF": "InvalidReference",
    "INVALID_DEC_CHARREF": "InvalidReference", "INVALID_HEX_CHARREF": "InvalidReference",
    "ENTITY_LOOP": "EntityLoop", "UNPARSED_ENTITY": "UnparsedEntity",
    "ENTITY_IS_EXTERNAL": "ExternalEntityInAttribute", "NOT_WELL_BALANCED": "UnbalancedEntity",
    "UNSUPPORTED_ENCODING": "UnsupportedEncoding", "UNKNOWN_ENCODING": "UnsupportedEncoding",
    "NAME_REQUIRED": "InvalidName", "NO_MEMORY": "Allocation", "USER_STOP": "Aborted",
    "RESOURCE_LIMIT": "LimitExceeded", "NAME_TOO_LONG": "LimitExceeded", "NS_DECL_ERROR": "Namespace",
    "INTERNAL_ERROR": "InvalidState", "DOCUMENT_EMPTY": "MissingRoot", "DOCUMENT_END": "TextOutsideRoot",
    "EXTRA_CONTENT": "TextOutsideRoot", "MISSING_ENCODING": "EncodingMismatch",
}
PARSER_SYNTAX = re.compile(
    r"(ATTLIST|ATTRIBUTE|CDATA|COMMENT|CONDSEC|DOCTYPE|ELEMCONTENT|ENTITY|EXT_SUBSET|INT_SUBSET|MIXED|"
    r"NOTATION|PI|XMLDECL|STRING|LITERAL)_NOT_(FINISHED|STARTED)|ATTRIBUTE_WITHOUT_VALUE|COMMENT_ABRUPTLY_ENDED|"
    r"CONDSEC_INVALID(_KEYWORD)?|ENCODING_NAME|ENTITYREF_\w+|EQUAL_REQUIRED|GT_REQUIRED|HYPHEN_IN_COMMENT|"
    r"LTSLASH_REQUIRED|LT_IN_ATTRIBUTE|LT_REQUIRED|MISPLACED_CDATA_END|NMTOKEN_REQUIRED|PCDATA_REQUIRED|"
    r"PEREF_\w+|PUBID_REQUIRED|RESERVED_XML_NAME|SEPARATOR_REQUIRED|SPACE_REQUIRED|STANDALONE_VALUE|"
    r"URI_REQUIRED|VALUE_REQUIRED|VERSION_MISSING|UNKNOWN_VERSION|VERSION_MISMATCH|REDECL_PREDEF_ENTITY|"
    r"EXT_ENTITY_STANDALONE|NOT_STANDALONE|CHARREF_\w+|DOCUMENT_START|ENTITY_CHAR_ERROR|ENTITY_PE_INTERNAL|"
    r"ENTITY_PROCESSING|NOTATION_PROCESSING|INVALID_ENCODING|INVALID_URI|URI_FRAGMENT|NO_DTD|ENTITY_BOUNDARY"
)
TREE_KIND = {  # XML_*_NODE -> (status, rust)
    "ELEMENT": ("I", "NodeKind::Element, OwnedNodeKind::Element"),
    "TEXT": ("I", "NodeKind::Text, OwnedNodeKind::Text"),
    "CDATA_SECTION": ("I", "NodeKind::Cdata, OwnedNodeKind::Cdata"),
    "ENTITY_REF": ("I", "NodeKind::EntityRef, OwnedNodeKind::EntityRef"),
    "PI": ("I", "NodeKind::ProcessingInstruction, OwnedNodeKind::ProcessingInstruction"),
    "COMMENT": ("I", "NodeKind::Comment, OwnedNodeKind::Comment"),
    "DOCUMENT": ("I", "NodeKind::Document, OwnedNodeKind::Document"),
    "DOCUMENT_TYPE": ("I", "NodeKind::Doctype, OwnedNodeKind::DocumentType"),
    "DTD": ("P", "NodeKind::Doctype, OwnedNodeKind::DocumentType - the DTD node is the doctype node"),
    "HTML_DOCUMENT": ("P", "OwnedDocument::is_html - HTML documents are OwnedDocument with a flag"),
    "ATTRIBUTE": ("A", "attributes are not nodes; Node::attributes yields Attribute"),
    "ENTITY": ("N", "no entity-declaration node"),
    "DOCUMENT_FRAG": ("N", "no document fragment node"),
    "NOTATION": ("N", "no notation node"),
    "NAMESPACE_DECL": ("N", "namespace declarations are NamespaceBinding, not nodes"),
    "ELEMENT_DECL": ("N", "no public DTD declaration nodes"),
    "ATTRIBUTE_DECL": ("N", "no public DTD declaration nodes"),
    "ENTITY_DECL": ("N", "no public DTD declaration nodes"),
}
READER_TYPE = {
    "ATTRIBUTE": "Attribute", "CDATA": "Cdata", "COMMENT": "Comment", "DOCUMENT_TYPE": "DocumentType",
    "ELEMENT": "Element", "END_ELEMENT": "EndElement", "ENTITY_REFERENCE": "EntityReference",
    "NONE": "None", "PROCESSING_INSTRUCTION": "ProcessingInstruction",
    "SIGNIFICANT_WHITESPACE": "SignificantWhitespace", "TEXT": "Text",
    "DOCUMENT": "Document", "DOCUMENT_FRAGMENT": "DocumentFragment", "END_ENTITY": "EndEntity",
    "ENTITY": "Entity", "NOTATION": "Notation", "WHITESPACE": "Whitespace", "XML_DECLARATION": "XmlDeclaration",
}
# Values libxml2 2.15.4's reader never reports (xmllint --stream --debug over its test tree).
READER_TYPE_UNUSED = {"DOCUMENT", "DOCUMENT_FRAGMENT", "END_ENTITY", "ENTITY", "NOTATION", "WHITESPACE", "XML_DECLARATION"}
READER_MODE = {"INITIAL": "Initial", "INTERACTIVE": "Interactive", "ERROR": "Error", "EOF": "Eof"}
READER_PROP = {"LOADDTD": "LoadDtd", "DEFAULTATTRS": "DefaultAttributes", "VALIDATE": "Validate", "SUBST_ENTITIES": "SubstituteEntities"}
RESOURCE = {"DTD": "DtdSubset", "GENERAL_ENTITY": "GeneralEntity", "PARAMETER_ENTITY": "ParameterEntity", "XINCLUDE": "XInclude", "XINCLUDE_TEXT": "XIncludeText"}
ENC = {"UTF8", "UTF16", "UTF16LE", "UTF16BE", "ASCII", "WINDOWS_1252"} | {f"8859_{n}" for n in (1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 14, 15, 16)}

# Struct fields of these libxml2 structs have no Rust counterpart by design.
FIELD_DEFAULT = {
    "_xmlParserCtxt": ("A", "internal parser state; parsers are opaque Rust types"),
    "_xmlParserInput": ("A", "internal input state; std Read/BufRead replace it"),
    "_xmlParserNodeInfo": ("N", "no node-info table (Node::range gives byte ranges)"),
    "_xmlParserNodeInfoSeq": ("N", "no node-info table (Node::range gives byte ranges)"),
    "_xmlSAXHandler": ("N", "SAX2 handler table not started (roadmap 15)"),
    "_xmlSAXHandlerV1": ("N", "SAX1 handler table not started"),
    "_xmlSAXLocator": ("N", "SAX locator not started (roadmap 15)"),
    "_xmlBuffer": ("A", "Vec<u8>/String replace xmlBuffer"),
    "_xmlOutputBuffer": ("A", "std::io::Write replaces xmlOutputBuffer"),
    "_xmlParserInputBuffer": ("A", "std::io::Read replaces xmlParserInputBuffer"),
    "_xmlValidCtxt": ("A", "valid::StreamValidator keeps its state private; valid::validate takes none"),
    "_xmlXPathContext": ("A", "xpath::Context is a builder; no public fields"),
    "_xmlXPathParserContext": ("A", "evaluation stack is internal"),
    "_xmlXPathObject": ("P", "xpath::Value"),
    "_xmlNodeSet": ("A", "Vec<xpath::Node> replaces xmlNodeSet"),
    "_xmlXPathAxis": ("A", "axes are built in"),
    "_xmlXPathFunct": ("A", "functions registered through Context::with_function"),
    "_xmlXPathType": ("A", "no custom XPath types"),
    "_xmlXPathVariable": ("A", "variables registered through Context::with_variable"),
    "_xmlChLRange": ("A", "the chars predicates keep their range tables private"),
    "_xmlChSRange": ("A", "the chars predicates keep their range tables private"),
    "_xmlChRangeGroup": ("A", "the chars predicates keep their range tables private"),
    "_xmlCharEncodingHandler": ("N", "no public encoding handler registry"),
    "_xmlDOMWrapCtxt": ("A", "no DOM-wrapper context; copies plan namespaces themselves"),
    "_xmlEntity": ("A", "tree links and bookkeeping; valid::Entity is a borrowed view"),
    "_xmlElement": ("A", "tree links and bookkeeping; valid::Element is a borrowed view"),
    "_xmlAttribute": ("A", "tree links and bookkeeping; valid::Attribute is a borrowed view"),
    "_xmlNotation": ("A", "valid::Notation is a borrowed view"),
    "_xmlDtd": ("A", "tree links; valid::Dtd is read through its accessors"),
    "_xmlElementContent": ("N", "no public content-model tree"),
    "_xmlEnumeration": ("N", "no public enumeration list"),
    "_xmlID": ("N", "IDs via Document::element_by_id"),
    "_xmlRef": ("N", "no IDREF table"),
    "_xmlError": ("A", "no counterpart field"),
    "_htmlElemDesc": ("N", "no public HTML element table"),
    "_htmlEntityDesc": ("N", "no public HTML entity table"),
}
for _s in "Schema SchemaAnnot SchemaAttribute SchemaAttributeGroup SchemaAttributeLink SchemaElement SchemaFacet SchemaFacetLink SchemaNotation SchemaType SchemaTypeLink SchemaWildcard SchemaWildcardNs".split():
    FIELD_DEFAULT[f"_xml{_s}"] = ("N", "no public schema component model; xsd::Schema is opaque")

TREE_FIELDS = {
    "_xmlNode.name": ("I", "Node::name, OwnedNode::name"),
    "_xmlNode.children": ("I", "Node::first_child, OwnedNode::first_child"),
    "_xmlNode.last": ("I", "Node::last_child, OwnedNode::last_child"),
    "_xmlNode.next": ("I", "Node::next_sibling, OwnedNode::next_sibling"),
    "_xmlNode.prev": ("I", "Node::previous_sibling, OwnedNode::previous_sibling"),
    "_xmlNode.parent": ("I", "Node::parent, OwnedNode::parent"),
    "_xmlNode.type": ("I", "Node::kind, OwnedNode::kind"),
    "_xmlNode.content": ("P", "Node::text, OwnedNode::text"),
    "_xmlNode.properties": ("I", "Node::attributes, OwnedNode::attributes"),
    "_xmlNode.ns": ("P", "Name::namespace"),
    "_xmlNode.nsDef": ("P", "Node::namespaces, OwnedNode::namespaces"),
    "_xmlNode.line": ("P", "diagnostic::LineIndex - computed from the node offset by Node::line and OwnedNode::line"),
    "_xmlNode.doc": ("A", "nodes borrow their document"),
    "_xmlNode._private": ("A", "no user pointer"),
    "_xmlNode.psvi": ("A", "no schema PSVI"),
    "_xmlNode.extra": ("A", "internal"),
    "_xmlAttr.name": ("I", "Attribute::name"),
    "_xmlAttr.children": ("P", "Attribute::value, Node::unexpanded_attribute"),
    "_xmlAttr.ns": ("P", "Name::namespace"),
    "_xmlAttr.next": ("P", "Node::attributes - iteration replaces links"),
    "_xmlAttr.prev": ("P", "Node::attributes - iteration replaces links"),
    "_xmlAttr.last": ("P", "Node::attributes - iteration replaces links"),
    "_xmlAttr.parent": ("N", "attributes do not point to their element"),
    "_xmlAttr.doc": ("A", "attributes borrow their document"),
    "_xmlAttr.atype": ("N", "no public attribute type"),
    "_xmlAttr.type": ("A", "attributes are not nodes"),
    "_xmlAttr._private": ("A", "no user pointer"),
    "_xmlAttr.psvi": ("A", "no schema PSVI"),
    "_xmlDoc.URL": ("I", "Document::url, OwnedDocument::url"),
    "_xmlDoc.children": ("I", "Document::root, OwnedDocument::root"),
    "_xmlDoc.last": ("I", "Document::root, OwnedDocument::root"),
    "_xmlDoc.version": ("P", "Declaration"),
    "_xmlDoc.encoding": ("P", "Declaration"),
    "_xmlDoc.standalone": ("P", "Declaration"),
    "_xmlDoc.intSubset": ("P", "Document::doctype, OwnedDocument::doctype"),
    "_xmlDoc.ids": ("P", "Document::element_by_id, OwnedDocument::element_by_id"),
    "_xmlDoc.type": ("P", "OwnedDocument::is_html"),
    "_xmlDoc.extSubset": ("N", "external subset not exposed"),
    "_xmlDoc.name": ("N", "not stored"),
    "_xmlDoc.properties": ("N", "no document property flags"),
    "_xmlDoc.parseFlags": ("N", "flags are not stored"),
    "_xmlDoc.charset": ("N", "not stored"),
    "_xmlDoc.compression": ("A", "no compression"),
    "_xmlDoc.dict": ("A", "no dictionary"),
    "_xmlDoc.oldNs": ("A", "internal"),
    "_xmlDoc._private": ("A", "no user pointer"),
    "_xmlDoc.psvi": ("A", "no schema PSVI"),
    "_xmlDoc.doc": ("A", "internal self-pointer"),
    "_xmlDoc.next": ("A", "documents are not linked"),
    "_xmlDoc.prev": ("A", "documents are not linked"),
    "_xmlDoc.parent": ("A", "documents are not linked"),
    "_xmlNs.href": ("I", "NamespaceBinding::uri"),
    "_xmlNs.prefix": ("I", "NamespaceBinding::prefix"),
    "_xmlNs.next": ("P", "Node::namespaces - iteration replaces links"),
    "_xmlNs.type": ("A", "namespace nodes carry no type tag"),
    "_xmlNs._private": ("A", "no user pointer"),
    "_xmlDtd.name": ("I", "valid::Dtd::name"),
    "_xmlDtd.ExternalID": ("I", "reader::Doctype::public_id"),
    "_xmlDtd.SystemID": ("I", "reader::Doctype::system_id"),
    "_xmlError.code": ("I", "diagnostic::Diagnostic::code"),
    "_xmlError.domain": ("I", "diagnostic::Diagnostic::domain"),
    "_xmlError.level": ("I", "diagnostic::Diagnostic::level"),
    "_xmlError.line": ("I", "diagnostic::Diagnostic::line"),
    "_xmlError.int2": ("I", "diagnostic::Diagnostic::column"),
    "_xmlError.message": ("I", "diagnostic::Diagnostic::message - without the trailing newline"),
    "_xmlError.str1": ("I", "diagnostic::Diagnostic::str1"),
    "_xmlError.str2": ("I", "diagnostic::Diagnostic::str2"),
    "_xmlError.int1": ("I", "diagnostic::Diagnostic::int1"),
    "_xmlError.node": ("P", "diagnostic::Diagnostic::element - the element's local name"),
    "_xmlError.file": ("A", "the caller's file name, passed to Diagnostic::render"),
    "_xmlError.str3": ("N", "no third string in the errors yaxml describes"),
    "_xmlError.ctxt": ("A", "no parser context object"),
    "_xmlURI.scheme": ("I", "Uri::scheme"),
    "_xmlURI.user": ("I", "Uri::user"),
    "_xmlURI.server": ("I", "Uri::host"),
    "_xmlURI.port": ("I", "Uri::port"),
    "_xmlURI.path": ("I", "Uri::path"),
    "_xmlURI.query": ("I", "Uri::query"),
    "_xmlURI.fragment": ("I", "Uri::fragment"),
    "_xmlURI.authority": ("P", "Uri::has_authority"),
    "_xmlURI.opaque": ("N", "not stored"),
    "_xmlURI.query_raw": ("N", "not stored"),
    "_xmlURI.cleanup": ("A", "internal flag"),
}

# --------------------------------------------------------------------------
# TABLE: `S|names|rust`, S in I P N A R; names separated by spaces, fnmatch globs
# allowed, `header:name` limits a name to one header.

TABLE = r"""
# ---- parser.h: entry points
I|xmlReadDoc xmlReadMemory xmlParseDoc xmlParseMemory|Document::parse_with, OwnedDocument::parse_with
P|xmlReadFd xmlReadIO xmlReadFile xmlParseFile xmlCtxtReadDoc xmlCtxtReadFd xmlCtxtReadFile xmlCtxtReadIO xmlCtxtReadMemory xmlParseDocument xmlCtxtParseDocument|OwnedDocument::parse_with - caller supplies bytes or a Read; no ctxt reuse for DOMs
P|xmlRecoverDoc xmlRecoverMemory xmlRecoverFile|OwnedDocument::parse_with, options::RECOVER
I|xmlParseDTD|valid::Dtd::parse
P|xmlCtxtParseDtd xmlIOParseDTD|valid::Dtd::parse
I|xmlParseInNodeContext|Editor::parse_content, html::parse_fragment - UTF-8 input, not the document's encoding; the first error, where libxml2 returns its last
P|xmlCtxtParseContent|Editor::parse_content, html::parse_fragment - no text declaration (hasTextDecl); UTF-8 input
I|xmlParseBalancedChunkMemory|Editor::parse_content - on OwnedDocument::root with default options
P|xmlParseBalancedChunkMemoryRecover|Editor::parse_content - with RECOVER; the nodes come back without libxml2's error code
N|xmlParseCtxtExternalEntity xmlParseExternalEntity xmlParseExtParsedEnt xmlParseEntity|external parsed entities as standalone input not started
N|xmlSAXParse* xmlSAXUserParse*|SAX2 handler table not started (roadmap 15)
I|xmlCreatePushParserCtxt|PushParser::new
I|xmlParseChunk|PushParser::feed, PushParser::finish
P|xmlCtxtReset xmlCtxtResetPush xmlClearParserCtxt|PushParser::reset_with, BufferedReader::reset_with, TextReader::reset_with - not for DOM builders or Reader
P|xmlNewParserCtxt xmlInitParserCtxt xmlCreateDocParserCtxt xmlCreateIOParserCtxt|ParseOptions::new - configuration value plus the parser type
N|xmlNewSAXParserCtxt|SAX2 handler table not started
A|xmlFreeParserCtxt xmlFreeInputStream xmlFreeParserInputBuffer|Drop
P|xmlCtxtSetOptions xmlCtxtUseOptions xmlCtxtGetOptions|ParseOptions::new, ParseOptions::has
I|xmlCtxtSetResourceLoader|ParseOptions::with_resolver
P|xmlCtxtSetMaxAmplification|Limits::max_entity_amplification
A|xmlSetExternalEntityLoader xmlGetExternalEntityLoader xmlLoadExternalEntity|no global loader; ParseOptions::with_resolver per parse
N|xmlCtxtSetErrorHandler|no error callback; errors are returned (roadmap 6)
P|xmlCtxtSetCatalogs xmlCtxtGetCatalogs|catalog::CatalogResolver::new
A|xmlCtxtGetDict xmlCtxtSetDict xmlCtxtGetPrivate xmlCtxtSetPrivate xmlCtxtGetUserData|no dictionary or user pointer
P|xmlCtxtGetDeclaredEncoding xmlCtxtGetStandalone xmlCtxtGetVersion|Declaration
P|xmlCtxtGetDocTypeDecl|Doctype
P|xmlCtxtGetInputPosition xmlByteConsumed|PushParser::position
N|xmlCtxtGetDocument xmlCtxtGetNode xmlCtxtGetInputWindow xmlCtxtGetStatus xmlCtxtGetValidCtxt xmlCtxtIsHtml xmlCtxtIsInSubset xmlCtxtIsStopped|no inspectable parser context
N|xmlCtxtGetSaxHandler xmlCtxtSetSaxHandler|SAX2 handler table not started
P|xmlStopParser|PushParser::feed, html::parse_events - ControlFlow cancellation of the event callback
P|xmlCtxtValidateDocument xmlCtxtValidateDtd|valid::validate
I|xmlHasFeature|features::has
A|xmlInitParser xmlCleanupParser xmlInitGlobals xmlCleanupGlobals|no global state
A|xml*Default xml*DefaultValue __xml*DefaultValue __xmlIndentTreeOutput __xmlSaveNoEmptyTags __xmlTreeIndentString xmlIndentTreeOutput xmlSaveNoEmptyTags xmlTreeIndentString xmlThrDef*|global default; per-call ParseOptions or save::Options flags
A|xmlNewInputFrom* xmlNewIOInputStream xmlParserInputGrow xmlParserInputRead xmlParserInputDeallocate xmlSetupParserForBuffer xmlParserInputBufferCreate* xmlAllocParserInputBuffer|std::io::Read/BufRead or byte slices replace input streams
N|xmlInputSetEncodingHandler xmlCtxtSetCharEncConvImpl|no caller-provided encoding handler
N|xmlParserAddNodeInfo xmlParserFindNodeInfo xmlParserFindNodeInfoIndex xmlInitNodeInfoSeq xmlClearNodeInfoSeq xmlParserNodeInfo xmlParserNodeInfoSeq|no node-info table; Node::range gives byte ranges
A|xmlAttrHashBucket xmlParserNsData xmlStartTag xmlParserInputState xmlParserMode xmlParserInputFlags parser.h:XML_PARSER_* parser.h:XML_INPUT_* XML_SKIP_IDS XML_SAX2_MAGIC XML_DEFAULT_VERSION XML_TREE_INTERNALS|internal parser state or flag
N|XML_COMPLETE_ATTRS XML_DETECT_IDS|SAX2 loadsubset bits; SAX2 handler table not started
N|xmlParserStatus XML_STATUS_*|no xmlParserStatus
N|XML_RESOURCE_MAIN_DOCUMENT|ResourceKind has no main-document kind
A|XML_RESOURCE_UNKNOWN|no unknown resource kind
P|xmlParserOption|options
I|xmlResourceType|ResourceKind
P|xmlResourceLoader|Resolver
I|xmlFeature|features::Feature
P|xmlParserVersion|features::LIBXML2_COMPAT_VERSION
N|*SAXFunc *SAX2Func|SAX2 handler table not started (roadmap 15)
N|xmlDefaultSAXHandler xmlDefaultSAXLocator xmlSAXHandlerV1 xmlSAXHandlerV1Ptr xmlSAXHandler xmlSAXHandlerPtr xmlSAXLocator xmlSAXLocatorPtr|SAX2 handler table not started
N|xmlExternalEntityLoader|use Resolver
P|xmlParserCtxt|PushParser
A|xmlParserInput xmlParserCtxtPtr xmlParserInputPtr xmlParserNodeInfoPtr xmlParserNodeInfoSeqPtr|input stream or raw pointer alias
A|xmlParserInputBuffer xmlParserInputBufferPtr xmlOutputBuffer xmlOutputBufferPtr|std::io::Read/Write
# ---- parserInternals.h
I|IS_CHAR IS_CHAR_CH IS_BYTE_CHAR|chars::is_char
I|IS_BLANK IS_BLANK_CH|chars::is_blank
I|IS_BASECHAR|chars::is_base_char
I|IS_DIGIT IS_DIGIT_CH|chars::is_digit
I|IS_COMBINING IS_COMBINING_CH|chars::is_combining
I|IS_EXTENDER IS_EXTENDER_CH|chars::is_extender
I|IS_IDEOGRAPHIC|chars::is_ideographic
I|IS_PUBIDCHAR IS_PUBIDCHAR_CH|chars::is_pubid_char
P|IS_LETTER IS_LETTER_CH xmlIsLetter|chars::is_base_char, chars::is_ideographic - Letter is their union
A|IS_ASCII_DIGIT IS_ASCII_LETTER|u8::is_ascii_digit and u8::is_ascii_alphabetic
P|XML_MAX_DICTIONARY_LIMIT XML_MAX_LOOKUP_LIMIT|Limits
P|XML_MAX_HUGE_LENGTH XML_MAX_TEXT_LENGTH XML_MAX_NAME_LENGTH XML_MAX_NAMELEN|Limits::max_token_bytes
P|xmlParserMaxDepth|Limits::max_depth
A|XML_SUBSTITUTE_* inputPop inputPush xmlCtxtErrMemory xmlCtxtPopInput xmlCtxtPushInput xmlCreateFileParserCtxt xmlCreateMemoryParserCtxt xmlCreateURLParserCtxt xmlNewInputFromFile xmlNewInputStream xmlNewStringInputStream xmlStringText xmlStringTextNoenc|internal parser plumbing
N|xmlParseContent xmlSwitchEncoding xmlSwitchEncodingName xmlSwitchToEncoding|parser-internal entry point not exposed
P|xmlSplitQName|Name::new
# ---- tree.h
A|LIBXML2_NEW_BUFFER XML_BUFFER_ALLOC_* xmlBuf xmlBuf* xmlBuffer xmlBuffer* xmlChildrenNode xmlRootNode xmlDefaultBufferSize XML_GET_CONTENT XML_GET_LINE XML_LOCAL_NAMESPACE|Vec<u8>/String or field-alias macro
I|XML_XML_NAMESPACE|XML_NAMESPACE
A|XML_XML_ID|names are (namespace, local) pairs: xml:id is (XML_NAMESPACE, "id")
A|xmlGetDocCompressMode xmlSetDocCompressMode|no compression
I|xmlDoc|Document, OwnedDocument
I|xmlNode|Node, OwnedNode
P|xmlAttr|Attribute
P|xmlNs|NamespaceBinding
A|xmlNsType|namespace nodes carry no type tag
P|xmlDtd|valid::Dtd
I|xmlElementType|NodeKind, OwnedNodeKind
A|xmlDocPtr xmlNodePtr xmlAttrPtr xmlNsPtr xmlDtdPtr xmlElementPtr xmlAttributePtr xmlEntityPtr xmlNotationPtr xmlIDPtr xmlRefPtr xmlEnumerationPtr xmlElementContentPtr xmlBufferPtr xmlBufPtr xmlErrorPtr|raw pointer alias; references and OwnedNodeId
P|xmlEntity|valid::Entity - a borrowed view
P|xmlElement|valid::Element - a borrowed view; the content model is printed, not a tree
P|xmlAttribute|valid::Attribute - a borrowed view
P|xmlNotation|valid::Notation - a borrowed view
P|xmlEnumeration|valid::Attribute::values
N|xmlElementContent xmlElementContentOccur xmlElementContentType XML_ELEMENT_CONTENT_*|no content-model tree; valid::Element::model prints it
N|xmlID xmlRef|no public ID/IDREF tables
I|xmlAttributeType|valid::AttributeKind
I|XML_ATTRIBUTE_CDATA|valid::AttributeKind::Cdata
I|XML_ATTRIBUTE_ID|valid::AttributeKind::Id
I|XML_ATTRIBUTE_IDREF|valid::AttributeKind::IdRef
I|XML_ATTRIBUTE_IDREFS|valid::AttributeKind::IdRefs
I|XML_ATTRIBUTE_ENTITY|valid::AttributeKind::Entity
I|XML_ATTRIBUTE_ENTITIES|valid::AttributeKind::Entities
I|XML_ATTRIBUTE_NMTOKEN|valid::AttributeKind::NmToken
I|XML_ATTRIBUTE_NMTOKENS|valid::AttributeKind::NmTokens
I|XML_ATTRIBUTE_ENUMERATION|valid::AttributeKind::Enumeration
I|XML_ATTRIBUTE_NOTATION|valid::AttributeKind::Notation
I|xmlAttributeDefault|valid::AttributeDefault
I|XML_ATTRIBUTE_NONE|valid::AttributeDefault::Value
I|XML_ATTRIBUTE_REQUIRED|valid::AttributeDefault::Required
I|XML_ATTRIBUTE_IMPLIED|valid::AttributeDefault::Implied
I|XML_ATTRIBUTE_FIXED|valid::AttributeDefault::Fixed
I|xmlElementTypeVal|valid::ContentKind
I|XML_ELEMENT_TYPE_UNDEFINED|valid::ContentKind::Undefined
I|XML_ELEMENT_TYPE_EMPTY|valid::ContentKind::Empty
I|XML_ELEMENT_TYPE_ANY|valid::ContentKind::Any
I|XML_ELEMENT_TYPE_MIXED|valid::ContentKind::Mixed
I|XML_ELEMENT_TYPE_ELEMENT|valid::ContentKind::Element
N|XML_ATTRIBUTE_*|no public DTD declaration nodes
I|_xmlElement.name|valid::Element::name
I|_xmlElement.etype|valid::Element::kind
P|_xmlElement.content|valid::Element::model - printed as xmlDumpElementDecl writes it
I|_xmlElement.attributes|valid::Element::attributes
A|_xmlElement.prefix _xmlAttribute.prefix|names are qualified as declared
R|_xmlAttribute.tree _xmlElement.contModel|deprecated in 2.15.4
I|_xmlAttribute.elem|valid::Attribute::element
I|_xmlAttribute.name|valid::Attribute::name
I|_xmlAttribute.atype|valid::Attribute::kind
I|_xmlAttribute.def _xmlAttribute.defaultValue|valid::Attribute::default
I|_xmlNotation.name|valid::Notation::name
I|_xmlNotation.PublicID|valid::Notation::public_id
I|_xmlNotation.SystemID|valid::Notation::system_id
P|_xmlDtd.entities _xmlDtd.pentities|valid::Dtd::entities - an iterator over both subsets
P|_xmlDtd.elements|valid::Dtd::elements
P|_xmlDtd.attributes|valid::Element::attributes
P|_xmlDtd.notations|valid::Dtd::notations
P|XML_DOC_HTML|OwnedDocument::is_html
N|XML_DOC_*|no document property flags
N|xmlDocProperties|no document property flags
N|XML_ELEMENT_DECL XML_ENTITY_DECL XML_NAMESPACE_DECL|no public DTD declaration nodes
R|XML_DOCB_DOCUMENT_NODE xmlRegisterNodeFunc xmlDeregisterNodeFunc|DocBook and node registration callbacks removed upstream
A|XML_XINCLUDE_START XML_XINCLUDE_END|xinclude never adds marker nodes (NOXINCNODE behaviour)
I|xmlNewDoc|OwnedDocument::create
P|xmlNewDocNode xmlNewDocRawNode xmlNewNode xmlNewChild xmlNewTextChild|Editor::append_element - creation is fused with attachment
P|xmlNewText xmlNewTextLen xmlNewDocText xmlNewDocTextLen|Editor::append_text
P|xmlNewCDataBlock|Editor::append_cdata
P|xmlNewComment xmlNewDocComment|Editor::append_comment
P|xmlNewPI xmlNewDocPI|Editor::append_processing_instruction
I|xmlNewNs|Editor::declare_namespace
P|xmlNewProp xmlNewNsProp xmlNewDocProp|Editor::set_attribute
A|xmlNewNodeEatName xmlNewNsPropEatName xmlNewDocNodeEatName|no dictionary or name ownership transfer
N|xmlNewReference xmlNewCharRef xmlNewDocFragment xmlNewDtd|not started (no detached nodes, entity-reference or DTD construction)
P|xmlCreateIntSubset|Editor::add_declarations - created by the first declarations, named after the document element
I|xmlAddChild|Editor::append_child
I|xmlAddNextSibling|Editor::insert_after
I|xmlAddPrevSibling|Editor::insert_before
P|xmlAddSibling|Editor::append_child - on the node's parent (merges Text, as xmlAddSibling)
P|xmlAddChildList|Editor::append_child - once per node of the list
I|xmlUnlinkNode|Editor::unlink
I|xmlReplaceNode|Editor::replace
P|xmlFreeNode xmlFreeNodeList|Editor::remove_subtree
A|xmlFreeDoc xmlFreeDtd xmlFreeNs xmlFreeNsList xmlFreeProp xmlFreePropList|Drop
I|xmlSetProp xmlSetNsProp|Editor::set_attribute
I|xmlUnsetProp xmlUnsetNsProp|Editor::remove_attribute
P|xmlRemoveProp|Editor::remove_attribute
I|xmlGetProp|Node::attribute, OwnedNode::attribute
P|xmlGetNoNsProp|Node::attribute, OwnedNode::attribute - matches the qualified name
I|xmlGetNsProp|Node::attribute_ns, OwnedNode::attribute_ns
P|xmlHasProp xmlHasNsProp xmlNodeGetAttrValue|Node::attribute_ns, OwnedNode::attribute_ns - returns the value, not the attribute node
I|xmlDocGetRootElement|Document::document_element, OwnedDocument::document_element
P|xmlDocSetRootElement|Editor::append_child
I|xmlGetLastChild|Node::last_child, OwnedNode::last_child
P|xmlFirstElementChild xmlLastElementChild xmlNextElementSibling xmlPreviousElementSibling xmlChildElementCount|Node::children, OwnedNode::children - filter to elements
I|xmlNodeGetContent|Node::content, OwnedNode::content
P|xmlNodeBufGetContent xmlBufGetNodeContent|Node::content, OwnedNode::content
P|xmlNodeSetContent xmlNodeSetContentLen|Editor::set_node_value
I|xmlNodeAddContent xmlNodeAddContentLen xmlTextConcat|Editor::add_content
P|xmlTextMerge|Editor::add_content - then Editor::remove_subtree on the second node
N|xmlStringGetNodeList xmlStringLenGetNodeList|not started
I|xmlNodeListGetString|Node::list_string, OwnedNode::list_string
P|xmlNodeListGetRawString|OwnedNode::list_string - list_string(true) is its inLine 1; its inLine 0 escaping (xmlEncodeSpecialChars) is not offered
I|xmlNodeSetName|Editor::set_name
P|xmlSetNs|Editor::set_name
I|xmlNodeGetBase xmlNodeGetBaseSafe|Node::base_uri, OwnedNode::base_uri
I|xmlNodeGetLang|Node::lang, OwnedNode::lang
I|xmlNodeGetSpacePreserve|Node::space_preserve, OwnedNode::space_preserve
P|xmlNodeSetBase xmlNodeSetLang xmlNodeSetSpacePreserve|Editor::set_attribute - xml:base, xml:lang, xml:space in the XML namespace; Editor::set_url for the document
P|xmlNodeIsText|Node::kind, OwnedNode::kind
I|xmlIsXHTML|Doctype::is_xhtml
P|xmlIsBlankNode|OwnedNode::text - with chars::is_blank over a Text or CDATA value
A|xmlBuildQName|format! of prefix and local name
I|xmlGetLineNo|diagnostic::LineIndex - Node::line and OwnedNode::line
I|xmlGetNodePath|Node::path, OwnedNode::path
I|xmlGetNsList xmlGetNsListSafe|Node::namespaces, OwnedNode::namespaces
I|xmlSearchNs|Node::search_ns, OwnedNode::search_ns
I|xmlSearchNsByHref|Node::search_ns_by_href, OwnedNode::search_ns_by_href
A|xmlReconciliateNs|automatic namespace reconciliation on every owned edit
I|xmlCopyDoc|OwnedDocument::try_clone
I|xmlCopyNode|Editor::copy - deep, or extended 2 when shallow; extended 0 is not offered
I|xmlDocCopyNode|Editor::import - deep, or extended 2 when shallow
P|xmlCopyNodeList|Editor::copy - one node at a time
P|xmlDocCopyNodeList|Editor::import - one node at a time
P|xmlCopyDtd|OwnedDocument::try_clone - copies the DTD with the document; no standalone DTD copy
A|xmlCopyProp xmlCopyPropList|attributes are not nodes; copied with their element by Editor::import/copy
A|xmlCopyNamespace xmlCopyNamespaceList|declarations are attributes; copied with their element
P|xmlDOMWrapCloneNode|Editor::import - declarations on the copy root; no prefix renaming or oldNs (docs/owned-dom.md#copying-and-importing)
A|xmlDOMWrapAdoptNode|handles name one document: Editor::import + remove_subtree (docs/bindings.md#moving-nodes-between-documents)
A|xmlDOMWrapReconcileNamespaces|every edit and copy reconciles before publishing
A|xmlDOMWrapRemoveNode|Editor::unlink; namespaces need no fixup
A|xmlDOMWrapAcquireNsFunction|no custom namespace callback; copies never rename
A|xmlDOMWrap*|no DOM-wrapper context; copies plan namespaces themselves
A|xmlSetTreeDoc xmlSetListDoc|a node belongs to its arena; Editor::import copies
P|xmlGetIntSubset|Document::doctype, OwnedDocument::doctype
P|xmlDocDump xmlDocFormatDump xmlSaveFile xmlSaveFileEnc xmlSaveFileTo xmlSaveFormatFile xmlSaveFormatFileEnc xmlSaveFormatFileTo|save::to_vec - returns bytes; caller writes the file
I|xmlDocDumpMemory xmlDocDumpFormatMemory xmlDocDumpMemoryEnc xmlDocDumpFormatMemoryEnc|save::to_vec
P|xmlNodeDump xmlElemDump xmlNodeDumpOutput xmlAttrSerializeTxtContent|save::to_vec, save::Tree
P|xmlValidateNCName xmlValidateNMToken xmlValidateName xmlValidateQName|Name::new
A|xmlSplitQName2 xmlSplitQName3|str::split_once on ':'; Name views carry prefix and local parts
R|xmlIsMixedElement xmlIsRef|deprecated in 2.15.4
# ---- entities.h
I|xmlGetDocEntity|valid::Dtd::entity
I|xmlGetParameterEntity|valid::Dtd::parameter_entity
P|xmlGetDtdEntity|valid::Dtd::entities, valid::Entity::external_subset - filter; a name both subsets declare keeps only its internal declaration
I|xmlGetPredefinedEntity|valid::predefined_entity
I|xmlEncodeEntitiesReentrant|save::encode_entities
I|xmlEncodeSpecialChars|save::encode_special_chars
I|xmlEntityType|valid::EntityKind
I|XML_INTERNAL_GENERAL_ENTITY|valid::EntityKind::InternalGeneral
I|XML_EXTERNAL_GENERAL_PARSED_ENTITY|valid::EntityKind::ExternalParsedGeneral
I|XML_EXTERNAL_GENERAL_UNPARSED_ENTITY|valid::EntityKind::ExternalUnparsedGeneral
I|XML_INTERNAL_PARAMETER_ENTITY|valid::EntityKind::InternalParameter
I|XML_EXTERNAL_PARAMETER_ENTITY|valid::EntityKind::ExternalParameter
I|XML_INTERNAL_PREDEFINED_ENTITY|valid::EntityKind::Predefined
P|xmlEntitiesTable|valid::Dtd::entities - an iterator, no table type
A|xmlFreeEntity|Drop
P|xmlAddDocEntity xmlAddDtdEntity xmlAddEntity|Editor::add_declarations - declarations as markup, appended to the internal subset
N|xmlNewEntity|no detached entity declarations
I|_xmlEntity.name|valid::Entity::name
I|_xmlEntity.etype|valid::Entity::kind
P|_xmlEntity.content|valid::Entity::content - also the text of loaded external entities, which libxml2 leaves unset
I|_xmlEntity.ExternalID|valid::Entity::public_id
I|_xmlEntity.SystemID|valid::Entity::system_id
P|_xmlEntity.URI|valid::Entity::uri - resolved only when a resolver supplies the base
A|_xmlEntity.length|str::len of valid::Entity::content
N|_xmlEntity.orig|the literal as written is not kept
# ---- xmlerror.h
A|xmlLastError __xmlStructuredError*|no global error state
A|xmlGetLastError xmlResetLastError xmlResetError xmlCopyError xmlCtxtResetLastError xmlSetGenericErrorFunc xmlSetStructuredErrorFunc initGenericErrorDefaultFunc xmlGenericError* xmlStructuredError* xmlParserError xmlParserWarning xmlParserValidityError xmlParserValidityWarning xmlParserPrintFileInfo xmlParserPrintFileContext __xmlGenericError*|no global error state or printing; diagnostics are returned values
I|xmlFormatError|diagnostic::Diagnostic::render
P|xmlCtxtGetLastError|Document::errors, OwnedDocument::errors, diagnostic::LineIndex::describe_all
I|xmlError _xmlError|diagnostic::Diagnostic - built on demand by Error::describe
I|xmlErrorDomain|diagnostic::Domain - the domains yaxml raises
I|xmlErrorLevel|diagnostic::Level
I|xmlParserErrors|diagnostic::code - codes yaxml produces; ErrorKind::libxml2_code
# ---- xpath.h / xpathInternals.h
I|xmlXPathCompile|xpath::XPath::compile
P|xmlXPathCtxtCompile|xpath::XPath::compile_with_limits
I|xmlXPathCompiledEval|xpath::XPath::evaluate
P|xmlXPathCompiledEvalToBoolean xmlXPathEval xmlXPathEvalExpression xmlXPathNodeEval xmlXPathEvalPredicate xmlXPathEvaluatePredicateResult|xpath::XPath::evaluate
I|xmlXPathNewContext|xpath::Context::new
I|xmlXPathRegisterNs|xpath::Context::with_namespace
I|xmlXPathRegisterVariable|xpath::Context::with_variable
I|xmlXPathRegisterVariableNS|xpath::Context::with_variable_ns
I|xmlXPathRegisterFunc|xpath::Context::with_function
I|xmlXPathRegisterFuncNS|xpath::Context::with_function_ns
N|xmlXPathRegisterFuncLookup xmlXPathRegisterVariableLookup xmlXPathFunctionLookup xmlXPathFunctionLookupNS xmlXPathVariableLookup xmlXPathVariableLookupNS xmlXPathNsLookup xmlXPathRegisterAllFunctions xmlXPathSetErrorHandler xmlXPathContextSetCache xmlXPathSetContextNode|lookup callbacks not started (docs/xpath.md)
N|XML_XPATH_CHECKNS XML_XPATH_NOVAR|CHECKNS/NOVAR flags not implemented (docs/xpath.md)
A|xmlXPathFreeContext xmlXPathFreeCompExpr xmlXPathFreeNodeSet xmlXPathFreeNodeSetList xmlXPathFreeObject xmlXPathFreeParserContext xmlXPathRegisteredFuncsCleanup xmlXPathRegisteredNsCleanup xmlXPathRegisteredVariablesCleanup xmlXPathInit|Drop
I|xmlXPathObject xmlXPathObjectType|xpath::Value
I|xmlXPathCompExpr|xpath::XPath
P|xmlXPathContext|xpath::Context
A|xmlXPathContextPtr xmlXPathCompExprPtr xmlXPathObjectPtr xmlXPathParserContextPtr xmlNodeSetPtr xmlXPathAxisPtr xmlXPathFuncPtr xmlXPathTypePtr xmlXPathVariablePtr|raw pointer alias
A|xmlNodeSet xmlXPathParserContext xmlXPathAxis xmlXPathType xmlXPathVariable xmlXPathFunct xmlXPathAxisFunc xmlXPathConvertFunc xmlXPathEvalFunc xmlXPathFuncLookupFunc xmlXPathVariableLookupFunc|internal XPath machinery; Vec<xpath::Node> and closures
I|xmlXPathFunction|xpath::Function
I|XPATH_NODESET XPATH_BOOLEAN XPATH_NUMBER XPATH_STRING|xpath::Value
N|XPATH_UNDEFINED XPATH_POINT XPATH_RANGE XPATH_LOCATIONSET XPATH_USERS XPATH_XSLT_TREE|no such XPath value type (range extensions removed in 2.15)
A|XPATH_EXPRESSION_OK|success is Ok(..)
A|XPATH_MEMORY_ERROR|allocation failure aborts
N|XPATH_NUMBER_ERROR XPATH_INVALID_OPERAND XPATH_INVALID_CTXT_SIZE XPATH_INVALID_CTXT_POSITION XPATH_ENCODING_ERROR XPATH_STACK_ERROR XPATH_FORBID_VARIABLE_ERROR XPTR_SYNTAX_ERROR XPTR_RESOURCE_ERROR XPTR_SUB_RESOURCE_ERROR|no matching xpath::ErrorKind
P|xmlXPathError|xpath::ErrorKind
P|xmlXPathCast* xmlXPathConvert* xmlXPathStringEvalNumber|xpath::XPath::evaluate - conversions run inside evaluation
P|xmlXPath*Function|xpath::XPath::evaluate - core function library, not individually callable
P|xmlXPathNext*|xpath::XPath::evaluate - axes run inside evaluation
P|xmlXPathAddValues xmlXPathSubValues xmlXPathMultValues xmlXPathDivValues xmlXPathModValues xmlXPathCompareValues xmlXPathEqualValues xmlXPathNotEqualValues xmlXPathValueFlipSign|xpath::XPath::evaluate - operators run inside evaluation
A|xmlXPathIsInf xmlXPathIsNaN|f64::is_infinite, f64::is_nan
R|xmlXPathNAN xmlXPathPINF xmlXPathNINF|deprecated; use f64::NAN and f64::INFINITY
A|xmlXPathNodeSet* xmlXPathEmptyNodeSet xmlXPathNewNodeSet* xmlXPathNewBoolean xmlXPathNewCString xmlXPathNewFloat xmlXPathNewString xmlXPathNewValueTree xmlXPathWrap* xmlXPathReturn* xmlXPathObjectCopy xmlXPathDifference xmlXPathDistinct* xmlXPathHasSameNodes xmlXPathIntersection xmlXPathLeading* xmlXPathNodeLeading* xmlXPathNodeTrailing* xmlXPathTrailing* xmlXPathPop* xmlXPathValue* xmlXPathStackIs* xmlXPathNewParserContext xmlXPathParseNCName xmlXPathParseName xmlXPathRoot xmlXPathSetArityError xmlXPathSetError xmlXPathSetTypeError xmlXPathCheckError xmlXPathGetContextNode xmlXPathGetDocument xmlXPathGetError xmlXPathErr xmlXPatherror xmlXPathIsNodeType CAST_TO_* CHECK_* XP_ERROR XP_ERROR0 valuePop valuePush|XPath evaluator internals; xpath::Value, Vec and closures replace the object and stack API
I|xmlXPathCmpNodes|xpath::compare - namespace nodes ordered by their element
N|xmlXPathDebugDump*|not started
A|xmlXPathOrderDocElems|caches document order in node content for speed; XPath ranks nodes with its own order keys
# ---- xmlreader.h
I|xmlReaderForMemory xmlReaderForDoc xmlReaderForIO xmlReaderForFd xmlNewTextReader|TextReader::with_options, TextReader::new
P|xmlReaderForFile xmlNewTextReaderFilename|TextReader::new - caller opens the file
P|xmlReaderNewDoc xmlReaderNewFd xmlReaderNewFile xmlReaderNewIO xmlReaderNewMemory xmlTextReaderSetup|TextReader::reset_with
I|xmlReaderWalker xmlReaderNewWalker|Walker::new, Walker::reset - over an OwnedDocument (docs/textreader.md#walker)
A|xmlFreeTextReader xmlTextReaderClose|Drop
I|xmlTextReader|TextReader
A|xmlTextReaderPtr xmlTextReaderLocatorPtr|raw pointer alias
I|xmlTextReaderRead|TextReader::read
I|xmlTextReaderNext|TextReader::next
I|xmlTextReaderAttributeCount|TextReader::attribute_count
I|xmlTextReaderBaseUri xmlTextReaderConstBaseUri|TextReader::base_uri
I|xmlTextReaderConstLocalName xmlTextReaderLocalName|TextReader::local_name
I|xmlTextReaderConstName xmlTextReaderName|TextReader::name
I|xmlTextReaderConstNamespaceUri xmlTextReaderNamespaceUri|TextReader::namespace_uri
I|xmlTextReaderConstPrefix xmlTextReaderPrefix|TextReader::prefix
I|xmlTextReaderConstValue xmlTextReaderValue|TextReader::value
I|xmlTextReaderConstXmlLang xmlTextReaderXmlLang|TextReader::xml_lang
I|xmlTextReaderDepth|TextReader::depth
I|xmlTextReaderExpand|TextReader::expand
I|xmlTextReaderGetAttribute|TextReader::get_attribute
I|xmlTextReaderGetAttributeNo|TextReader::get_attribute_no
I|xmlTextReaderGetAttributeNs|TextReader::get_attribute_ns
I|xmlTextReaderGetParserProp|TextReader::parser_property
P|xmlTextReaderSetParserProp|TextReader::set_parser_property - only before the first read (docs/textreader.md)
I|xmlTextReaderReadAttributeValue|TextReader::read_attribute_value
I|xmlTextReaderIsDefault|TextReader::is_default - always false, as libxml2's
I|xmlTextReaderNormalization|TextReader::normalization
I|xmlTextReaderStandalone|TextReader::standalone
I|xmlTextReaderByteConsumed|TextReader::byte_consumed - exact bytes taken, libxml2 counts 4 KiB chunks
I|xmlTextReaderGetRemainder|TextReader::into_remainder
P|xmlTextReaderCurrentDoc xmlTextReaderCurrentNode|Walker::document, Walker::current_node - walker only; the parsing reader keeps no tree (docs/textreader.md)
N|xmlTextReaderPreserve xmlTextReaderPreservePattern|live tree pointers and free-behind; rejected, TextReader::expand copies a subtree (docs/textreader.md)
I|xmlTextReaderHasAttributes|TextReader::has_attributes
I|xmlTextReaderHasValue|TextReader::has_value
I|xmlTextReaderIsEmptyElement|TextReader::is_empty_element
I|xmlTextReaderIsValid|TextReader::is_valid
I|xmlTextReaderLookupNamespace|TextReader::lookup_namespace
I|xmlTextReaderMoveToAttribute|TextReader::move_to_attribute
I|xmlTextReaderMoveToAttributeNo|TextReader::move_to_attribute_no
I|xmlTextReaderMoveToAttributeNs|TextReader::move_to_attribute_ns
I|xmlTextReaderMoveToElement|TextReader::move_to_element
I|xmlTextReaderMoveToFirstAttribute|TextReader::move_to_first_attribute
I|xmlTextReaderMoveToNextAttribute|TextReader::move_to_next_attribute
I|xmlTextReaderNodeType|TextReader::node_type
I|xmlTextReaderReadInnerXml|TextReader::read_inner_xml
I|xmlTextReaderReadOuterXml|TextReader::read_outer_xml
I|xmlTextReaderReadState|TextReader::read_state
I|xmlTextReaderReadString|TextReader::read_string
I|xmlTextReaderRelaxNGSetSchema|TextReader::relaxng_set_schema
P|xmlTextReaderRelaxNGValidate xmlTextReaderRelaxNGValidateCtxt|TextReader::relaxng_set_schema - schema is compiled by the caller
I|xmlTextReaderSetSchema|TextReader::xsd_set_schema
P|xmlTextReaderSchemaValidate xmlTextReaderSchemaValidateCtxt|TextReader::xsd_set_schema - schema is compiled by the caller
P|xmlTextReaderSetResourceLoader|ParseOptions::with_resolver
P|xmlTextReaderSetMaxAmplification|Limits::max_entity_amplification
I|xmlTextReaderGetParserLineNumber|TextReader::line_number - takes the caller's LineIndex
I|xmlTextReaderGetParserColumnNumber|TextReader::column_number - takes the caller's LineIndex
I|xmlTextReaderConstEncoding|TextReader::encoding
I|xmlTextReaderConstXmlVersion|TextReader::xml_version
I|xmlTextReaderIsNamespaceDecl|TextReader::is_namespace_decl - false without a node (libxml2 -1)
I|xmlTextReaderQuoteChar|TextReader::quote_char
A|xmlTextReaderConstString|strings are borrowed &str; no reader dictionary
I|xmlTextReaderNextSibling|Walker::next_sibling - walker only, as in libxml2 (-1 for parsing readers)
N|xmlTextReaderLocatorBaseURI xmlTextReaderLocatorLineNumber|no error callback; locators belong to xmlTextReaderSetErrorHandler
N|xmlTextReaderErrorFunc xmlTextReaderGetErrorHandler xmlTextReaderSetErrorHandler xmlTextReaderSetStructuredErrorHandler xmlTextReaderGetLastError|no error callback; xsd_diagnostics/relaxng_diagnostics return values
I|xmlReaderTypes|NodeType
I|xmlTextReaderMode|ReadState
I|xmlParserProperties|ParserProperty
N|xmlParserSeverities XML_PARSER_SEVERITY_*|no severity levels
N|XML_TEXTREADER_MODE_CLOSED XML_TEXTREADER_MODE_READING|ReadState has no such variant
# ---- xmlsave.h
I|xmlSaveDoc xmlSaveTree|save::to_vec
P|xmlSaveToBuffer xmlSaveToFd xmlSaveToFilename xmlSaveToIO xmlSaveClose xmlSaveFinish xmlSaveFlush xmlSaveSetIndentString|save::to_vec, save::Options - returns a Vec; no Write sink yet
I|xmlSaveOption|save::Options
P|xmlSaveCtxt|save::Options
A|xmlSaveCtxtPtr|raw pointer alias
# ---- xmlwriter.h
P|xmlNewTextWriter xmlNewTextWriterFilename xmlNewTextWriterMemory|TextWriter::new - any Write sink; the caller opens files
P|xmlNewTextWriterDoc xmlNewTextWriterPushParser|TextWriter::new - write into a Vec, then OwnedDocument::parse_with(ParseOptions::new(0)) (docs/writer.md)
P|xmlNewTextWriterTree|Editor::parse_content - then append_child; prefixes resolve in context (docs/writer.md)
I|xmlTextWriter|TextWriter
A|xmlTextWriterPtr xmlFreeTextWriter|Drop
A|xmlTextWriterWriteFormat* xmlTextWriterWriteVFormat*|format!/write! replace printf-style formatting
I|xmlTextWriterStart* xmlTextWriterEnd* xmlTextWriterFullEndElement xmlTextWriterWrite*|TextWriter::start_element - one method per call, named in snake_case (xmlTextWriterWriteDTDElement is TextWriter::write_dtd_element)
I|xmlTextWriterWriteRawLen|TextWriter::write_raw
I|xmlTextWriterFlush xmlTextWriterSetIndent xmlTextWriterSetIndentString xmlTextWriterSetQuoteChar|TextWriter::flush, TextWriter::set_indent, TextWriter::set_indent_string, TextWriter::set_quote_char
P|xmlTextWriterClose|TextWriter::into_inner - end_document, then drop or take the sink
# ---- valid.h
P|xmlValidateDocument xmlValidateDtd xmlValidateElement|valid::validate, Document::validate, OwnedDocument::validate
P|xmlGetID|Document::element_by_id, OwnedDocument::element_by_id
A|xmlNewValidCtxt xmlFreeValidCtxt xmlFreeIDTable xmlFreeEnumeration|Drop
P|xmlValidatePushElement xmlValidatePushCData xmlValidatePopElement|valid::StreamValidator::event - deprecated in 2.15.4; parser events instead of nodes
P|xmlValidateDocumentFinal|valid::StreamValidator::finish - deprecated in 2.15.4; IDREFs resolve when the document element ends
P|xmlValidCtxt|valid::StreamValidator - one document's push validation state; tree validation takes none
A|xmlValidCtxtPtr|raw pointer alias
I|xmlGetDtdElementDesc|valid::Dtd::element
P|xmlGetDtdQElementDesc|valid::Dtd::element - takes the qualified name
I|xmlGetDtdAttrDesc|valid::Dtd::attribute
P|xmlGetDtdQAttrDesc|valid::Dtd::attribute - takes qualified names
I|xmlGetDtdNotationDesc|valid::Dtd::notation
P|xmlElementTable|valid::Dtd::elements - an iterator, no table type
P|xmlAttributeTable|valid::Element::attributes - an iterator, no table type
P|xmlNotationTable|valid::Dtd::notations - an iterator, no table type
P|xmlAddAttributeDecl xmlAddElementDecl xmlAddNotationDecl|Editor::add_declarations - declarations as markup, appended to the internal subset
I|xmlIsID|OwnedDocument::is_id
P|xmlAddID xmlAddIDSafe xmlRemoveID|Editor::set_attribute, Editor::remove_attribute - the ID table follows ID-typed attributes; no IDs by hand
N|xmlValidGetPotentialChildren xmlValidGetValidElements xmlDumpNotationTable xmlValidState xmlValidStatePtr|no public declaration model (docs/compatibility.md DTD rows)
N|xmlIDTable xmlIDTablePtr xmlRefTable xmlRefTablePtr xmlValidityErrorFunc xmlValidityWarningFunc|no public declaration tables
# ---- encoding.h
P|XML_CHAR_ENCODING_UTF8 XML_CHAR_ENCODING_UTF16 XML_CHAR_ENCODING_UTF16LE XML_CHAR_ENCODING_UTF16BE XML_CHAR_ENCODING_ASCII XML_CHAR_ENCODING_WINDOWS_1252|OwnedDocument::parse_bytes - decoded by label; no public encoding enum
P|XML_CHAR_ENCODING_UCS4BE XML_CHAR_ENCODING_UCS4LE XML_CHAR_ENCODING_EBCDIC|OwnedDocument::parse_bytes - feature encodings; detected by signature, no public encoding enum
N|XML_CHAR_ENCODING_UCS4_2143 XML_CHAR_ENCODING_UCS4_3412 XML_CHAR_ENCODING_UCS2 XML_CHAR_ENCODING_2022_JP XML_CHAR_ENCODING_SHIFT_JIS XML_CHAR_ENCODING_EUC_JP XML_CHAR_ENCODING_HTML XML_CHAR_ENCODING_NONE XML_CHAR_ENCODING_ERROR|encoding unsupported (docs/encoding.md)
N|xmlCharEnc* xmlCharEncoding* xmlCreateCharEncodingHandler xmlDetectCharEncoding xmlFindCharEncodingHandler xmlGetCharEncodingHandler xmlGetCharEncodingName xmlLookupCharEncodingHandler xmlNewCharEncodingHandler xmlOpenCharEncodingHandler xmlParseCharEncoding xmlRegisterCharEncodingHandler xmlIsolat1ToUTF8 xmlUTF8ToIsolat1 UTF8Toisolat1 isolat1ToUTF8 XML_ENC_*|no public encoding handlers or registry (roadmap 22)
# ---- HTMLparser.h / HTMLtree.h
I|htmlReadDoc htmlReadMemory htmlParseDoc|html::parse
P|htmlReadFd htmlReadFile htmlReadIO htmlParseFile htmlParseDocument htmlCtxtParseDocument htmlCtxtReadDoc htmlCtxtReadFd htmlCtxtReadFile htmlCtxtReadIO htmlCtxtReadMemory|html::parse - bytes in, no ctxt reuse
I|htmlCreatePushParserCtxt|html::PushParser::new
I|htmlParseChunk|html::PushParser::feed, html::PushParser::finish
P|htmlCreateFileParserCtxt htmlCreateMemoryParserCtxt htmlNewParserCtxt htmlCtxtSetOptions htmlCtxtUseOptions|html::Options::new
P|htmlNewSAXParserCtxt|html::parse_events, html::EventParser::new - a closure receives each callback as an html::Event
N|htmlCtxtReset|ctxt reuse not started (docs/html.md)
A|htmlFreeParserCtxt|Drop
P|htmlParserCtxt|html::PushParser
A|htmlDocPtr htmlNodePtr htmlParserCtxtPtr htmlParserInputPtr htmlSAXHandlerPtr htmlElemDescPtr htmlEntityDescPtr|raw pointer alias
P|htmlSAXHandler|html::Event - the callbacks HTML parsing makes, as events
I|XML_HTML_STRUCURE_ERROR|diagnostic::code::HTML_STRUCTURE_ERROR, html::DiagnosticKind::MisplacedTag
N|htmlParserInput htmlParserNodeInfo htmlElemDesc htmlEntityDesc|node info and element/entity tables not public (docs/html.md)
I|htmlDocDumpMemory htmlDocDumpMemoryFormat|html::to_html, html::save
P|htmlDocDump htmlSaveFile htmlSaveFileEnc htmlSaveFileFormat|html::save - returns bytes
I|htmlNodeDump|html::dump_node - format true
P|htmlNodeDumpFormatOutput htmlNodeDumpOutput htmlDocContentDumpOutput htmlDocContentDumpFormatOutput|html::dump_node - returns UTF-8 bytes (a converting output buffer: save with save::AS_HTML)
P|htmlNodeDumpFile htmlNodeDumpFileFormat|html::dump_node - returns bytes
N|htmlSetMetaEncoding htmlNewDoc htmlNewDocNoDtD|HTML construction not started (docs/html.md)
P|htmlGetMetaEncoding|OwnedDocument::html_encoding
A|HTML_COMMENT_NODE HTML_ENTITY_REF_NODE HTML_PI_NODE HTML_PRESERVE_NODE HTML_TEXT_NODE|node-type aliases
N|HTML_DEPRECATED HTML_INVALID HTML_NA HTML_REQUIRED HTML_VALID htmlStatus htmlDefaultSubelement htmlElementAllowedHereDesc htmlRequiredAttrs UTF8ToHtml|no public HTML element table
P|htmlParserOption|html::Options
# ---- uri.h
I|xmlParseURI xmlParseURISafe xmlParseURIReference|Uri::parse
I|xmlParseURIRaw|Uri::parse_raw
I|xmlBuildURI xmlBuildURISafe|uri::resolve
I|xmlBuildRelativeURI xmlBuildRelativeURISafe|uri::relative
I|xmlCanonicPath|uri::canonic_path
I|xmlValidateNameValue|valid::value_matches - AttributeKind::Id
I|xmlValidateNamesValue|valid::value_matches - AttributeKind::IdRefs
I|xmlValidateNmtokenValue|valid::value_matches - AttributeKind::NmToken
I|xmlValidateNmtokensValue|valid::value_matches - AttributeKind::NmTokens
I|xmlPathToURI|uri::canonic_path - xmlPathToURI is xmlCanonicPath
I|xmlNormalizeURIPath|uri::normalize_path
P|xmlURIEscape|uri::escape
I|xmlURIEscapeStr|uri::escape
I|xmlURIUnescapeString|uri::unescape
P|xmlSaveUri xmlPrintURI|Uri - fmt::Display
P|xmlCreateURI|Uri
A|xmlFreeURI xmlURIPtr|Drop
I|xmlURI|Uri
# ---- xmlIO.h
A|xmlAllocOutputBuffer xmlOutputBuffer* xmlParserInputBuffer* xmlRegister*Callbacks xmlCleanup*Callbacks xmlPop*Callbacks xmlInput*Callback xmlOutput*Callback xmlParserGetDirectory xmlCheckFilename __xmlOutputBuffer* __xmlParserInputBuffer*|std::io::Read/Write replace I/O buffers; Resolver replaces global callbacks
# ---- catalog.h
I|xmlCatalog|catalog::Catalog
I|xmlLoadACatalog xmlLoadCatalog|catalog::Catalog::load
P|xmlNewCatalog xmlParseCatalogFile|catalog::Catalog::parse
I|xmlACatalogResolve xmlACatalogResolvePublic xmlACatalogResolveSystem xmlCatalogResolve xmlCatalogResolvePublic xmlCatalogResolveSystem|catalog::Catalog::resolve
I|xmlACatalogResolveURI xmlCatalogResolveURI|catalog::Catalog::resolve_uri
P|xmlCatalogLocalResolve xmlCatalogLocalResolveURI|catalog::CatalogResolver::new
I|xmlACatalogAdd|catalog::Catalog::add
I|xmlACatalogRemove|catalog::Catalog::remove
I|xmlACatalogDump|catalog::Catalog::dump
P|xmlCatalogAddLocal|catalog::CatalogResolver::catalog_pi - the resolver of a CATALOG_PI parse keeps the list
A|xmlCatalogFreeLocal|the resolver owns the list and drops it with the parse
A|xmlCatalogAdd xmlCatalogRemove xmlCatalogDump xmlCatalogDumpDoc xmlLoadCatalogs xmlCatalogGetPublic xmlCatalogGetSystem|operate on the process-wide default catalog; Catalog::add/remove/dump edit a catalog value (docs/catalog.md)
A|xmlCatalogConvert xmlConvertSGMLCatalog|declined: 2.15.4 passes the address of its argument as the catalog, and converting through the right one moves nothing (docs/catalog.md)
N|xmlLoadSGMLSuperCatalog|not started (docs/catalog.md)
I|xmlCatalogIsEmpty|catalog::Catalog::is_empty - not lazy (docs/catalog.md)
A|xmlCatalogCleanup xmlInitializeCatalog xmlCatalogSetDefaults xmlCatalogGetDefaults xmlCatalogSetDefaultPrefer xmlCatalogSetDebug xmlCatalogAllow xmlCatalogPrefer XML_CATA_ALLOW_* XML_CATA_PREFER_NONE xmlFreeCatalog|no global catalog state
P|XML_CATA_PREFER_PUBLIC XML_CATA_PREFER_SYSTEM|catalog::Catalog::parse - read from the catalog file's prefer attribute
A|XML_CATALOGS_NAMESPACE XML_CATALOG_PI|private constants of catalog::Catalog and the parser
A|xmlCatalogPtr|raw pointer alias
# ---- xmlschemas.h
I|xmlSchemaParse|xsd::Schema::parse
P|xmlSchemaNewParserCtxt xmlSchemaNewMemParserCtxt xmlSchemaNewDocParserCtxt xmlSchemaSetResourceLoader|xsd::Schema::with_resolver
I|xmlSchemaValidateDoc|xsd::Schema::validate
I|xmlSchemaValidateStream|xsd::Schema::validate_stream
P|xmlSchemaValidateFile|xsd::Schema::validate_stream - caller opens the file
I|xmlSchemaIsValid|xsd::is_valid
I|xmlSchema|xsd::Schema
P|xmlSchemaNewValidCtxt xmlSchemaValidCtxt|xsd::StreamValidator::new
A|xmlSchemaFree xmlSchemaFreeParserCtxt xmlSchemaFreeValidCtxt xmlSchemaPtr xmlSchemaParserCtxt xmlSchemaParserCtxtPtr xmlSchemaValidCtxtPtr|Drop or raw pointer alias
A|xmlSchemaSet*Errors xmlSchemaSet*StructuredErrors xmlSchemaGet*Errors xmlSchemaValidity*Func|diagnostics are returned as Vec<xsd::Diagnostic>
N|xmlSchemaDump xmlSchemaSAXPlug xmlSchemaSAXPlugPtr xmlSchemaSAXPlugStruct xmlSchemaSAXUnplug xmlSchemaSetValidOptions xmlSchemaValidCtxtGetOptions xmlSchemaValidCtxtGetParserCtxt xmlSchemaValidateOneElement xmlSchemaValidateSetFilename xmlSchemaValidateSetLocator xmlSchemaValidOption XML_SCHEMA_VAL_VC_I_CREATE xmlSchemaValidError XML_SCHEMAS_ERR_*|not started (docs/xsd.md)
# ---- schemasInternals.h
N|xmlSchemaFreeType xmlSchemaFreeWildcard xmlSchemaAnnot xmlSchemaAnnotPtr xmlSchemaAttribute xmlSchemaAttributeGroup xmlSchemaAttributeGroupPtr xmlSchemaAttributeLink xmlSchemaAttributeLinkPtr xmlSchemaAttributePtr xmlSchemaContentType xmlSchemaElement xmlSchemaElementPtr xmlSchemaFacet xmlSchemaFacetLink xmlSchemaFacetLinkPtr xmlSchemaFacetPtr xmlSchemaNotation xmlSchemaNotationPtr xmlSchemaType xmlSchemaTypeLink xmlSchemaTypeLinkPtr xmlSchemaTypePtr xmlSchemaTypeType xmlSchemaVal xmlSchemaValPtr xmlSchemaWildcard xmlSchemaWildcardNs xmlSchemaWildcardNsPtr xmlSchemaWildcardPtr XML_SCHEMAS_* XML_SCHEMA_CONTENT_* XML_SCHEMA_TYPE_* XML_SCHEMA_EXTRA_*|no public schema component model; xsd::Schema is opaque
I|xmlSchemaValType|schema_types::Type
# ---- xmlschemastypes.h
I|xmlSchemaValidatePredefinedType|schema_types::Type::parse
P|xmlSchemaValPredefTypeNode xmlSchemaValPredefTypeNodeNoNorm|schema_types::Type::parse
I|xmlSchemaCompareValues xmlSchemaCompareValuesWhtsp|schema_types::Value::compare
I|xmlSchemaGetPredefinedType xmlSchemaGetBuiltInType|schema_types::Type::from_name
I|xmlSchemaGetBuiltInListSimpleTypeItemType|schema_types::Type::item
I|xmlSchemaGetValType|schema_types::Value::ty
I|xmlSchemaNewFacet|schema_types::Facet::new
P|xmlSchemaCheckFacet|schema_types::Facet::new
I|xmlSchemaValidateFacet xmlSchemaValidateFacetWhtsp|schema_types::Facet::check
P|xmlSchemaValidateLengthFacet xmlSchemaValidateLengthFacetWhtsp xmlSchemaValidateListSimpleTypeFacet|schema_types::Facet::check
I|xmlSchemaCollapseString xmlSchemaWhiteSpaceReplace|schema_types::normalize
A|xmlSchemaCopyValue xmlSchemaFreeValue xmlSchemaFreeFacet xmlSchemaInitTypes xmlSchemaCleanupTypes|Clone/Drop; no global type table
N|xmlSchemaGetCanonValue xmlSchemaGetCanonValueWhtsp xmlSchemaGetFacetValueAsULong xmlSchemaIsBuiltInTypeFacet xmlSchemaNewNOTATIONValue xmlSchemaNewQNameValue xmlSchemaNewStringValue xmlSchemaValueAppend xmlSchemaValueGetAsBoolean xmlSchemaValueGetAsString xmlSchemaValueGetNext|not started (canonical values, list values; docs/schema-types.md)
I|xmlSchemaWhitespaceValueType|schema_types::WhiteSpace
I|XML_SCHEMA_WHITESPACE_PRESERVE|schema_types::WhiteSpace::Preserve
I|XML_SCHEMA_WHITESPACE_REPLACE|schema_types::WhiteSpace::Replace
I|XML_SCHEMA_WHITESPACE_COLLAPSE|schema_types::WhiteSpace::Collapse
A|XML_SCHEMA_WHITESPACE_UNKNOWN|no unknown whitespace mode
# ---- relaxng.h
I|xmlRelaxNGParse|relaxng::Schema::parse
P|xmlRelaxNGNewParserCtxt xmlRelaxNGNewMemParserCtxt xmlRelaxNGNewDocParserCtxt xmlRelaxNGSetResourceLoader|relaxng::Schema::parse, relaxng::Schema::from_tree
I|xmlRelaxNGValidateDoc|relaxng::Schema::validate
P|xmlRelaxNGValidatePushElement xmlRelaxNGValidatePopElement xmlRelaxNGValidatePushCData|relaxng::Stream::event
N|xmlRelaxNGValidateFullElement XML_RELAXNGP_CRNG xmlRelaxNGParserFlag xmlRelaxParserSetFlag|not started (docs/relaxng.md)
A|xmlRelaxNGDump xmlRelaxNGDumpTree|declined: debug printers of libxml2's define tree and of the schema document it rewrites in place; yaxml keeps neither (docs/relaxng.md)
P|xmlRelaxParserSetIncLImit|relaxng::Limits::max_include_depth
A|XML_RELAXNGP_FREE_DOC XML_RELAXNGP_NONE XML_RELAXNG_OK|no parser flags or success code
A|xmlRelaxNGNewValidCtxt xmlRelaxNGFree xmlRelaxNGFreeParserCtxt xmlRelaxNGFreeValidCtxt xmlRelaxNGPtr xmlRelaxNGParserCtxt xmlRelaxNGParserCtxtPtr xmlRelaxNGValidCtxt xmlRelaxNGValidCtxtPtr xmlRelaxNGInitTypes xmlRelaxNGCleanupTypes xmlRelaxNGValidCtxtClearErrors xmlRelaxNGSet*Errors xmlRelaxNGGet*Errors xmlRelaxNGValidity*Func|Drop, raw pointer alias, or diagnostics returned as Vec
I|xmlRelaxNG|relaxng::Schema
I|xmlRelaxNGValidErr|relaxng::Code
# ---- xmlregexp.h / automata
I|xmlRegexpCompile|regexp::Regex::new
I|xmlRegexpExec|regexp::Regex::is_match
I|xmlRegexp|regexp::Regex
A|xmlRegFreeRegexp xmlRegexpPtr|Drop
N|xmlRegexpIsDeterminist xmlRegExecCallbacks xmlRegExecCtxt xmlRegExecCtxtPtr|push execution contexts not started (docs/compatibility.md)
R|xmlAutomata* xmlFreeAutomata xmlNewAutomata|deprecated; LIBXML_AUTOMATA off in 2.15
# ---- xinclude.h
I|xmlXIncludeProcess xmlXIncludeProcessFlags xmlXIncludeProcessFlagsData|xinclude::process
P|xmlXIncludeProcessNode xmlXIncludeProcessTree xmlXIncludeProcessTreeFlags xmlXIncludeProcessTreeFlagsData|xinclude::process - whole documents only
P|xmlXIncludeNewContext xmlXIncludeSetFlags xmlXIncludeSetResourceLoader|xinclude::Options
A|xmlXIncludeFreeContext xmlXIncludeCtxt xmlXIncludeCtxtPtr xmlXIncludeSetErrorHandler xmlXIncludeGetLastError|Drop; errors returned by xinclude::process
I|XINCLUDE_NS|xinclude::NS
I|XINCLUDE_OLD_NS|xinclude::OLD_NS
A|XINCLUDE_*|element/attribute/value names are the spec's literals (include, fallback, href, parse, xml, text, encoding, xpointer)
# ---- pattern.h / c14n.h / schematron.h / xpointer.h
I|xmlPatterncompile xmlPatternCompileSafe|pattern::Pattern::compile
I|xmlPatternMatch|pattern::Pattern::matches
I|xmlPatternGetStreamCtxt|pattern::Pattern::stream
I|xmlStreamPush xmlStreamPushNode|pattern::StreamMatcher::push
I|xmlStreamPushAttr|pattern::StreamMatcher::push_attr
I|xmlStreamPop|pattern::StreamMatcher::pop
I|xmlStreamWantsAnyNode|pattern::StreamMatcher::wants_any_node
I|xmlPattern|pattern::Pattern
I|xmlStreamCtxt|pattern::StreamMatcher
A|xmlFreePattern xmlFreePatternList xmlFreeStreamCtxt xmlPatternPtr xmlStreamCtxtPtr|Drop or raw pointer alias
I|xmlPatternStreamable|pattern::Pattern::streamable
I|xmlPatternMinDepth|pattern::Pattern::min_depth
I|xmlPatternMaxDepth|pattern::Pattern::max_depth - None = not streamable, Some(None) = unbounded (-2)
I|xmlPatternFromRoot|pattern::Pattern::from_root
I|xmlPatternFlags|pattern
I|XML_PATTERN_XPATH|pattern::XPATH
I|XML_PATTERN_XSSEL|pattern::XS_SELECTOR
I|XML_PATTERN_XSFIELD|pattern::XS_FIELD
A|XML_PATTERN_DEFAULT|zero flags
I|xmlC14NDocDumpMemory|c14n::to_string
I|xmlC14NDocSaveTo|c14n::write
P|xmlC14NDocSave|c14n::write - caller opens the file
P|xmlC14NExecute|c14n::write, c14n::Options
P|xmlC14NIsVisibleCallback|c14n::Options - node subset instead of a callback
I|xmlC14NMode|c14n::Mode
I|XML_C14N_1_0|c14n::Mode::C14n10
I|XML_C14N_1_1|c14n::Mode::C14n11
I|XML_C14N_EXCLUSIVE_1_0|c14n::Mode::Exclusive10
I|xmlSchematronParse|schematron::Schema::parse
P|xmlSchematronNewParserCtxt xmlSchematronNewMemParserCtxt xmlSchematronNewDocParserCtxt|schematron::Schema::parse, schematron::Schema::from_tree
I|xmlSchematronValidateDoc|schematron::Schema::validate
I|xmlSchematron|schematron::Schema
I|xmlSchematronValidOptions|schematron
I|XML_SCHEMATRON_OUT_QUIET|schematron::OUT_QUIET
I|XML_SCHEMATRON_OUT_TEXT|schematron::OUT_TEXT
I|XML_SCHEMATRON_OUT_XML|schematron::OUT_XML
I|XML_SCHEMATRON_OUT_ERROR|schematron::OUT_ERROR
A|XML_SCHEMATRON_OUT_BUFFER XML_SCHEMATRON_OUT_FILE XML_SCHEMATRON_OUT_IO|output sinks; Output::Text returns a String
A|xmlSchematronFree xmlSchematronFreeParserCtxt xmlSchematronFreeValidCtxt xmlSchematronNewValidCtxt xmlSchematronParserCtxt xmlSchematronParserCtxtPtr xmlSchematronPtr xmlSchematronValidCtxt xmlSchematronValidCtxtPtr xmlSchematronSetValidStructuredErrors xmlSchematronValidity*Func|Drop or diagnostics returned
I|xmlXPtrEval|xpointer::evaluate
# ---- xmlversion.h.in / xmlexports.h
I|LIBXML_VERSION|LIBXML2_COMPAT_VERSION_NUMBER
I|LIBXML_DOTTED_VERSION|LIBXML2_COMPAT_VERSION
P|LIBXML_VERSION_STRING|LIBXML2_COMPAT_VERSION_NUMBER
P|LIBXML_TEST_VERSION|features::check_version
I|xmlCheckVersion|features::check_version
R|LIBXML_HTTP_STUBS_ENABLED|HTTP/FTP stubs removed upstream
A|LIBXML_VERSION_EXTRA LIBXML_MODULE_EXTENSION LIBXML_THREAD_ALLOC_ENABLED LIBXML_ATTR_ALLOC_SIZE LIBXML_ATTR_FORMAT LIBXML_DLL_IMPORT XMLCALL XMLCDECL XMLPUBFUN XMLPUBVAR XML_DEPRECATED XML_DEPRECATED_MEMBER|C build or ABI macro
P|LIBXML_*_ENABLED|features::Feature - runtime query rather than a build macro
# ---- SAX2.h xlink.h
N|xmlSAX2* xmlSAXVersion|SAX2 handler table not started (roadmap 15)
R|xlink* XLINK_* _xlinkHandler*|xlink.h is deprecated in 2.15.4 ("don't use"); the parser never calls its handlers
# ---- xmlstring.h chvalid.h dict.h hash.h list.h debugXML.h threads.h xmlmemory.h xmlmodule.h nanohttp.h
A|BAD_CAST xmlChar xmlStr* xmlCharStr* xmlCheckUTF8 xmlGetUTF8Char xmlUTF8*|str/String/char replace xmlChar helpers (Rust strings are valid UTF-8)
I|xmlIsChar xmlIsCharQ xmlIsChar_ch|chars::is_char
I|xmlIsBlank xmlIsBlankQ xmlIsBlank_ch|chars::is_blank
I|xmlIsBaseChar xmlIsBaseCharQ xmlIsBaseChar_ch|chars::is_base_char
I|xmlIsDigit xmlIsDigitQ xmlIsDigit_ch|chars::is_digit
I|xmlIsCombining xmlIsCombiningQ|chars::is_combining
I|xmlIsExtender xmlIsExtenderQ xmlIsExtender_ch|chars::is_extender
I|xmlIsIdeographic xmlIsIdeographicQ|chars::is_ideographic
I|xmlIsPubidChar xmlIsPubidCharQ xmlIsPubidChar_ch|chars::is_pubid_char
P|xmlIsPubidChar_tab|chars::is_pubid_char - a predicate, no table
A|xmlChLRange* xmlChRange* xmlChSRange* xmlCharInRange xmlIs*Group|the chars predicates keep their range tables private
A|xmlDict* xmlInitializeDict xmlHash* xmlLink* xmlList* XML_CAST_FPTR|no dictionary, hash or list API; HashMap/Vec/VecDeque replace them
I|xmlDebugDumpString|debug::dump_string
I|xmlDebugDumpDocumentHead|debug::dump_document_head
I|xmlDebugDumpNode|debug::dump_node
I|xmlDebugDumpOneNode|debug::dump_one_node
I|xmlDebugDumpNodeList|debug::dump_node_list
I|xmlDebugDumpAttr|debug::dump_attribute - attribute by position
I|xmlDebugDumpAttrList|debug::dump_attributes
P|xmlDebugDumpDocument|debug::dump_document - no NOENT entity-declaration children, CRLF and `<?pi ?>` divergences (feature `debug`)
P|xmlDebugDumpDTD|debug::dump_dtd - the internal subset re-read, as save reads it
P|xmlDebugDumpEntities|debug::dump_entities - declaration order, no `orig`
P|xmlDebugCheckDocument|debug::check_document - detached roots and non-Name names; the tree keeps libxml2's other checked invariants
A|xmlMem* xmlGcMem* xmlMalloc* xmlRealloc* xmlFree xmlFreeFunc xmlCleanupMemory xmlInitMemory xmlMemoryStrdup xmlMemoryDump xmlStrdupFunc __xmlFree __xmlMalloc __xmlMallocAtomic __xmlMemStrdup __xmlRealloc|global allocator and Drop; no allocation callbacks
A|xmlCheckThreadLocalStorage xmlCleanupThreads xmlFreeMutex xmlFreeRMutex xmlInitThreads xmlLockLibrary xmlMutex xmlMutexLock xmlMutexPtr xmlMutexUnlock xmlNewMutex xmlNewRMutex xmlRMutex xmlRMutexLock xmlRMutexPtr xmlRMutexUnlock xmlUnlockLibrary|no global state; types are Send
R|xmlModule* XML_MODULE_*|dynamic modules removed from default builds (Feature::Modules is false)
R|xmlNanoHTTP* xmlNanoFTP*|HTTP/FTP clients removed upstream
"""


def parse_table():
    exact, globs = {}, []
    for ln, line in enumerate(TABLE.strip().splitlines(), 1):
        if not line.strip() or line.startswith("#"):
            continue
        status, names, rust = line.split("|", 2)
        for n in names.split():
            hdr, _, pat = n.rpartition(":")
            if any(c in pat for c in "*?["):
                globs.append((hdr, pat, status, rust))
            elif (hdr, pat) in exact:
                sys.exit(f"TABLE: duplicate name {n} (line {ln})")
            else:
                exact[(hdr, pat)] = (status, rust)
    return exact, globs


EXACT, GLOBS = parse_table()


# --------------------------------------------------------------------------
# Classification

class Ctx:
    def __init__(self):
        src = source_dir() / "include" / "libxml"
        self.err = enum_values(src / "xmlerror.h", "XML_ERR_OK = 0", "} xmlParserErrors")
        self.rng = enum_values(src / "relaxng.h", "XML_RELAXNG_OK = 0", "} xmlRelaxNGValidErr")
        self.xsd = rust_codes("xsd.rs", "Code")
        self.valid = rust_codes("valid.rs", "Code")
        self.rngp = rust_codes("relaxng.rs", "SchemaCode")
        self.rngv = rust_codes("relaxng.rs", "Code")


def by_code(codes, value, path, kind):
    v = codes.get(value)
    return ("I", f"{path}::{v}") if v else ("N", f"no yaxml diagnostic carries this {kind} code")


# The codes, levels and domains Error::describe produces (src/diagnostic.rs).
DIAG_CODES = set(re.findall(r"pub const (\w+): u16", (SRC / "diagnostic.rs").read_text()))
DIAG_LEVEL = {"XML_ERR_WARNING": "Warning", "XML_ERR_ERROR": "Error", "XML_ERR_FATAL": "Fatal"}
DIAG_DOMAIN = {
    f"XML_FROM_{c}": rust
    for c, rust in [("PARSER", "Parser"), ("NAMESPACE", "Namespace"), ("HTML", "Html"), ("IO", "Io"), ("XINCLUDE", "XInclude"),
                    ("XPATH", "XPath"), ("SCHEMASP", "SchemasParser"), ("SCHEMASV", "SchemasValidity"),
                    ("RELAXNGV", "RelaxNgValidity"), ("VALID", "Valid"), ("SCHEMATRONV", "SchematronValidity")]
}


def DIAG_NAME(i):
    """The `diagnostic::code` constant name for a libxml2 error code."""
    return re.sub(r"^XML_(?:ERR_|(NS_)ERR_|)", lambda m: m.group(1) or "", i)


def error_rule(i, ctx):
    if re.fullmatch(r"XML_SCHEMA[PV]_\w+", i):
        return by_code(ctx.xsd, ctx.err[i], "xsd::Code", "XML_SCHEMA*")
    if i.startswith("XML_RNGP_") or i == "XML_ERR_RESOURCE_LIMIT":
        return by_code(ctx.rngp, ctx.err[i], "relaxng::SchemaCode", "XML_RNGP")
    if i.startswith("XML_DTD_") or i in ("XML_ERR_UNDECLARED_ENTITY", "XML_ERR_ENTITY_BOUNDARY"):
        return by_code(ctx.valid, ctx.err[i], "valid::Code", "XML_DTD")
    if i.startswith("XML_SCHEMATRONV_"):
        return "I", "schematron::Code::" + ("Assert" if i.endswith("ASSERT") else "Report")
    if m := re.fullmatch(r"XML_XPATH_(\w+)", i):
        n = m.group(1)
        if n in XPATH_ERR:
            return "P", f"xpath::ErrorKind::{XPATH_ERR[n]} - category only; no numeric code"
        return {"EXPRESSION_OK": ("A", "success is Ok(..)"), "MEMORY_ERROR": ("A", "allocation failure aborts")}.get(n, ("N", "no matching xpath::ErrorKind"))
    if m := re.fullmatch(r"XML_XINCLUDE_(\w+)", i):
        n = m.group(1)
        if n in XINCLUDE_ERR:
            return "P", f"xinclude::ErrorKind::{XINCLUDE_ERR[n]} - category only"
        return "N", "no matching xinclude::ErrorKind"
    xptr = {"XML_XPTR_CHILDSEQ_START": "ChildSequenceStart", "XML_XPTR_UNKNOWN_SCHEME": "UnknownScheme", "XML_XPTR_SYNTAX_ERROR": "Syntax"}
    if i in xptr:
        return "P", f"xpointer::Error::{xptr[i]} - category only"
    if i.startswith("XML_XPTR_"):
        return "N", "no matching xpointer::Error"
    if i == "XML_C14N_RELATIVE_NAMESPACE":
        return "P", "c14n::Error::RelativeNamespace - category only"
    if i.startswith("XML_C14N_"):
        return "N", "no matching c14n::Error"
    if i == "XML_REGEXP_COMPILE_ERROR":
        return "P", "regexp::Error::Syntax - category only"
    if (name := DIAG_NAME(i)) in DIAG_CODES:
        return "I", f"diagnostic::code::{name}"
    if i in DIAG_LEVEL:
        return "I", f"diagnostic::Level::{DIAG_LEVEL[i]}"
    if i in DIAG_DOMAIN:
        return "I", f"diagnostic::Domain::{DIAG_DOMAIN[i]}"
    if m := re.fullmatch(r"XML_NS_ERR_(\w+)", i):
        return "P", "ErrorKind::" + ("DuplicateAttribute" if m.group(1) == "ATTRIBUTE_REDEFINED" else "Namespace") + " - category only"
    if m := re.fullmatch(r"XML_ERR_(\w+)", i):
        n = m.group(1)
        if n == "OK":
            return "A", "success is Ok(..)"
        if n in ("NONE", "WARNING", "ERROR", "FATAL"):
            return "N", "no severity level in Error"
        if n in ("ARGUMENT", "SYSTEM"):
            return "A", "typed arguments; OS errors are std::io::Error"
        if n in PARSER_ERR:
            return "P", f"ErrorKind::{PARSER_ERR[n]} - category only; no numeric code"
        if PARSER_SYNTAX.fullmatch(n):
            return "P", "ErrorKind::Syntax - category only; no numeric code"
        return "N", "no matching ErrorKind"
    if i.startswith("XML_FROM_"):
        return "N", "no error domain; each module has its own error type"
    if i.startswith(("XML_MODULE_", "XML_FTP_", "XML_HTTP_")):
        return "R", "dynamic modules and HTTP/FTP removed upstream"
    if i.startswith("XML_IO_"):
        return "N", "no per-code I/O errors; std::io::Error and ErrorKind::ExternalResource"
    if i.startswith("XML_HTML_"):
        return "N", "html::DiagnosticKind has no matching variant"
    if i.startswith(("XML_CATALOG_", "XML_SAVE_", "XML_TREE_", "XML_BUF_", "XML_I18N_", "XML_CHECK_", "XML_WAR_")):
        return "N", "no per-code diagnostic"
    return None


def header_rule(h, i, k, ctx):
    if h == "xmlerror.h":
        return error_rule(i, ctx)
    if m := re.fullmatch(r"XML_PARSE_(\w+)", i):
        n = m.group(1)
        if n in OPT_ALL:
            s, note = OPT.get(n, ("I", ""))
            return s, f"options::{n}" + (f" - {note}" if note else "")
        return "A", "xmlParserMode value; internal"
    if m := re.fullmatch(r"HTML_PARSE_(\w+)", i):
        s, note = HTML_OPT.get(m.group(1), ("I", ""))
        return s, f"html::{m.group(1)}" + (f" - {note}" if note else "")
    if m := re.fullmatch(r"XML_WITH_(\w+)", i):
        return ("I", f"features::Feature::{FEATURE[m.group(1)]}") if m.group(1) in FEATURE else ("A", "sentinel value")
    if h == "xmlsave.h" and (m := re.fullmatch(r"XML_SAVE_(\w+)", i)):
        return "I", f"save::{m.group(1)}"
    if m := re.fullmatch(r"XML_(\w+)_NODE", i):
        if m.group(1) in TREE_KIND:
            return TREE_KIND[m.group(1)]
    if m := re.fullmatch(r"XML_READER_TYPE_(\w+)", i):
        if m.group(1) in READER_TYPE:
            note = " - never reported, as by libxml2's reader" if m.group(1) in READER_TYPE_UNUSED else ""
            return "I", f"NodeType::{READER_TYPE[m.group(1)]}{note}"
    if m := re.fullmatch(r"XML_TEXTREADER_MODE_(\w+)", i):
        if m.group(1) in READER_MODE:
            return "I", f"ReadState::{READER_MODE[m.group(1)]}"
    if h == "xmlreader.h" and (m := re.fullmatch(r"XML_PARSER_(\w+)", i)):
        if m.group(1) in READER_PROP:
            return "I", f"ParserProperty::{READER_PROP[m.group(1)]}"
    if m := re.fullmatch(r"XML_RESOURCE_(\w+)", i):
        if m.group(1) in RESOURCE:
            return "I", f"ResourceKind::{RESOURCE[m.group(1)]}"
    if m := re.fullmatch(r"XML_CHAR_ENCODING_(\w+)", i):
        if m.group(1) in ENC:
            return "P", "OwnedDocument::parse_bytes - decoded by label; no public encoding enum"
    if h == "schemasInternals.h" and k == "enum_value" and (m := re.fullmatch(r"XML_SCHEMAS_(\w+)", i)):
        if m.group(1) in SCHEMA_TYPES:
            return "I", f"schema_types::Type::{SCHEMA_TYPES[m.group(1)]}"
    if m := re.fullmatch(r"XML_SCHEMA_FACET_(\w+)", i):
        return "I", f"schema_types::Facet::{FACETS[m.group(1)]}"
    if m := re.fullmatch(r"XML_RELAXNG_ERR_(\w+)", i):
        return by_code(ctx.rngv, ctx.rng[i], "relaxng::Code", "XML_RELAXNG_ERR")
    if m := re.fullmatch(r"XPATH_(\w+)", i):
        if m.group(1) in XPATH_ERR:
            return "P", f"xpath::ErrorKind::{XPATH_ERR[m.group(1)]} - category only; no numeric code"
    return None


def lookup(h, i):
    if (h, i) in EXACT:
        return EXACT[(h, i)]
    return EXACT.get(("", i))


def glob_lookup(h, i):
    for hdr, pat, s, rust in GLOBS:
        if hdr in ("", h) and fnmatch.fnmatchcase(i, pat):
            return s, rust
    return None


def classify(row, ctx):
    h, i, k = row["header"], row["identifier"], row["kind"]
    if k == "empty_header":
        return "A", "empty compatibility header; no declarations"
    d = row["deprecated_marker"] == "yes"
    for r in (lookup(h, i), header_rule(h, i, k, ctx), TREE_FIELDS.get(i), glob_lookup(h, i)):
        if r:
            return r
    if k == "struct" and i.startswith("_"):
        return classify({**row, "identifier": i[1:], "kind": "typedef"}, ctx)
    if k == "typedef" and i.endswith("Ptr"):
        return "A", "raw pointer alias"
    if k == "field" and (r := FIELD_DEFAULT.get(i.split(".")[0])):
        return ("R", "deprecated in 2.15.4") if d and r[0] == "N" else r
    if d:
        return "R", "deprecated in 2.15.4"
    return None


def generate():
    ctx = Ctx()
    with TSV.open(newline="") as f:
        rd = csv.DictReader(f, delimiter="\t")
        fields = [c for c in rd.fieldnames if c != "rust"] + ["rust"]
        rows = list(rd)
    left = defaultdict(list)
    for r in rows:
        res = classify(r, ctx)
        if res is None:
            left[r["header"]].append(r["identifier"])
            res = ("N", "no yaxml equivalent")
        r["status"], r["rust"] = STATUS[res[0]], res[1]
    for h, ids in sorted(left.items()):
        print(f"fallback {h}: {len(ids)}: {' '.join(ids)}", file=sys.stderr)
    errors = check_rows(rows)
    if errors:
        print("\n".join(errors[:100]), file=sys.stderr)
        sys.exit(f"{len(errors)} unresolved rows")
    with TSV.open("w", newline="") as f:
        w = csv.DictWriter(f, fields, delimiter="\t", lineterminator="\n")
        w.writeheader()
        w.writerows(rows)
    summary(rows)


def summary(rows):
    """Markdown status x priority table, as pasted into docs/compatibility.md."""
    pri = sorted({r["priority"] for r in rows})
    cnt = Counter((r["status"], r["priority"]) for r in rows)
    print("| Status | " + " | ".join(pri) + " | Total |")
    print("| --- |" + " ---: |" * (len(pri) + 1))
    for s in STATUS.values():
        print(f"| {s} | " + " | ".join(str(cnt[(s, p)]) for p in pri) + f" | {sum(cnt[(s, p)] for p in pri)} |")
    print("| **Total** | " + " | ".join(str(sum(cnt[(s, p)] for s in STATUS.values())) for p in pri) + f" | {len(rows)} |")


def check():
    with TSV.open(newline="") as f:
        rows = list(csv.DictReader(f, delimiter="\t"))
    errors = check_rows(rows)
    if errors:
        print("\n".join(errors), file=sys.stderr)
        sys.exit(f"{len(errors)} problems in {TSV.name}")
    summary(rows)


if __name__ == "__main__":
    check() if "--check" in sys.argv[1:] else generate()
