# Normalization scan optimization

Base: f1b824ecf81cd4e4c62788ac56dfde0f53b74038.

The matched parsing baseline identifies long unchanged text as a large gap.
The private microphase probe finds scalar normalization detection expensive;
the parser and normalizer both scan the same raw value. These are independent
measurements, not an additive partition of parse time.

Evaluate two small changes separately: A replaces detection with the existing
memchr dependency; B gives the parser sole ownership of detection and the Cow
decision, leaving the transformation to return String. Consider retaining B
alone if the combination causes a repeated material regression. Avoid
unmeasured thresholds, custom unsafe/SIMD code, new dependencies, or changes
to XML-character validation.

Required behavior: the four detection contexts retain their exact trigger
sets; unchanged values borrow; changed values allocate and have the same XML
normalization/reference semantics; raw-length owned-byte accounting happens
before allocation and reference parsing. Preserve validation precedence and
UTF-8/UTF-16 source-byte diagnostics. No public API change or new capability.

Acceptance: independent literal/property regressions first pass on baseline;
happy/error/quota/ownership/offset cases pass on the final source; ordinary
tests, Clippy and formatting pass. Run the full pinned XMLTS catalog and keep
all required-pass pairs. Produce fresh library-only fuzz-smoke coverage with
all source files and no production exclusions; report remaining gaps honestly.

Measure the same fixed workloads and iterations in paired fresh processes,
alternating order, with exact semantic observations, source/build/binary/input
identity, and raw samples retained. Screen 25 workloads first; test the final
candidate across all 65 workloads and investigate/repeat any material
regression. DOM primary is parse plus drop; streaming includes observation.
Do not infer libxml2 superiority or universal performance from a Rust A/B run.

Independently review the complete diff and evidence, including deliberately
broken sentinel detection, borrowed selection, and quota/reference ordering,
before integration into local main. Record rejected experiments as such.
