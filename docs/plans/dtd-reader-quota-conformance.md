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

Third-supplement baseline and explicit errata: `/private/tmp/yaxml-dtd-reader-quota-third-baseline.log` retained 57 passes and four failures. The three U token failures agreed in native cause/anchor/origins, but the predicted high-water24 was inaccurate: the independent stable `TextCoordinator` control proves the accepted stable scanner retains the complete rejected 13-byte tag through25 before applying the token ceiling. Its separate log is `/private/tmp/yaxml-dtd-reader-quota-third-control.log`; high-water25 is now required. The BOM control accidentally used the public default 1MiB token size with a deliberately 16-byte Text scratch slot; explicitly selecting token16 corrects that fixture setup. Its subsequent red run is retained at `/private/tmp/yaxml-dtd-reader-quota-third-corrected.log`: zero-work scanning has no current token and therefore no error origin, rather than a fabricated empty Input span. The direct scanner zero-work control independently pins that absent token/origin. No native cause, budget boundary, logical value, diagnostic anchor or latest-delivered origin was weakened.

The collision table's global36 is sufficient only for DTD completion, not complete T parsing (which requires48, as already frozen above). Paired full-document controls therefore use global48/native12. Distinct rejection cases pin global35/native11 to aggregate Work and global36/native11 to native Catalog Work; this preserves the frozen tie and one-above attribution exactly.

The completed focused matrix has 57 paired quota cases and six independent/control groups. A repeated test-only success-ceiling substitution makes all 57 rejection oracles fail while the six controls pass (`/private/tmp/yaxml-dtd-reader-quota-third-raised-controls-red.log`). The source is restored byte-for-byte afterward. This checks the table's discrimination; it is not a production mutation or a claim of exhaustive interaction coverage.

## Fourth frozen supplement: explicit Q1–Q8 closure

Authoritative checklist: accepted `docs/plans/dtd-reader-quota-facade.md`, “Smallest complete test implementation”. Freeze these additional literal profiles before executing them. F denotes `<!DOCTYPE r [<!ENTITY e 'x'>]><r>&e;</r>` (40 bytes, DTD end30, Start30..33, inclusion33..36). Independent action journal: DTD87 = scan30 + grammar28 + catalog29; Start adds9→96; reference adds9→105; frame entry1→106; local scan/validate2→108; frame consume1→109; semantic Text1→110; End12→122. Architecture supplied this journal independently of facade execution.

RP is `<!DOCTYPE r [<!ENTITY % p '<!--x-->'>%p;%p;]><r/>` (49): physical declaration13..37, references37..40/40..43, End43..45, root45..49. Scanner work45+8+8=61; promotion8+8=16. PE-model is `<!DOCTYPE r [<!ENTITY % p '<!ELEMENT r (a)>'>%p;]><r/>` (54): reference45..48, generated unit0..16, opening parenthesis12. PE-bad replaces that unit by `<!ELEMENT r>` (12), reference41..44, syntax error11. Empty is `<!DOCTYPE r [<!ENTITY e '&z;'><!ENTITY z ''>]><r>&e;</r>` (56), reference49..52 and root46..49; nested entry costs1 + active comparison1 + spelling3, after outer entry1, so native attempts2/work6/expanded3 despite no Text.

