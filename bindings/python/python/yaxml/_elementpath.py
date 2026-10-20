"""ElementPath (``find``/``findall``) translated to XPath 1.0.

ElementPath is a subset of XPath with Clark names (``{uri}local``); each
Clark namespace gets a generated prefix. Paths are compiled once and cached.
"""

from __future__ import annotations

import re
from collections.abc import Mapping
from functools import lru_cache

from ._yaxml import XPath

# The tokenizer of xml.etree.ElementPath, with quoted strings kept whole.
_TOKEN = re.compile(
    r"('[^']*'|\"[^\"]*\"|::|//?|\.\.|\(\)|!=|[/.*:\[\]\(\)@=])"
    r"|((?:\{[^}]+\})?[^/\[\]\(\)@!=\s]+)|\s+"
)
_LITERAL = re.compile(r"^-?\d+$|^last$")


def compile(path: str, namespaces: Mapping[str | None, str] | None = None) -> XPath:
    key = tuple(sorted((namespaces or {}).items(), key=lambda kv: kv[0] or ""))
    return _compile(path, key)


@lru_cache(maxsize=512)
def _compile(path: str, namespaces: tuple[tuple[str | None, str], ...]) -> XPath:
    expression, bindings = translate(path, dict(namespaces))
    return XPath(expression, namespaces=bindings)


def translate(path: str, namespaces: Mapping[str | None, str]) -> tuple[str, dict[str, str]]:
    """The XPath expression for an ElementPath and the prefixes it binds."""
    if path.startswith("/"):
        raise SyntaxError("cannot use absolute path on element")
    if not path or path.endswith("/"):
        raise SyntaxError(f"invalid path {path!r}")
    default = namespaces.get(None) or namespaces.get("")
    bindings = {p: u for p, u in namespaces.items() if p}
    generated: dict[str, str] = {}

    def prefix_for(uri: str) -> str:
        if uri not in generated:
            name = f"_yx{len(generated)}"
            generated[uri] = name
            bindings[name] = uri
        return generated[uri]

    out: list[str] = []
    depth = 0
    previous = ""
    for match in _TOKEN.finditer(path):
        op, tag = match.group(1), match.group(2)
        if op:
            depth += op == "["
            depth -= op == "]"
            out.append(op)
            previous = op
            continue
        if not tag:
            continue
        attribute = previous == "@"
        previous = tag
        if depth and _LITERAL.match(tag):
            out.append(tag)
        elif tag.startswith("{*}"):
            local = tag[3:]
            out.append("*" if local == "*" else f"*[local-name()='{local}']")
        elif tag.startswith("{}"):
            out.append(tag[2:])
        elif tag.startswith("{"):
            uri, local = tag[1:].split("}", 1)
            out.append(f"{prefix_for(uri)}:{local}")
        elif ":" in tag:
            prefix = tag.split(":", 1)[0]
            if prefix not in bindings:
                raise SyntaxError(f"prefix {prefix!r} not found in prefix map")
            out.append(tag)
        elif default and not attribute and tag != "*":
            out.append(f"{prefix_for(default)}:{tag}")
        else:
            out.append(tag)
    return "".join(out), bindings
