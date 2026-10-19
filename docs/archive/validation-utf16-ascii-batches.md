# Rejected UTF-16 ASCII batching screen

The `experiment/utf16-ascii-batches` branch is not accepted for main. Against
`9e4b3a6`, bounded stack batches substantially shortened long UTF-16 text and
lexical-token parsing in a preliminary screen, but repeatedly slowed the UTF-8
tiny-element workload. Preserve the experiment rather than trading that
regression for a selected headline improvement.

## Candidate and semantic checks

Commit `d51c531` adds a maximum 256-byte ASCII batch after UTF-16 encoding
detection and scalar carry are drained. It stops before non-ASCII units and
angle brackets, and consumes only the prefix accepted by the tokenizer. The
tokenizer copies a complete plain run only when both raw and decoded ceilings
fit; otherwise scalar processing preserves the first failing source offset and
raw-before-decoded limit precedence. It adds no dependency, heap batch buffer,
unsafe code or retained source-offset map.

Commit `119055c` separates the tokenizer's UTF-8 and UTF-16 loop dispatch and
adds long-prefix malformed-surrogate and simultaneous token-ceiling checks.
That dispatch change did not remove the regression. Both paths still appear
inside one `Tokenizer::advance` symbol in the release binary; this observation
does not establish a code-generation cause.

Focused tests passed for both byte orders, one-byte through full-input splits,
batch boundaries, non-ASCII transitions, rejected consumer progress, exact
decoded/raw ceilings, event values/ranges, retained buffered input, cancellation
and sticky terminal state. Strict Clippy and formatting passed. A second agent
reviewed `d51c531` and ran its all-target suite without finding a semantic
blocker. The final dispatch variant passed its focused internal and UTF-16
public tests. It has not passed the required fresh LLVM coverage and external
libxml2 comparison gates, and has no claim of complete semantic validation.

## Preliminary performance evidence

Three local paired screens compared immutable release binaries. Each screen
used six fixtures, UTF-8/UTF-16LE/UTF-16BE, buffer capacities 1 and 8,192, and four
samples per build/case: one warmup and three retained samples. Within each
sample, adjacent case pairs alternated baseline-first and candidate-first
order; case order reversed between samples. Because there are 36 cases, this
schedule keeps each individual case's build order fixed across samples rather
than counterbalancing it. Every process
completed with one output row and empty stderr. All non-timing fields, including
event counts, byte counts and first-event consumed bytes, matched within pairs.
These are screening samples, not the 31-sample acceptance measurements used for
accepted storage changes. No libxml2 performance claim follows from them.

The builds were intended to compare baseline `9e4b3a6` with the experimental
sources described above. Retained binary hashes establish their immutable
identities, but the build logs do not retain source-revision and source-manifest
hashes. They therefore do not independently establish that source-to-binary
provenance. Any acceptance measurement must retain those manifests as well as
counterbalance build order within each case.

Representative final-screen medians are elapsed nanoseconds per configured
fixture batch; negative change means faster:

| Fixture | Encoding | Buffer | Baseline | Candidate | Change |
| --- | --- | ---: | ---: | ---: | ---: |
| Tiny elements | UTF-8 | 8,192 | 12,733,709 | 14,196,541 | +11.5% |
| Tiny elements | UTF-16BE | 8,192 | 19,316,541 | 20,380,167 | +5.5% |
| Tiny elements | UTF-16LE | 1 | 26,545,000 | 28,677,250 | +8.0% |
| Long text | UTF-16LE | 8,192 | 9,293,708 | 1,988,750 | -78.6% |
| Lexical payloads | UTF-16BE | 8,192 | 6,856,250 | 1,219,500 | -82.2% |
| Namespaces | UTF-16LE | 8,192 | 35,865,083 | 27,272,542 | -24.0% |

The UTF-8 tiny-element regression was between 11.5% and 14.4% in all three
screens. It remains unexplained; natural encoding dispatch alone did not fix it.
One-byte UTF-16 tiny-element input also regressed. Any continuation needs a
specific hypothesis and a new matched measurement, followed by the outstanding
semantic gates, before production integration.

Raw process rows, runner scripts, analyzers and summaries remain under
`/private/tmp/yaxml-utf16-batch-screen{,2,3}*` and
`/private/tmp/yaxml-utf16-batch-summary{,2,3}.txt`. The final runner is
`/private/tmp/yaxml-utf16-batch-screen3.rb`; the binaries are
`/private/tmp/yaxml-utf16-batch-pull-baseline` and
`/private/tmp/yaxml-utf16-batch-pull-candidate3`. Final artifact SHA-256 values:

| Artifact | SHA-256 |
| --- | --- |
| Baseline binary | `dbe4fb2d68ec6e617a7f88803d27f0579dedd3caefbab07810f8b71e18633554` |
| Candidate binary | `cb75c03aa91783688d64ae206b33c54308f28bcddcf8da6c693b922ceca6ae8c` |
| Raw process rows | `4520e8b2b0534413764b6a8954a77ae8bd35f02c90b29e41cee24f7875724840` |
| Runner script | `758b56d17fd87c01d1551dc9e35e3ef2171c622e7d0def733629f17ca9bf3ca0` |
| Full summary | `5c100db6f46c0da70238b784c41978d14b8874d0f5ed5dc33a77978aeedb99ed` |
