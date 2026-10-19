# Side-index attribute observer code generation

This note explains the bounded optimized-code inspection requested by the
[attribute membership experiment](owned-attribute-membership-experiment.md).
It compares the unchanged `abdd709` representation with the rejected side-index
prototype at `27e7ba1`. It does not change a representation, API, annotation, or
benchmark, and it does not make a general performance claim.

## Frozen observer and build

The inspected executable is the exact external diagnostic used for the 31-sample
counterbalanced measurement. Its combined phase repeats `observe()` 1,024 times.
Each observation walks `OwnedDocument::nodes()` and consumes these public values:

- node count and `OwnedNode::kind()`;
- qualified, local, and namespace name lengths from `OwnedNode::name()`;
- attribute count plus qualified, local, and namespace name lengths, value
  length, and first value byte from `OwnedNode::attributes()`.

The phase publishes node and attribute counts and a wrapping checksum. The
analyzer rejects a pair unless all three observables agree. For example, retained
`sparse0` sample 1 has 4,098 nodes, zero attributes, 4,196,352 node visits, and
combined checksum `16674414433853468672` in both executables. Its measured
combined times were 9,472,959 ns for baseline and 13,230,542 ns for the
side-index prototype. Across 31 retained samples, the medians were 9,168,459 ns
and 13,161,250 ns, a 43.5% increase; both relative MADs were 0.1%.

Both executables were built offline on Apple M1 Max/aarch64 macOS with rustc
1.95.0 (`59807616e`, LLVM 22.1.2), the locked `memchr 2.8.3`, and the same release
profile: optimization level 3, thin LTO, one codegen unit, aborting panics, and
stripped debug information. The commands were:

```sh
CARGO_TARGET_DIR=/private/tmp/yaxml-attribute-ir-baseline/target-flags \
  cargo rustc --release --offline --locked -vv -- --emit=llvm-ir,asm
CARGO_TARGET_DIR=/private/tmp/yaxml-attribute-ir-side/target-flags \
  cargo rustc --release --offline --locked -vv -- --emit=llvm-ir,asm
```

The logged final binary invocation includes `-C opt-level=3 -C panic=abort
-C lto=thin -C codegen-units=1 -C strip=debuginfo --emit=llvm-ir,asm`. Separate
target directories forced complete builds against the archived exact sources.

## Generated shape

The baseline has one element content variant. In the combined loop, LLVM maps the
content discriminant to `OwnedNodeKind`, tests once for the element range, and
then loads the name and parsed attribute range from fixed offsets. The relevant
AArch64 sequence begins:

```asm
ldr   x11, [x10, #24]
sub   x8, x11, #2
cmp   x11, #1
csinc x8, x8, xzr, hi
cmp   x8, #6
b.ne  ...
...
cmp   x11, #1
b.hi  ...
ldp   x2, x3, [x10, #48]  // direct stored name range
...
ldp   x2, x3, [x10, #32]  // direct parsed attribute range
```

The side-index prototype adds `ElementBaseline` and `ElementEdited`. LLVM keeps
both cases in the inlined combined loop. It dispatches over the expanded kind,
selects different name payload addresses, and later dispatches again between the
global parsed range and the edited-buffer arena:

```asm
ldr   x9, [x8, #24]!
sub   x11, x9, #2
cmp   x9, #1
csinc x11, x11, xzr, hi
cmp   x11, #3
b.gt  ...
...
cmp   x11, #1
csel  x12, x8, x9, eq     // baseline or edited name payload
...
cmp   x11, #1
b.ne  ...
ldp   x0, x1, [x10, #72]  // baseline attribute range
...
ldr   x0, [x10, #80]      // edited-buffer index
ldr   x1, [x30, #112]     // edited-buffer arena length
```

The optimized LLVM IR shows the same distinction. Baseline reduces element
handling to one signed-range test before the direct name path. The side version
retains a switch with both element values, a pointer `select` for their different
name layouts, and a later two-case switch for attribute storage:

