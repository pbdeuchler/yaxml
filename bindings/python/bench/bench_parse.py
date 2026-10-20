"""Parse (and a few follow-up operations) on yaxml-harness's benches/parse.rs
fixtures: yaxml vs lxml vs the standard library's xml.etree.

    uv run --group compare python bench/bench_parse.py [--repeat 7]

Reports the median wall time per call over `repeat` runs, each run sized to
take at least ~0.2 s, and throughput in MiB/s.
"""

from __future__ import annotations

import argparse
import statistics
import time
import xml.etree.ElementTree as stdlib

import yaxml

try:
    import lxml.etree as lxml
except ImportError:  # pragma: no cover
    lxml = None

FIXTURES = {
    "tiny": "<root a='v'>text</root>",
    "flat": "<root>" + "<item a='value'>text</item>" * 10_000 + "</root>",
    "text": "<root>" + "text " * 100_000 + "</root>",
    "normalized": "<root>" + "<item a='&#x9;'>a&amp;b</item>" * 10_000 + "</root>",
    "namespaces": "<r xmlns:p='urn:p'>" + "<p:item p:a='v'/>" * 10_000 + "</r>",
    "dtd": "<!DOCTYPE root [<!ENTITY e 'one <b>two</b> &#x33;'>"
    "<!ATTLIST item a CDATA 'd' t NMTOKENS ' x  y '>]>"
    "<root>" + "<item t=' p  q '>&e;&amp;</item>" * 10_000 + "</root>",
}


def measure(function, repeat: int) -> float:
    function()
    loops, elapsed = 1, 0.0
    while True:
        start = time.perf_counter()
        for _ in range(loops):
            function()
        elapsed = time.perf_counter() - start
        if elapsed >= 0.2:
            break
        loops *= 2
    samples = [elapsed / loops]
    for _ in range(repeat - 1):
        start = time.perf_counter()
        for _ in range(loops):
            function()
        samples.append((time.perf_counter() - start) / loops)
    return statistics.median(samples)


def libraries():
    out = {"yaxml": yaxml}
    if lxml is not None:
        out["lxml"] = lxml
    out["stdlib"] = stdlib
    return out


def operations(module, data: bytes):
    root = module.fromstring(data)
    yield "parse", lambda: module.fromstring(data)
    yield "tostring", lambda: module.tostring(root)
    yield "iter", lambda: sum(1 for _ in root.iter())
    yield "findall", lambda: len(root.findall(".//item"))


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repeat", type=int, default=7)
    parser.add_argument("--fixture", action="append")
    arguments = parser.parse_args()
    names = arguments.fixture or list(FIXTURES)
    print(f"{'fixture':<11} {'operation':<9} {'library':<7} {'median':>12} {'MiB/s':>9} {'vs yaxml':>9}")
    for name in names:
        data = FIXTURES[name].encode()
        results: dict[tuple[str, str], float] = {}
        for library, module in libraries().items():
            for operation, function in operations(module, data):
                results[(operation, library)] = measure(function, arguments.repeat)
        for (operation, library), seconds in results.items():
            base = results[(operation, "yaxml")]
            rate = len(data) / seconds / (1 << 20)
            print(
                f"{name:<11} {operation:<9} {library:<7} {seconds * 1e6:>10.2f}us {rate:>9.2f} "
                f"{seconds / base:>8.2f}x"
            )


if __name__ == "__main__":
    main()
