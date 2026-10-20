"""Concurrent use: documents are locked, and parsing, serialization and
validation release the GIL."""

import io
import threading

import yaxml as et

BIG = b"<r>" + b"<i a='1'>text</i>" * 20000 + b"</r>"


def run(workers):
    errors = []

    def wrap(work):
        try:
            work()
        except Exception as error:  # noqa: BLE001
            errors.append(error)

    threads = [threading.Thread(target=wrap, args=(w,)) for w in workers]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join()
    assert errors == []


def test_parallel_parse_and_serialize():
    results = []

    def work():
        root = et.fromstring(BIG)
        results.append(len(et.tostring(root)))

    run([work] * 8)
    assert len(set(results)) == 1


def test_shared_document_readers_and_writer():
    root = et.fromstring(BIG)
    schema = et.RelaxNG(et.fromstring(
        b"<element name='r' xmlns='http://relaxng.org/ns/structure/1.0'><zeroOrMore>"
        b"<element name='i'><attribute name='a'/><text/></element></zeroOrMore></element>"))

    def read():
        for _ in range(5):
            assert len(root.xpath("//i")) >= 20000
            et.tostring(root)
            schema.validate(root)

    def write():
        for n in range(200):
            et.SubElement(root, "i", a=str(n)).text = "x"

    run([read, read, write, read])
    assert len(root) == 20200


def test_cross_document_moves_from_threads():
    target = et.Element("t")

    def move():
        for _ in range(50):
            source = et.fromstring(b"<s><a/></s>")
            target.append(source[0])

    run([move] * 4)
    assert len(target) == 200


def test_parse_file_objects_concurrently():
    run([lambda: et.parse(io.BytesIO(BIG))] * 4)
