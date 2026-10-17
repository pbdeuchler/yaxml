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

The supplement's first run was 37 passed, 2 failed, retained at `/private/tmp/yaxml-dtd-reader-quota-supplement-baseline.log`. Only the assumed shared-work error origin differed: the direct lower ContentController control `independent_content_work_rejection_has_an_anchor_before_tag_publication` proves both policies reject before tag publication with physical anchor 12 and **no error origin**, while retaining accepted position 16 and work 44. The facade must preserve that absent error origin and the latest delivered End `11..12`; it must not manufacture a rejected tag origin. This independently checked correction supersedes the sentence above claiming those two work failures retain `12..16` as an error origin. Other root tag failures, such as catalog value/reference policy rejection, continue to retain their actual whole-tag error origin. The lower control log is `/private/tmp/yaxml-dtd-reader-quota-supplement-control.log`.

The existing blind group `reader::dtd_spec_tests::non_bmp_replacement_work_counts_utf8_bytes_instead_of_scalars` already supplies the frame-expanded-byte 3/4/5 boundary; this supplement does not duplicate it. Together with that existing group, the new matrix covers every scalar field in `ControllerLimits`, its scanner/unit/catalog/frame components, `TextLimits`/reference/aggregate scratch, and `LocalLimits`.

Final negative-control check: temporarily substituting each row's success ceiling for its rejection ceiling made all 37 quota tests fail at their rejection oracle while the three independent component controls remained green. This was a test-only fixture mutation, not a production-guard mutation or a claim of complete mutation coverage. The original setter was restored byte-for-byte. The red log is `/private/tmp/yaxml-dtd-reader-quota-raised-controls-red.log`.

## Third frozen supplement: interactions and parser complements

Frozen before executing the cases below. The following literal fixtures supplement T/V/P/H: U is `<!DOCTYPE r><r a='éé'/>` (25 UTF-8 bytes); D is `<!DOCTYPE r [<!ENTITY e '<n><m/></n>'>]><r>&e;</r>` (50); L is `<!DOCTYPE r [<!ENTITY e '&#38;inner;'><!ENTITY inner 'I'>]><r>&e;</r>` (69); N5 is `<!DOCTYPE r [<!ENTITY e '&#38;#65;'>]><r>&e;</r>` (48); S prepends `<?p a\r\n?>` (literal CR/LF, nine bytes) to P (61). G16/N16 use the entity declaration from R with body `1234567890123456&e;` / `1234567890123456&#65;` (56/58).

| Complement | Rejection / complete success | Frozen diagnostic and location |
| --- | --- | --- |
| BOM raw/decoded constructors | BOM+T: raw18/19, decoded15/16 | InputBytes at0 wins when both fail; otherwise DecodedBytes at0. Exact19/16 accepts; into_inner preserves BOM. Both constructor errors precede bad Session scan-work0; exact constructor then lazy scan-work0 fails at3, no delivered origin. |
| Physical raw vs decoded body token | U raw12/13 and decoded12/13 separately; tie12/12 | Content Text Xml TokenBytes / DecodedTokenBytes; raw wins tie; anchor12, high-water24, latest End11..12, no error origin. |
| Generated parser depth | D depth2/3 | Content Tag Parser Depth at43, generated m local3..7, primary43..46; high-water46, latest generated n local0..3. |
| Scanner work across PE | P work52/58 | Dtd Scan Work at43, generated-local6, partial origin local0..6 with primary43..46; high-water46, latest physical declaration13..43. |
| Scanner work after generated unit | P work57/58 | Dtd Scan Work at47, origin input46..47; high-water47, latest generated comment local0..10/primary43..46. |
| Local named reference | L native reference bytes6/7 | Content Text Reference Bytes at62, generated-local6, zero-width local6..6/primary62..65; high-water65, latest root59..62. |
| Local numeric reference | N5 native reference bytes4/5 | Same Bytes cause at41, generated-local4, zero-width local4..4/primary41..44; high-water44, latest root38..41. |
| General versus numeric text token | G16 raw/decoded16 accepts two Text events (16-byte prefix, x); N16 raw16/21 rejects/accepts | N16 TokenBytes at33, high-water54, no error origin, latest root30..33; exact21 emits one Text ending A. General inclusion must not combine the preceding Text quota with the replacement. |
| Effective reference scratch minimum | T reference byte/scratch7/3 and3/7, aggregate34/35 | Content Text Scratch at12, high-water12, latest End11..12; reserve minimum3, not maximum7. |
| Checked aggregate scratch | T input slot, replacement slot, or both reference limits usize::MAX versus16; aggregate usize::MAX | Content Text ScratchOverflow at12, high-water12, latest End11..12, no allocation attempt or panic. |
| Native/global catalog collision | T native11 with aggregate35 versus36 | Dtd aggregate Work wins equal remaining11, at0 with whole header0..12/value Declaration(0,0); high-water12. Raising aggregate36 leaves native Catalog Work. Both generous native12/global36 accept. |
| Global-work/scratch collision | T Text work0 with input slot usize::MAX versus normal work/slot16 | Work precedes ScratchOverflow after End at12, latest End11..12. |
| Shared parser normalization/promotion | S Parser owned12/13 | Prior PI logical data a+LF costs3; generated DTD comment promotion costs10. Dtd Ownership OwnedBytes at52, generated local0..10/primary52..55, local0; high-water55, latest physical declaration22..52. Prior spend must survive handoff; failed promotion does not debit parser/native ownership. |
| Cumulative catalog declarations | V declarations1/2 | Header already spends one. Dtd Catalog Declarations at13/origin13..29/value Declaration(1,0); high-water29, latest Begin0..13. |
| PE→GE reference attempts | H attempts1/2 | Content Frame ReferenceAttempts at52/input52..55, high-water55, latest root49..52. Failed second attempt leaves attempts1/work16/expanded15; complete control has2/18/16. |

The native/global tie rule follows the accepted lower controller's explicit aggregate-cap attribution (`ceiling <= native`), not a new facade measurement. Tests retain exact typed errors and sticky counters. Any location disagreement remains a red baseline and requires independent lower-level evidence before correction.

Already-covered complements are cited rather than duplicated: `attribute_depth_input_and_decoded_ceilings_are_inclusive` pins final explicit+default attributes and ordinary parser depth; `exact_borrowed_owned_and_event_budgets_do_not_inherit_streaming_copy_costs` pins combined DTD/generated event counts and normalized body ownership; `pre_dtd_declaration_misc_event_and_owned_budgets_survive_handoff` pins XML declaration/PI event continuity and PI+body shared ownership. Existing exact catalog expansion and frame-work handoff rows provide Q3/Q4 cumulative complements; the new declaration/attempt rows discriminate reset at the specific PE→GE seam.
