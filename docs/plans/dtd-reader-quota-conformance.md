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
