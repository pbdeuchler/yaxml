# Frozen independent stable-reader quota matrix

Accepted base: `fc364ed`, tree `d4adffd`. This plan is frozen before executing the new facade cases or inspecting any subsequent author implementation. The existing accepted lower-level scanner/unit/catalog/frame/reference/local specifications supply the literal quota rules. Production stays untouched; a test-module include belongs in the existing author-test file only.

The facade must retain the native typed cause, physical diagnostic anchor, replacement identity/local byte offset when applicable, latest delivered origin and accepted primary high-water. Three repeated failures must preserve the exact first error, position, origin and observable counters. Every rejection has an independent complete-document success control. Success controls also check nondecreasing aggregate work across declaration/body publication.

Fixtures:

- T: `<!DOCTYPE r><r/>`.
- M: `<!DOCTYPE r [<!ELEMENT r (a)>]><r/>`.
- A: `<!DOCTYPE r [<!ATTLIST r a CDATA #IMPLIED>]><r/>`.
- V: `<!DOCTYPE r [<!ENTITY e 'é'>]><r a='&e;'/>`.
- G: `<!DOCTYPE r [<!ENTITY e '<n/>'>]><r>&e;</r>`.
- R: `<!DOCTYPE r [<!ENTITY e 'x'>]><r>&e;&e;</r>`.
- N: `<!DOCTYPE r [<!ENTITY a '&b;'><!ENTITY b 'B'>]><r>&a;</r>`.
- P: `<!DOCTYPE r [<!ENTITY % p '<!--a&#13;b-->'>%p;]><r/>`.
- H: `<!DOCTYPE r [<!ENTITY % p "<!ENTITY e 'x'>">%p;]><r>&e;</r>`.

Body raw/decoded slots are 16 bytes and local slots initially 16. Reference slots are 16. All other dimensions are generous unless listed. Decimal thresholds count bytes, including both bytes of `é`; quoted `&#13;` produces one CR in the parameter replacement.

| Family | Fixture and rejection/control ceilings | Frozen native error | Physical anchor / high-water / latest delivered range |
| --- | --- | --- | --- |
| Scanner unit | T, 11 / 12 | Dtd Scan UnitBytes | 11 / 11 / none |
| Scanner scratch | T, 11 / 12 | Dtd Scan ScratchBytes | 11 / 11 / none |
| Scanner work | T, 11 / 12 | Dtd Scan Work | 11 / 11 / none |
| Unit tokens | T, 0 / 1 | Dtd Grammar Tokens | 10 / 12 / none |
| Unit model depth | M, 0 / 1 | Dtd Grammar ModelDepth | 25 / 29 / 0..13 |
| Unit attribute definitions | A, 0 / 1 | Dtd Grammar AttributeDefinitions | 25 / 42 / 0..13 |
| Catalog storage | V, 2 / 3 | Dtd Catalog StorageBytes | 13 / 29 / 0..13 |
| Catalog value | V, 1 / 2 | Dtd Catalog ValueBytes | 25 / 29 / 0..13 |
| Catalog expansion across handoff | V, 3 / 4 | Content Tag Catalog ExpandedBytes | 31 / 43 / 29..31 |
| Catalog work | T, 0 / generous | Dtd Catalog Work | 0 / 12 / none |
| Frame active depth | N, 1 / 2 | Content Frame ActiveDepth | 50 / 53 / 47..50 |
| Frame work | G, 4 / 5 | Content Frame Work | 36 / 39 / 33..36 |
| Frame work across PE→GE handoff | H, 17 / 18 | Content Frame Work | 52 / 55 / 49..52 |
| Global Text work | T, 0 / generous | Content Text Work at body construction | 12 / 12 / 11..12 |
| Reference bytes | G, 2 / 3 | Content Text Reference Bytes | 38 / 39 / 33..36 |
| Reference scratch | G, 2 / 3 | Content Text Reference ScratchBytes | 38 / 39 / 33..36 |
| Reference work | G, 8 / 9 | Content Text Reference Work | 36 / 39 / 33..36 |
| Reference work across two inclusions | R, 9 / 18 | Content Text Reference Work | 36 / 39 / generated x from first inclusion |
| Aggregate scratch | T, 34 / 35 with 16+16+3 slots | Content Text Scratch after End | 12 / 12 / 11..12 |
| Input scratch slot | T, 15 / 16 | Content Text Scratch after End | 12 / 12 / 11..12 |
| Replacement scratch slot | T, 15 / 16 | Content Text Scratch after End | 12 / 12 / 11..12 |
| Local token | G, 3 / 4 | Content Text Local Xml TokenBytes | 36 / 39 / 33..36 |
| Local scratch | G, 3 / 4 | Content Text Local Xml DecodedTokenBytes | 36 / 39 / 33..36 |
| Local work | G, 7 / 8 | Content Text Local Work | 36 / 39 / 33..36 |
| Declaration generated-owned | P, 9 / 10 | Dtd OwnedBytes before generated Item | 43 / 46 / 13..43 |
| Declaration aggregate work | T, 0 / generous | Dtd Work | 0 / 0 / none |

