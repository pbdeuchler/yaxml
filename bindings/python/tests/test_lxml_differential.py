"""Differential test against lxml over the libxml2 2.15.4 test corpus.

Both sides parse every ``test/**/*.xml`` file with lxml's default parser
options (``XMLParser()`` on each side; nothing external is loaded) and are
compared on acceptance, error code and line, the element tree (tags,
attributes, text, tails, comments and PIs), ``tostring`` bytes and C14N.

The corpus is found at ``$YAXML_LIBXML2_SOURCE/test`` or
``yaxml-oracle/libxml2-2.15.4/test`` beside the worktree directory; the test
is skipped without it or without lxml. ``-s`` prints the agreement table.
"""

from __future__ import annotations

import os
import pathlib
from collections import Counter

import pytest

import yaxml

lxml = pytest.importorskip("lxml.etree")

HERE = pathlib.Path(__file__).resolve().parent
SOURCE = pathlib.Path(
    os.environ.get("YAXML_LIBXML2_SOURCE", HERE / "../../../../../yaxml-oracle/libxml2-2.15.4")
)
CORPUS = sorted((SOURCE / "test").rglob("*.xml")) if (SOURCE / "test").is_dir() else []

# Minimum agreement rates (percent) this run must keep.
FLOORS = {"acceptance": 95.0, "error": 80.0, "tree": 95.0, "tostring": 85.0, "c14n": 90.0}


def shape(module, element):
    """A comparable view of an element subtree."""
    tag = element.tag
    if not isinstance(tag, str):
        kind = "comment" if tag is module.Comment else "pi" if tag is module.PI else "entity"
        return (kind, getattr(element, "target", None) if kind == "pi" else None, element.text, element.tail)
    return (
        tag,
        sorted(element.attrib.items()),
        element.text,
        element.tail,
        [shape(module, child) for child in element],
    )


def run(module, data: bytes):
    try:
        tree = module.parse(__import__("io").BytesIO(data), module.XMLParser())
    except module.XMLSyntaxError as error:
        return ("error", error.code, error.lineno)
    except Exception as error:  # noqa: BLE001 - lxml raises others for odd inputs
        return ("error", type(error).__name__, None)
    root = tree.getroot()
    if root is None:
        return ("error", "no root", None)
    try:
        c14n = module.tostring(tree, method="c14n")
    except Exception as error:  # noqa: BLE001
        c14n = type(error).__name__
    return ("ok", shape(module, root), module.tostring(tree), c14n)


@pytest.mark.skipif(not CORPUS, reason="libxml2 test corpus not found")
def test_agreement_with_lxml():
    totals, agreed = Counter(), Counter()
    differences: list[str] = []
    for path in CORPUS:
        data = path.read_bytes()
        mine, theirs = run(yaxml, data), run(lxml, data)
        name = str(path.relative_to(SOURCE))
        totals["acceptance"] += 1
        if mine[0] != theirs[0]:
            differences.append(f"acceptance {name}: yaxml {mine[:3]!r:.80} lxml {theirs[:3]!r:.80}")
            continue
        agreed["acceptance"] += 1
        if mine[0] == "error":
            totals["error"] += 1
            agreed["error"] += mine[1:] == theirs[1:]
            continue
        for index, metric in enumerate(("tree", "tostring", "c14n"), start=1):
            totals[metric] += 1
            if mine[index] == theirs[index]:
                agreed[metric] += 1
            elif metric == "tree":
                differences.append(f"tree {name}")
    rates = {metric: 100.0 * agreed[metric] / totals[metric] for metric in totals}
    print(f"\nlxml {'.'.join(map(str, lxml.LXML_VERSION))} (libxml2 {lxml.LIBXML_VERSION}), {len(CORPUS)} files")
    for metric, rate in rates.items():
        print(f"  {metric:<10} {agreed[metric]:>4}/{totals[metric]:<4} {rate:6.2f}%")
    for line in differences[:40]:
        print("  ", line)
    for metric, floor in FLOORS.items():
        assert rates.get(metric, 100.0) >= floor, (metric, rates)