```llvm
; baseline
%is_element_or_document = icmp samesign ult i64 %discriminant, 2
br i1 %is_element_or_document, label %name, label %loop.backedge

; side index
switch i64 %kind, label %unreachable [
  i64 1, label %element
  i64 2, label %element
  ...
]
%is_baseline = icmp eq i64 %kind, 1
%name_payload = select i1 %is_baseline, ptr %baseline, ptr %edited
...
switch i64 %kind, label %loop.backedge [
  i64 1, label %baseline.attributes
  i64 2, label %edited.attributes
]
```

The assembly between the `Instant` clock calls for the whole combined phase is
340 lines/299 instruction lines in baseline and 409 lines/361 instruction lines
in the side prototype. The same static regions contain 73 and 84 conditional
branch instructions under a mechanical mnemonic count. These are static control
flow graph sizes, not executed instruction or misprediction counts; panic and
bounds-check paths in the regions need not execute.

## Explanation and falsification

The evidence supports one specific explanation: adding a second element variant
prevents this optimized public accessor composition from collapsing to the
parsed-element layout. The exact loop therefore executes additional element-case
selection and must retain both baseline-range and side-arena attribute paths.
Sparse fixtures do little attribute work to amortize this dispatch, which fits
the 43.5% `sparse0` and 33.9% mostly-empty regressions. Standalone iteration,
kind, name, and attribute phases remain much closer because they do not compose
all three accessor decisions in one inlined loop.

This explanation is falsifiable. If a future compiler or representation lets
the exact observer prove that every parsed element uses the baseline layout, its
optimized IR should collapse to one element case with direct name and attribute
offsets: no baseline/edited name `select` and no second attribute-storage switch.
With the same observable counts and checksum, the sparse0 and mostly-empty gap
should then largely disappear. If the dispatch disappears but the stable timing
gap remains, this explanation is false.

The inspection does not allocate causality among added instructions, branch
prediction, register pressure, instruction-cache placement, or other
microarchitectural effects. No hardware counters or cycle-level simulation were
used. It establishes a code-shape difference consistent with the measured
regression, not a compiler defect.

## Artifacts

Full optimized IR and assembly, phase-bounded assembly, focused dispatch
snippets, and complete verbose compiler logs remain in
`/private/tmp/yaxml-attribute-ir-artifacts` and `/private/tmp`. Primary hashes:

| Artifact | SHA-256 |
| --- | --- |
| Exact diagnostic source | `7916e02b8ebfa2b6021e0aad602991058f313726a09bd32e1095605a52b3b1ff` |
| Raw 31-sample evidence | `090532f5f737ff65bd728fac3755854b4a2685179eac364c9be283b510d1d3c6` |
| Baseline full IR / assembly | `0d91ac3a8c826f3d8e797d1210beeaf41485f6da7311f237ec003702a0e5b278` / `51e095c74a32fd67d6824bfc27cd976cc5e832ecb1533071a45cdde4d1a865ac` |
| Side full IR / assembly | `82871db6ab5c2b7daf2b37f7671e64a810d653f793a461074f62d0f6accd4da1` / `3be5fdc49cf7498cb65ca927e76777446ba40d2bb07ee879940d1caeb9e3e923` |
| Baseline phase-bounded assembly | `9910b2e4cae68faa81e9106c910589de131fcfae28225196fd06a3be8999aa2e` |
| Side phase-bounded assembly | `3a6af586b9dbf78ace39069da8e548b8284dbd7e83e103827a36fef0da33dca2` |
| Baseline focused IR / assembly | `ab2ddd4ae7bf47e96b20c07ac056d4b789e3cbac2f43248383785addb9fb1504` / `7b86c36abe5aa9aac44b4bfa84ec7714b2c5cf6f9ae87d795c3c078c2bba1f34` |
| Side focused IR / assembly | `7c4d607bc9407caf7d7cb07d774d96536121457380f58b79aa2054518a53781f` / `3110cf752d53a251e55e196f9f15fe4e8ffab4b7a1c4415131e650d5a19e3f56` |
| Baseline / side verbose rustc logs | `3562c42e043fd2b3beebad4ba5767aa0d2e8896752ef667c22fad690230a9593` / `52fa6ac095b2053b63cfe4ec9dd37d8063d7877cfc73d92c993a5fd8a1e2a068` |
| Archived exact `27e7ba1` source | `ff068dbeeabaafa92937cb176bbf4577b14bb214e072f6f89ffd4c8b3197a873` |