| Missing discriminator | Rejection / control and literal expected failure |
| --- | --- |
| Q1 DTD-free inert settings | All three constructors on `<r/>` with all Session fields0 and with overflowing scratch slots; exact ordinary Empty/EOF/origin0..4, even when Session could not construct a DTD body. Whole-input3 must still reject constructor InputBytes0 before those settings. |
| Q2 repeated scanner | RP native work58/61: D Scan Work at40, replacement declaration0/invocation2 local0..7, local7, physical40..43; high-water43, latest generated first comment local0..8/37..40. |
| Q2 generated semantic unit | PE-model model-depth0/1: D Grammar ModelDepth at45, replacement0/inv1 local0..16/45..48, local12; high-water48, latest physical declaration13..45. |
| Q3 stable native values | `<!DOCTYPE r><r a='v'/>`: ValueBytes0/1, ExpandedBytes0/1, native Work13/14; C Tag Catalog cause at18/value Input18, raw12..22, high-water22, latest End11..12. |
| Q3 read default | `<!DOCTYPE r [<!ATTLIST r a CDATA 'v'>]><r/>`: ValueBytes0/1, error D Catalog at34/value Declaration(1,21), raw13..37, high-water37, latest Begin0..13; success delivers default v. |
| Q3 tokenized pre-collapse | `<!DOCTYPE r [<!ATTLIST r a NMTOKENS #IMPLIED>]><r a=' v  '/>`: value ceiling1; expanded3/4, C Tag Catalog ExpandedBytes at56/value Input56, raw47..60, high-water60, latest End45..47; success value v. |
| Q3 indirect context/cumulative spend | `<!DOCTYPE r [<!ENTITY e '&z;'><!ENTITY z 'xx'>]><r a='&e;'/>`: expanded6/7 (declarations3+2, use2); error C Tag Catalog ExpandedBytes at48/value Replacement(entity1,1), raw48..60, high-water60, latest End46..48. |
| Q4 empty nested quotas | Empty attempts1/2, native work5/6, expanded2/3. Error C Frame native kind at49, replacement outer0/inv1 local0..3/49..52, local0; high-water52, latest Start46..49. Success no Text, final native2/6/3. |
| Q5 repeated local work | G with two consecutive inclusions (46 bytes): local work15/16; C Text Local Work at39, second invocation2 local0..0/39..42, local0; high-water42, latest first generated n0..4/36..39. |
| Q7 scanner competitor | T native/global Work11/11 gives aggregate D Work at11, no error origin, high-water11, no prior event; native11 with generous global already has scanner-native row. Complete control native12/global48. |
| Q7 frame competitors | F native0/global105 gives C Work, global106 gives C Frame Work; anchor33, input33..36, high-water36/latest Start30..33. Complete native2/global122. |
| Q7 reference competitors | F native8/global104 gives C Work, global105 gives C Text Reference Work; anchor33, no origin, high-water36/latest Start. Complete native9/global122. |
| Q7 local competitors | F native1/global107 gives C Work, global108 gives C Text Local Work; anchor33, replacement0/inv1 local0..0/33..36, local0, high-water36/latest Start. Complete native2/global122. |
| Q7 inherited cap/no reopening | T Text35: body-constructor C Text Work after End; Text36: active C Work before body progress. Exact full48 and one-above49 accept. Existing declaration47/Text-generous row proves Text cannot raise inherited cap. |
| Q8 repeated promotion/no refund | RP raw-controller owned15/16 and Parser owned15/16 independently; second comment error at40, replacement0/inv2 local0..8/40..43, local0, high-water43, latest first comment. Controller kind D Limit OwnedBytes; Parser kind D Ownership OwnedBytes. Retain and drop earlier units separately; prior spend8 never refunded. |
| Q8 scanner collision | T scan Work0 with scanner unit/scratch0 gives D Scan Work at0, no origin/progress; normal limits accept. |
| Q8 unit collision | T grammar UnitBytes11 with Tokens0 gives UnitBytes at11/raw0..12/high-water12/no prior event; normal12/1 accepts. |
| Q8 malformed before copy | PE-bad with each owned allowance0, separately and together, must give D Grammar Syntax at41/local11, whole replacement0..12/41..44, high-water44, latest physical declaration13..41. Raising ownership cannot make malformed grammar acceptable or change the first error. |
| Q8 Reference Work before growth | F reference Work/Bytes/Scratch all0: Reference Work at33, no origin, high-water33/latest Start; normal9/3/3 accepts. |
| Q8 Expanded before Value | Stable explicit v with both native Expanded/Value0 returns ExpandedBytes before ValueBytes at18, same raw/root/value context above;1/1 accepts. |
| Q8 semantic error before copy | `<!DOCTYPE r><r a='v&z;'/>`, Parser owned0: missing declaration at19/value Input19, raw12..26/high-water26/latest End11..12. External `<!DOCTYPE r SYSTEM 'x'>` prefix gives unavailable UnreadEntity at30/value Input30, raw23..37/high-water37/latest End22..23. Matching v prefix cannot bypass validation, and neither error may become deferred allocation failure. |

Q1 UTF-16 rejection/ordinary cursor/three-constructor controls remain `constructors_reject_utf16_and_invalid_whole_input_exactly_like_public_reader`, `ordinary_documents_match_public_reader_positions_values_origins_and_lifetimes`, and `each_constructor_is_lazy_beyond_preflight_and_returns_original_bytes`. Q2 native unit/scratch/bytes/tokens/attributes/physical model rows remain the first two supplements; repeated work and generated ModelDepth above close the semantic complements. Q3 PE-created binding and cumulative expansion are already discriminated by `frame_attempts_handoff`, `frame_work_handoff`, and `catalog_expanded_handoff`; new stable/default/tokenized/indirect rows close the remaining catalog contexts. Q4 non-BMP byte expansion is `non_bmp_replacement_work_counts_utf8_bytes_instead_of_scalars`, generated Parser depth is `generated_parser_depth`, attempts continuity is `frame_attempts_handoff`. Q5 native reference/local rows, replacement-coordinate rows and General/numeric segmentation remain named third-supplement cases; repeated local work closes source-switch continuity. Q6 existing independent16+16+3 slot sum, asymmetric minima and three overflow cases discriminate the same slot arithmetic as the illustrative16+8+4 profile without another numeric duplicate. Q7 exact47/48 and catalog native/global tie are existing cases; competitors above complete the other native kinds. Q8 physical borrowed0, body Text4/5/6, stable changed attribute63/64 and preamble-owned overlap remain the exact named facade groups cited in the third supplement; repeated promotion/collisions above close the additional contexts. No explicit Q1–Q8 behavior is deliberately deferred; foreign retained-tokenizer/UTF-16 scratch reconfiguration remains the checklist's explicit lower-layer-only exclusion, not a Reader seam.

Before executing a further Q6 complement, freeze its literal canonical profile too: T with physical16/local8/reference4 needs28 aggregate slots. Total27 rejects,28/29 accept; input slot15 rejects,16/17 accept with unrelated total enlarged to29; replacement slot7 rejects,8/9 accept with total29. Reference raw/scratch10/4 and4/10 still reserve4, hence total27 rejects and28/29 accept. Every rejection is Content Text Scratch at12/high-water12/no error origin/latest End11..12, sticky; controls complete T. This explicit numeric table supersedes the preceding proposed Q6 citation-only disposition.
