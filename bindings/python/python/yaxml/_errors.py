"""Exceptions and error logs, shaped like lxml.etree's."""

from __future__ import annotations

from collections.abc import Iterable, Iterator, Sequence
from typing import overload

DOMAIN_NAMES = {
    0: "NONE",
    1: "PARSER",
    3: "NAMESPACE",
    8: "IO",
    11: "XINCLUDE",
    12: "XPATH",
    16: "SCHEMASP",
    17: "SCHEMASV",
    19: "RELAXNGV",
    23: "VALID",
    28: "SCHEMATRONV",
}
LEVEL_NAMES = {0: "NONE", 1: "WARNING", 2: "ERROR", 3: "FATAL"}

Row = tuple[str, int, int, int, int, int, "str | None"]


class LogEntry:
    """One libxml2 ``xmlError``: lxml's ``_LogEntry``."""

    __slots__ = ("message", "domain", "type", "level", "line", "column", "filename")

    def __init__(
        self,
        message: str,
        domain: int,
        type: int,
        level: int,
        line: int,
        column: int,
        filename: str | None,
    ) -> None:
        self.message = message
        self.domain = domain
        self.type = type
        self.level = level
        self.line = line
        self.column = column
        self.filename = filename

    @property
    def domain_name(self) -> str:
        return DOMAIN_NAMES.get(self.domain, str(self.domain))

    @property
    def level_name(self) -> str:
        return LEVEL_NAMES.get(self.level, str(self.level))

    @property
    def type_name(self) -> str:
        return str(self.type)

    @property
    def path(self) -> None:
        return None

    def __str__(self) -> str:
        return (
            f"{self.filename or '<string>'}:{self.line}:{self.column}:"
            f"{self.level_name}:{self.domain_name}:{self.type}: {self.message}"
        )

    def __repr__(self) -> str:
        return str(self)


class ErrorLog(Sequence[LogEntry]):
    """An immutable list of :class:`LogEntry`: lxml's ``_ListErrorLog``."""

    def __init__(self, entries: Iterable[LogEntry] = ()) -> None:
        self._entries = list(entries)

    @overload
    def __getitem__(self, index: int) -> LogEntry: ...
    @overload
    def __getitem__(self, index: slice) -> list[LogEntry]: ...
    def __getitem__(self, index: int | slice) -> LogEntry | list[LogEntry]:
        return self._entries[index]

    def __len__(self) -> int:
        return len(self._entries)

    def __iter__(self) -> Iterator[LogEntry]:
        return iter(self._entries)

    def __repr__(self) -> str:
        return "\n".join(map(str, self._entries))

    @property
    def last_error(self) -> LogEntry | None:
        errors = [e for e in self._entries if e.level >= 2]
        return errors[-1] if errors else None

    def filter_from_level(self, level: int) -> ErrorLog:
        return ErrorLog(e for e in self._entries if e.level >= level)

    def filter_from_errors(self) -> ErrorLog:
        return self.filter_from_level(2)

    def filter_from_warnings(self) -> ErrorLog:
        return self.filter_from_level(1)

    def filter_domains(self, domains: int | Iterable[int]) -> ErrorLog:
        wanted = {domains} if isinstance(domains, int) else set(domains)
        return ErrorLog(e for e in self._entries if e.domain in wanted)


def log(rows: Iterable[Row]) -> ErrorLog:
    return ErrorLog(LogEntry(*row) for row in rows)


class Error(Exception):
    """Base of every yaxml exception (lxml's ``etree.Error``)."""


class LxmlError(Error):
    def __init__(self, message: str, error_log: ErrorLog | None = None) -> None:
        super().__init__(message)
        self.error_log = error_log if error_log is not None else ErrorLog()


class LxmlSyntaxError(LxmlError, SyntaxError):
    pass


class ParseError(LxmlSyntaxError):
    """A parse failure with libxml2's code, domain, line and column."""

    def __init__(
        self,
        message: str,
        code: int = 0,
        line: int = 0,
        column: int = 0,
        filename: str | None = None,
        domain: int = 1,
        rows: Iterable[Row] = (),
    ) -> None:
        super().__init__(message, log(rows))
        self.msg = message
        self.code = code
        self.domain = domain
        self.lineno = line
        self.offset = column
        self.filename = filename or "<string>"

    @property
    def position(self) -> tuple[int, int]:
        return (self.lineno or 0, self.offset or 0)

    @property
    def line(self) -> int:
        return self.lineno or 0

    @property
    def column(self) -> int:
        return self.offset or 0


class XMLSyntaxError(ParseError):
    pass


class DocumentInvalid(LxmlError):
    pass


class XPathError(LxmlError):
    def __init__(self, message: str, code: int = 0) -> None:
        super().__init__(message)
        self.code = code


class XPathEvalError(XPathError):
    pass


class XPathSyntaxError(LxmlSyntaxError, XPathError):
    def __init__(self, message: str, code: int = 0) -> None:
        LxmlError.__init__(self, message)
        self.msg = message
        self.code = code


class _SchemaParseError(LxmlError):
    def __init__(self, message: str, rows: Iterable[Row] = ()) -> None:
        super().__init__(message, log(rows))


class XMLSchemaError(LxmlError):
    pass


class XMLSchemaParseError(_SchemaParseError, XMLSchemaError):
    pass


class RelaxNGError(LxmlError):
    pass


class RelaxNGParseError(_SchemaParseError, RelaxNGError):
    pass


class DTDError(LxmlError):
    pass


class DTDParseError(_SchemaParseError, DTDError):
    pass


class SchematronError(LxmlError):
    pass


class SchematronParseError(_SchemaParseError, SchematronError):
    pass


class C14NError(LxmlError):
    pass


class SerialisationError(LxmlError):
    pass


class StaleElementError(Error, ValueError):
    """The proxy's node was removed from its document."""


__all__ = [
    "C14NError",
    "DTDError",
    "DTDParseError",
    "DocumentInvalid",
    "Error",
    "ErrorLog",
    "LogEntry",
    "LxmlError",
    "LxmlSyntaxError",
    "ParseError",
    "RelaxNGError",
    "RelaxNGParseError",
    "SchematronError",
    "SchematronParseError",
    "SerialisationError",
    "StaleElementError",
    "XMLSchemaError",
    "XMLSchemaParseError",
    "XMLSyntaxError",
    "XPathError",
    "XPathEvalError",
    "XPathSyntaxError",
]