The plan's baseline may expose inaccurate inherited location assumptions or a production defect. Any discrepancy must be retained as red evidence and resolved from an independent lower-layer contract or an explicit policy decision; it must not be silently changed to match a facade observation.

Additional cumulative controls: H performs one PE and one GE invocation, preserving two reference attempts and 18 native frame work units; V spends two expansion bytes constructing the entity before two more bytes expanding its attribute use; R must not reset reference work between its two inclusions. Monotonic aggregate counters and exact sticky snapshots supplement these literal discriminators.

## Explicit errata after the frozen baseline

The original table is preserved above. Its first facade run was 21 passed and 5 failed; the raw log is `/private/tmp/yaxml-dtd-reader-quota-baseline.log`. All native error kinds and physical anchors agreed. Two independently authored lower-component controls were then run before changing expectations; both passed, without any production edit:

- `independent_scanner_header_keeps_its_physical_delimiter_before_publication` pins the scanner's rejected no-subset header unit to `0..12`, including `>`. Only successful facade publication separates Begin `0..11` and held End `11..12`. Therefore unit-token and catalog-work **error** origins are `0..12`, rather than the successful Begin span used initially.
- `independent_reference_limits_stop_at_the_first_unaccepted_source_byte` pins byte/scratch ceilings of two on `&e;` to accepted position two, and exhausted cumulative reference work to zero newly accepted bytes of the second reference. This agrees with the existing native monotone-work and reference-copy-before-skip specifications. Therefore the reference Bytes/Scratch facade high-water is **38**, and the two-inclusion native-work high-water is **36**; their physical anchors remain unchanged.

The independent control log is `/private/tmp/yaxml-dtd-reader-quota-independent-controls.log`. No expected type, budget threshold, logical value, physical diagnostic anchor, replacement identity or sticky-error requirement was relaxed. These five corrections distinguish accepted primary high-water from bytes merely present in the caller's remaining input.

## Second frozen supplement

These rows are frozen separately after the corrected 26-row tranche, before executing the added facade cases. They reconcile the remaining seven configurable fields with the native limit inventory and strengthen exact work boundaries. The shared 47/48 and native catalog 11/12 thresholds were supplied independently by the architecture reconciliation, rather than measured from this facade.

| Family | Fixture rejection / success | Frozen native cause | Anchor / high-water / latest range |
| --- | --- | --- | --- |
| Unit byte limit | T 11 / 12 | Dtd Grammar UnitBytes | 11 / 12 / none |
| Catalog declarations | T 0 / 1 | Dtd Catalog Declarations | 0 / 12 / none |
| Catalog bindings | V 0 / 1 | Dtd Catalog Bindings | 13 / 29 / 0..13 |
| Catalog reference attempts | V 0 / 1 | Content Tag Catalog References | 37 / 43 / 29..31 |
| Catalog active expansion depth | V 0 / 1 | Content Tag Catalog Depth | 37 / 43 / 29..31 |
| Frame reference attempts | G 0 / 1 | Content Frame ReferenceAttempts | 36 / 39 / 33..36 |
| Controller physical bytes during declarations | T 11 / 16 | Dtd PhysicalBytes | 11 / 11 / none |
| Controller physical bytes during body | T 15 / 16 | Content PhysicalBytes | 15 / 15 / 11..12 |
| Native catalog work exact | T 11 / 12 | Dtd Catalog Work | 0 / 12 / none |
| Shared declaration work exact | T 47 / 48 | Content Work | 12 / 16 / 11..12 |
| Shared Text work exact | T 47 / 48 | Content Work | 12 / 16 / 11..12 |

No-subset rejected grammar/catalog header origins include the complete scanner unit `0..12`. Body tag failures retain `12..16`; a rejected ordinary value reference retains the attribute location `Input(37)` and root raw origin `31..43`. Shared work is 36 at completed DTD End, 44 after root scan/validation, and 48 only after accepted tag processing. A 47 ceiling must retain 44, rather than debit rejected work or reset the DTD spend. As in the first tranche, any discrepancy is retained as a red baseline and resolved independently.
