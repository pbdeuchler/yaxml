# Project status

## Objective and constraints

yaxml aims to replace the useful libxml2 surface with a memory-safe Rust core,
then support Python, Ruby, JavaScript/TypeScript and Go bindings over stable
ownership and error contracts. The governing constraints are:

- `unsafe_code = "forbid"` throughout the Rust crate;
- `memchr` is the only dependency;
- bounded work, memory, diagnostics, subprocess output and retained evidence;
- compatibility claims tied to an explicit libxml2 baseline and reproducible
  differential evidence;
- fuzz target coverage and measured execution coverage tracked separately;
- performance claims based on retained matched measurements;
- binding needs considered in handles, ownership, callbacks and structured
  errors before language packages are implemented; and
- allocator-aware APIs may use the allocator facilities stabilized for Rust
  1.100 after the public API and MSRV policy are reviewed.

The main references are [the roadmap](../roadmap.md),
[the architecture](../design.md), [the compatibility inventory](../compatibility.md),
[the security model](../security.md) and the machine-readable
[libxml2 API inventory](../libxml2-api.tsv).

## Program status

| Area | State | Evidence and remaining work |
| --- | --- | --- |
| Feature and API catalogue | In progress | The compatibility and API inventories exist. Pinning one complete libxml2 release/build-feature baseline and closing every symbol/deviation remains open. |
| Core XML reader and DOM | Implemented for the documented profile | UTF-8 XML 1.0 subset parsing, namespaces, structured errors, bounded slice input, arena and owned DOM work, source origins and iterative traversal are present. Full XML and libxml2 behavior remain broader than the implemented profile. |
| Conformance and oracle infrastructure | Established | The pinned XMLTS run covers 2,585 cases across five parser interfaces, with 12,925 classified rows and a 407-ID/1,871-pair supported regression baseline. Pinned libxml2 comparisons and structural catalogs are retained. |
| Streaming | Implemented for current public profile | Push, callback and buffered reading have explicit final EOF, cancellation, I/O and retained-memory contracts. Private DTD streaming variants remain outside public activation. |
| Writing and serialization | Implemented for current UTF-8 profile | Event writing and complete DOM serialization exist with bounded output and failure behavior. Namespace repair, detached-subtree formatting options and UTF-16 output remain open. |
| Encodings | Partial | UTF-8 and reviewed UTF-16 input paths exist. Remaining legacy encodings and XML 1.1 are later work. |
| Mutation | Implemented for bounded owned DOM operations | Broader libxml2 mutation compatibility remains open. |
| DTD support | Large private implementation, not publicly activated | The private controller, entities, defaults, provenance, stable reader, streaming, buffered and writer compositions have extensive independent reviews. The active fuzz/process-integration milestone is described in the companion handoff file. Public APIs, DOM storage, broad conformance and performance gates remain open. |
| Fuzzing | Strong harness foundation; 100% open | Replay, bounded mutation campaigns, lineage, coverage accounting and private DTD drivers exist. Every input-bearing private DTD frontend still needs its own bounded target and the combined denominator must reach measured closure without exclusions. |
| Performance | Measured, mixed | In the first matched 65-workload baseline, libxml2 had the lower primary median in 42 workloads, yaxml in 19, and four were within the descriptive band. Targeted normalization work produced large Rust-side wins, but the project has not established across-the-board superiority. |
| Allocator API | Planned | The architecture should use Rust 1.100 allocator support where it improves arenas, owned documents, scratch storage and FFI ownership. No allocator-wide public contract should be frozen before API/MSRV review. |
| Language bindings | Not started | Python, Ruby, JavaScript/TypeScript and Go packages are a P4 deliverable after stable core ownership, error, threading and callback boundaries. |

## Roadmap position

The public P0/P1 foundation is substantial: the parser, DOM, streaming,
serialization, UTF-16, conformance harness, fuzz campaign and baseline
benchmarking are all present for bounded documented profiles. The largest
active engineering program is private DTD integration, which belongs to P2 in
the product roadmap but is being proven privately before public activation.

The current private DTD fuzz plan has five implementation stages:

1. private stable-reader driver and independent model — complete;
2. boundary, quota, ownership and lifecycle matrix — complete;
3. Cargo/libtest process protocol, source inventory and profile lifecycle —
   implemented and evidenced, final acceptance pending;
4. separate bounded streaming, buffered and writer targets plus a frozen
   focused-guard list — next implementation stage; and
5. pinned measured coverage closure for each target and the combined private
   DTD source-manifest union — open.

The detailed plan is
[Private DTD Reader fuzz and coverage integration](../plans/dtd-reader-fuzz.md).

## Project-management rules already established

- Design, implementation, conformance review, holistic review and integration
  are separate gates. A passing implementation report does not replace either
  independent review.
- Each coverage report freezes source, compiler, object, tool and profile
  identities. Profiles are merged only for byte-identical executable/object
  mappings.
- Production source rows are not excluded to improve a percentage. Gaps remain
  visible until the appropriate driver reaches them safely.
- Tests include deliberate production mutations. A claimed discriminator must
  turn red when its guarded behavior is removed and return green after a
  byte-exact restoration.
- Security-sensitive filesystem publication uses same-handle identity,
  containment and before/after gates. Unsupported platforms fail closed when a
  safe persistent identity is unavailable under the no-unsafe/one-dependency
  constraints.
- Temporary review worktrees must not mutate the same files concurrently.
  Mutation reviews use disposable copies or explicitly serialized access.

## Material risks and open decisions

1. **Temporary evidence retention.** The only sealed archive for the latest
   process candidate is under `/private/tmp`. Its checksum is recorded in the
   process-integration handoff. Preserve it before any cleanup or reboot.
2. **Final review gap.** Repair-specific reviews pass, but no final full-series
   review report exists for the regenerated `r11` archive. Integration is
   blocked on two new PASS reports.
3. **Public DTD activation.** Private correctness does not yet settle public
   event/error compatibility, external-resource policy, DOM representation or
   performance.
4. **Coverage language.** Current evidence validates campaign mechanics and
   exact report accounting. It does not establish crate-wide or private-DTD
   100% coverage.
5. **Performance language.** Current measurements do not support a blanket
   claim that yaxml is faster than libxml2.
6. **Platform support for the campaign tool.** The library continues to
   cross-check on Windows, but the security-sensitive `fuzz_campaign` example
   intentionally returns an unsupported-platform error on non-Unix systems
   until stable safe persistent file identity is available.
