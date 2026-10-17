# Private DTD Reader libtest protocol parser review

Plan: [private DTD Reader fuzzing](plans/dtd-reader-fuzz.md).
Integrated commit: `94b38330b23d43fa872675a40fb819c21b55e64a`.
Tree: `b98d7b06f82c6d40dd6910041e75cc3131c84f79`.
Base: `9af4644a94cdb57cfc228feea1a66bd508130f28`.

Verdict: PASS, with no open Critical, Important or Minor findings for the
parser slice. Campaign process invocation, Cargo artifact selection and raw
profile validation remain separate implementation gates.

## Frozen protocol

`libtest-exact-v1` names the ignored private entry
`reader::dtd_fuzz_tests::replay_stdin` and fixes these four arguments in order:
`--exact`, the test name, `--ignored`, and `--test-threads=1`.

The parser accepts one exact successful libtest frame using uniformly LF or
uniformly CRLF line endings. It rejects mixed endings, lone carriage returns,
unexpected lines or blank lines, ANSI bytes, a byte-order mark, non-ASCII
bytes, altered names, statuses or counts, and any whitespace outside the fixed
grammar. Filtered counts and elapsed whole seconds are canonical checked
`u32` decimals. Elapsed time may omit the fraction or carry one through nine
fraction digits; its accepted spelling is preserved without floating-point
conversion. The largest accepted frame is 186 normalized LF bytes or 192 CRLF
wire bytes. Parsing itself is capped at 1,024 bytes so malformed duplicate and
extra-line diagnostics remain observable within a fixed allocation bound.

The error type preserves structured categories for encoding, line endings,
shape, name, status, counts, whitespace and each numeric failure. Independent
literal fixtures cover changing counts and times, maximum values, line endings,
ordering, truncation, extra output, every decimal failure and fixed-byte
perturbations.

## Pinned evidence and review repairs

The pinned nightly `2026-06-06` libtest executable produced a 163-byte stdout
frame with empty stderr, 802 filtered tests and elapsed text `0.00`; the parser
accepted the captured bytes and exact structured values.

Initial review found that malformed summary spacing was rejected under other
error categories. Two narrow repairs route leading, doubled, trailing, tabbed
and internal summary-prefix whitespace to the whitespace checker without
capturing ordinary test lines or weakening duplicate-line, extra-line or
numeric classifications. Disposable regressions confirmed both repairs are
required.

The final matrix-conformance review passed with report SHA-256
`5583981af20dd19287893220fabed6d85ac9eefa7bc98c4972ef7da788b18795`.
The final holistic review passed with report SHA-256
`c0b0ed3aa18a8dae071e05a3d823863767115f1ff8976a4c8ce67fd93a4b8da3`.

The integrated example passes 20 tests in debug and release. Strict example
Clippy, formatting and diff checks pass. The module remains test-only until the
campaign shell, fake subprocess and real-process acceptance path are wired in
the next slices.
