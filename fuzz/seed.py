#!/usr/bin/env -S uv run --script
"""Seed fuzz/corpus/<target>/ from the W3C XMLTS and libxml2 test inputs.
Usage: fuzz/seed.py [XMLCONF_DIR LIBXML2_TEST_DIR]. Inputs over MAX_INPUT_BYTES are skipped."""
import hashlib, os, re, sys
from pathlib import Path

W = Path(__file__).resolve().parent.parent.parent  # the directory holding yaxml-harness
conf = Path(sys.argv[1] if len(sys.argv) > 2 else W / "yaxml-suites/xmlconf")
lt = Path(sys.argv[2] if len(sys.argv) > 2 else W / "yaxml-oracle/libxml2-2.15.4/test")
out = Path(__file__).resolve().parent / "corpus"
MAX = 1 << 20
XML_TARGETS = "dom owned push buffered writer serialize limits encoding".split()
DOCS = [lt / d for d in ("", "valid", "VC", "VCM", "dtds", "namespaces", "errors", "errors10", "recurse", "SVG", "xmlid", "XInclude/docs", "c14n")]
DECL = re.compile(rb"^\s*<\?xml[^>]*\?>\s*")
SELECTORS = [0, 1, 2, 4, 5, 6, 0x82] + [8 | i << 4 | i % 3 for i in range(8)]

def put(target, data):
    if len(data) <= MAX:
        d = out / target
        d.mkdir(parents=True, exist_ok=True)
        (d / hashlib.sha1(data).hexdigest()[:16]).write_bytes(data)

files = [p for p in conf.rglob("*") if p.is_file()] + [p for d in DOCS for p in (d.rglob("*") if d.name in ("c14n", "docs") else d.glob("*")) if p.is_file()]
for p in files:
    data = p.read_bytes()
    for t in XML_TARGETS:
        put(t, data)
    body = DECL.sub(b"", data, 1)
    if body.startswith(b"<!DOCTYPE"):
        put("dtd", body)
    elif p.suffix in (".dtd", ".ent"):
        put("dtd", b"\0" + data)
    for s in SELECTORS if "c14n" in p.parts else SELECTORS[:1] + SELECTORS[5:6]:
        put("c14n", bytes([s]) + data)

docs = {p.name: p.read_bytes() for p in (lt / "XPath/docs").iterdir()}
for d in (lt / "XPath/expr", lt / "XPath/tests", lt / "XPath/xptr"):
    for p in d.glob("*"):
        doc = next((v for k, v in docs.items() if p.name.startswith(k)), None)
        for line in p.read_text(errors="replace").splitlines():
            put("xpath", line.encode())
            if doc:
                put("xpath", line.encode() + b"\0" + doc)

lines = [l for p in (lt / "URI").glob("*") for l in p.read_text(errors="replace").splitlines()]
for prev, l in zip([""] + lines, lines):
    put("uri", f"{l}\n{prev}\n/:%#".encode())

for x in (lt / "schemas").glob("*.xsd"):
    stem = x.stem.rsplit("_", 1)[0]
    for i in (lt / "schemas").glob(f"{stem}_*.xml"):
        put("xsd", x.read_bytes() + b"\0" + i.read_bytes())
