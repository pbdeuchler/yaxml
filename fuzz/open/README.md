# Open fuzz findings (unfixed, found by the seeded corpus run)

Kept outside `fuzz/regressions/` because `tests/fuzz_smoke.rs::fuzz_regressions_replay` replays that tree and requires every case to pass. Move a file there once fixed. Replay: `fuzz/target/<triple>/release/<target> open/<target>/<file>` (after `cargo fuzz build`) or `cargo run --release --example fuzz_replay -- <file>`. Not tmin-minimized. Verified against main at ebe3b6f (which already fixed the `pull read beyond an event suspension` and `owned parse differs` classes also found here).

| Reproducer | Failure |
| --- | --- |
| `dom/crash-d1d8*`, `dtd/crash-baca*` | panic `reader made no bounded progress` (driver.rs) on a 1.7 KB billion-laughs entity chain (`<!ENTITY laughN "&laugh(N-1);&laugh(N-1);">`). |
| `buffered/crash-d1d8*` | same input: assertion `count <= bytes.len()*2+1` in the pull oracle. |
| `buffered/crash-cd20*` | `decoded semantic error changed kind`: Syntax at byte 132 vs expected InvalidState. |
| `{owned,serialize,writer,dtd}/crash-*` | `LimitExceeded(OutputBytes)` serializing a valid tree (oracle asserts success). DTD with many defaulted enumerated attributes (`ATTLIST e a000 (0) '0' ...`) whose defaults expand output past the `6*len+1024` bound. Possibly an oracle bound issue. |
| `limits/crash-8141*` | `reader/DOM policy acceptance mismatch` for a DOCTYPE input. |
| `{c14n,owned,push}/{slow-unit,timeout}-*` | 11 s to over 70 s per testcase on entity-expansion inputs (billion laughs); no expansion time/size bound on these paths. |
