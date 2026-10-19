#!/usr/bin/env python3
"""Record libxml2's decoding of bytes 0x80..=0xFF for each built-in charset.

Usage: uv run tooling/charset_oracle.py XMLLINT > tests/fixtures/charsets.tsv

XMLLINT must be libxml2 2.15.4 built with LIBXML2_WITH_ISO8859X=ON and without
iconv/ICU. Each byte is parsed in its own document and serialized as UTF-8;
0000 records a byte that xmllint rejects.
"""
import re
import subprocess
import sys

LABELS = ["US-ASCII", "ISO-8859-1"] + [
    f"ISO-8859-{n}" for n in (2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 14, 15, 16)
] + ["windows-1252"]


def decode(xmllint, label, byte):
    doc = f'<?xml version="1.0" encoding="{label}"?><r>'.encode() + bytes([byte]) + b"</r>"
    run = subprocess.run([xmllint, "--encode", "UTF-8", "-"], input=doc, capture_output=True)
    match = re.search(rb"<r>(.*)</r>", run.stdout, re.S)
    if run.returncode or not match:
        return 0
    text = re.sub(
        rb"&#(x?)([0-9a-fA-F]+);",
        lambda m: chr(int(m[2], 16 if m[1] else 10)).encode(),
        match[1],
    ).decode()
    assert len(text) == 1, (label, byte, text)
    return ord(text)


xmllint = sys.argv[1]
print("# libxml2 2.15.4 xmllint (ISO8859X, no iconv/ICU): label, then bytes 0x80..=0xFF")
for label in LABELS:
    print(label + "\t" + " ".join(f"{decode(xmllint, label, b):04x}" for b in range(0x80, 0x100)))
