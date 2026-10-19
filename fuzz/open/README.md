# Fuzz reproducers (unfixed library findings)

Found by the seeded corpus run (see ../README.md). Replay: `cargo fuzz run <target> regressions/<target>/<file>` or `cargo run --release --example fuzz_replay -- <file>`. Not yet minimized by `cargo fuzz tmin`; no deterministic tests exist yet.

| Reproducer | Targets | Failure |
| --- | --- | --- |
| `{buffered,dtd,encoding,limits}/crash-*` | buffered, dtd, encoding, limits | panic `pull read beyond an event suspension` (pull.rs:300). 62-byte case: internal entity whose value is a character reference (`<!ENTITY e "&#38;">`) used in content. The limits case is an undeclared-entity document with an external DTD subset. |
| `dom/crash-*` | dom | panic `reader made no bounded progress` (driver.rs:445) on a 1.7 KB billion-laughs entity chain (`<!ENTITY laughN "&laugh(N-1);&laugh(N-1);">`). |
| `{owned,serialize,writer}/crash-*` | owned, serialize, writer | `LimitExceeded(OutputBytes)` at byte 31017 when serializing a valid tree (oracle asserts success, serialize.rs:56 / writer.rs:36). 5 KB input: DTD with an element declaring ~hundreds of defaulted enumerated attributes (`ATTLIST e a000 (0) '0' ...`), so defaults expand output beyond the oracle's `6*len+1024` bound. May be an oracle bound issue rather than a library bug. |
| `{owned,push}/slow-unit-*`, `push/timeout-*`, `c14n/{slow-unit,timeout}-*` | owned, push, c14n | Entity-expansion inputs (billion laughs) take 14 s to over 60 s per testcase: expansion is accepted without a time/size bound in these paths. |

Additional classes from the second (fork, ignore-crashes) pass; the table above lists the first. Counts are crashing inputs found:

| Reproducer | Failure |
| --- | --- |
| `limits/crash-d050*` (282 inputs) | `owned parse differs` assertion under derived limits (71-byte case). |
| `limits/crash-8141*` (4) | `reader/DOM policy acceptance mismatch` with DOCTYPE input. |
| `limits/crash-5b10*`, others (350 total across buffered/encoding/dtd/limits) | same `pull read beyond an event suspension` as above. |
| `buffered/crash-d1d8*`, `dtd/crash-baca*` | assertion `count <= bytes.len()*2+1` (billion-laughs expansion exceeds the pull oracle's bound). |
| `buffered/crash-cd20*` | `decoded semantic error changed kind`: Syntax at byte 132 vs expected InvalidState. |
| `{dtd,owned,serialize,writer}/crash-a78d*` | OutputBytes rejection at byte 129 (6.8 KB input); more OutputBytes variants (byte 98, 41541) exist in `artifacts/`. |
