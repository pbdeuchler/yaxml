use super::*;

type CapturedUnit = (ScanKind, Vec<u8>, Range<usize>);

fn limits() -> ScanLimits {
    ScanLimits {
        max_unit_bytes: 4096,
        max_scratch_bytes: 4096,
        max_work: 65536,
    }
}

fn collect(mode: ScanMode, input: &[u8], chunks: &[usize]) -> Result<Vec<CapturedUnit>, ScanError> {
    collect_with_limits(mode, input, chunks, limits())
}

fn collect_with_limits(
    mode: ScanMode,
    input: &[u8],
    chunks: &[usize],
    ceilings: ScanLimits,
) -> Result<Vec<CapturedUnit>, ScanError> {
    let mut scanner = DtdScanner::new(mode, ceilings);
    let mut units = Vec::new();
    let mut position: usize = 0;
    for length in chunks.iter().copied().chain(std::iter::once(input.len())) {
        let end = position.saturating_add(length).min(input.len());
        while position < end && !scanner.is_complete() {
            let consumed = scanner.advance(&input[position..end])?;
            assert!(consumed != 0 || scanner.current().is_some());
            position += consumed;
            if let Some((kind, bytes, range)) = scanner.current() {
                units.push((kind, bytes.to_vec(), range));
                scanner.release();
            }
        }
        if scanner.is_complete() {
            break;
        }
    }
    loop {
        scanner.finish_step()?;
        if let Some((kind, bytes, range)) = scanner.current() {
            units.push((kind, bytes.to_vec(), range));
            scanner.release();
        } else {
            assert!(scanner.is_complete());
            assert_eq!(scanner.work(), scanner.position());
            return Ok(units);
        }
    }
}

#[test]
fn literal_units_keep_quotes_delimiters_and_local_spans_at_every_split() {
    let parts: &[(ScanKind, &str)] = &[
        (ScanKind::Header, "<!DOCTYPE é SYSTEM 'a[>b' ["),
        (ScanKind::Whitespace, "\r\n "),
        (ScanKind::Declaration, "<!ENTITY e '<a/>🙂>tail'>"),
        (ScanKind::Comment, "<!-- ' > ] %x; -->"),
        (ScanKind::Instruction, "<?p ' > ]'??>"),
        (ScanKind::ParameterReference, "%é;"),
        (ScanKind::Whitespace, "\t"),
        (ScanKind::End, "] \r\n>"),
    ];
    let input = parts.iter().map(|(_, text)| *text).collect::<String>() + "<r/>";
    let mut offset = 0;
    let expected: Vec<_> = parts
        .iter()
        .map(|(kind, text)| {
            let range = offset..offset + text.len();
            offset = range.end;
            (*kind, text.as_bytes().to_vec(), range)
        })
        .collect();
    for split in 0..=input.len() {
        assert_eq!(
            collect(ScanMode::Doctype, input.as_bytes(), &[split]).unwrap(),
            expected,
            "split={split}"
        );
    }
    assert_eq!(
        collect(ScanMode::Doctype, input.as_bytes(), &vec![1; input.len()]).unwrap(),
        expected
    );
}

#[test]
fn ready_and_completed_units_leave_the_document_suffix_untouched() {
    for input in ["<!DOCTYPE r><r/>", "<!DOCTYPE r SYSTEM 'a>b'><r/>"] {
        let boundary = input.find("<r/>").unwrap();
        let mut scanner = DtdScanner::new(ScanMode::Doctype, limits());
        assert_eq!(scanner.current(), None);
        scanner.release();
        assert_eq!(scanner.advance(&[]), Ok(0));
        assert_eq!(scanner.advance(input.as_bytes()), Ok(boundary));
        assert!(scanner.is_complete());
        assert_eq!(scanner.position(), boundary);
        assert_eq!(
            scanner.current(),
            Some((ScanKind::Header, &input.as_bytes()[..boundary], 0..boundary))
        );
        assert_eq!(scanner.advance(b"<r/>"), Ok(0));
        scanner.finish_step().unwrap();
        assert!(scanner.current().is_some());
        scanner.release();
        scanner.release();
        assert_eq!(scanner.current(), None);
        assert_eq!(scanner.advance(b"<r/>"), Ok(0));
        scanner.finish_step().unwrap();
        assert_eq!(scanner.position(), boundary);
    }
}

#[test]
fn fragment_eof_drains_whitespace_without_normalizing_cr_or_unicode() {
    let input = "<!ENTITY é 'a\r\nb\rc'>%ns:é;\r\n\t ";
    let first = "<!ENTITY é 'a\r\nb\rc'>";
    let second = "%ns:é;";
    let expected = vec![
        (
            ScanKind::Declaration,
            first.as_bytes().to_vec(),
            0..first.len(),
        ),
        (
            ScanKind::ParameterReference,
            second.as_bytes().to_vec(),
            first.len()..first.len() + second.len(),
        ),
        (
            ScanKind::Whitespace,
            b"\r\n\t ".to_vec(),
            first.len() + second.len()..input.len(),
        ),
    ];
    for split in 0..=input.len() {
        assert_eq!(
            collect(ScanMode::SubsetFragment, input.as_bytes(), &[split]).unwrap(),
            expected
        );
    }
    assert_eq!(collect(ScanMode::SubsetFragment, b"", &[]).unwrap(), vec![]);
    assert_eq!(
        collect(ScanMode::SubsetFragment, b" \t\r\n", &[1, 1, 1, 1]).unwrap(),
        vec![(ScanKind::Whitespace, b" \t\r\n".to_vec(), 0..4)]
    );
}

#[test]
fn release_cannot_discard_a_partial_unit() {
    let mut scanner = DtdScanner::new(ScanMode::SubsetFragment, limits());
    assert_eq!(scanner.advance(b"<!ENTITY e '"), Ok(12));
    assert_eq!(scanner.current(), None);
    scanner.release();
    assert_eq!(scanner.advance(b"x>y'>"), Ok(5));
    assert_eq!(
        scanner.current(),
        Some((
            ScanKind::Declaration,
            b"<!ENTITY e 'x>y'>".as_slice(),
            0..17
        ))
    );
    scanner.release();
    scanner.finish_step().unwrap();
    assert!(scanner.is_complete());
}

#[test]
fn fragment_completion_requires_a_complete_unit_at_every_eof_prefix() {
    // These individual units have no valid interior EOF, including inside UTF-8.
    let samples = [
        "<!ELEMENT é EMPTY>",
        "<!ENTITY é '🙂>a'>",
        "<!-- 🙂 -->",
        "<?é 🙂?>",
        "%é;",
    ];
    for sample in samples {
        for end in 1..sample.len() {
            let bytes = &sample.as_bytes()[..end];
            let mut scanner = DtdScanner::new(ScanMode::SubsetFragment, limits());
            assert_eq!(scanner.advance(bytes), Ok(end));
            let expected = match std::str::from_utf8(bytes) {
                Ok(_) => ScanError {
                    kind: ScanErrorKind::UnexpectedEof,
                    offset: end,
                },
                Err(error) => ScanError {
                    kind: ScanErrorKind::InvalidUtf8,
                    offset: error.valid_up_to(),
                },
            };
            assert_eq!(
                scanner.finish_step(),
                Err(expected),
                "sample={sample:?} end={end}"
            );
            assert_eq!(scanner.advance(b"ignored"), Err(expected));
            assert_eq!(scanner.finish_step(), Err(expected));
            assert_eq!(scanner.position(), end);
        }
        assert_eq!(
            collect(
                ScanMode::SubsetFragment,
                sample.as_bytes(),
                &vec![1; sample.len()]
            )
            .unwrap()
            .len(),
            1
        );
    }
}

#[test]
fn doctype_requires_a_real_end_after_header_or_trailing_whitespace() {
    for sample in [
        "",
        "<",
        "<!DOCTYPE",
        "<!DOCTYPE r 'a>",
        "<!DOCTYPE r [",
        "<!DOCTYPE r [\r\n",
        "<!DOCTYPE r []",
        "<!DOCTYPE r [] \t",
    ] {
        let result = collect(ScanMode::Doctype, sample.as_bytes(), &vec![1; sample.len()]);
        assert_eq!(
            result,
            Err(ScanError {
                kind: ScanErrorKind::UnexpectedEof,
                offset: sample.len()
            }),
            "{sample:?}"
        );
    }
    let input = b"<!DOCTYPE r [<!ELEMENT r EMPTY>]><r/>";
    assert_eq!(
        collect(ScanMode::Doctype, input, &[1; 40]).unwrap().len(),
        3
    );
}

#[test]
fn structural_errors_have_exact_local_offsets() {
    let samples: &[(ScanMode, &[u8], usize)] = &[
        (ScanMode::Doctype, b"<!DoCTYPE r>", 3),
        (ScanMode::SubsetFragment, b"x", 0),
        (ScanMode::SubsetFragment, b"]>", 0),
        (ScanMode::SubsetFragment, b"<x>", 1),
        (ScanMode::SubsetFragment, b"<!-x>", 3),
        (ScanMode::SubsetFragment, b"<![INCLUDE[x]]>", 2),
        (ScanMode::SubsetFragment, b"%;", 1),
        (ScanMode::SubsetFragment, b"%1x;", 1),
        (ScanMode::SubsetFragment, b"%a b;", 2),
        (ScanMode::SubsetFragment, "%é!;".as_bytes(), 3),
        (ScanMode::Doctype, b"<!DOCTYPE r []x>", 14),
    ];
    for &(mode, input, offset) in samples {
        let expected = Err(ScanError {
            kind: ScanErrorKind::Syntax,
            offset,
        });
        for split in 0..=input.len() {
            assert_eq!(
                collect(mode, input, &[split]),
                expected,
                "input={input:?} split={split}"
            );
        }
    }
}

#[test]
fn completed_units_validate_utf8_once_and_report_the_invalid_sequence_start() {
    for (input, offset) in [
        (b"<!--\xff-->".as_slice(), 4),
        (b"<?p \xf0\x9f?>".as_slice(), 4),
        (b"<!ENTITY e '\xc3x'>".as_slice(), 12),
        (b"%\xff;".as_slice(), 1),
    ] {
        for split in 0..=input.len() {
            assert_eq!(
                collect(ScanMode::SubsetFragment, input, &[split]),
                Err(ScanError {
                    kind: ScanErrorKind::InvalidUtf8,
                    offset
                }),
                "input={input:?} split={split}"
            );
        }
    }
}

#[test]
fn lexical_framing_does_not_claim_complete_declaration_grammar_validation() {
    for input in [
        "<!UNKNOWN x>",
        "<!ENTITY e '\0'>",
        "<!-- a--b -->",
        "<?xml x?>",
    ] {
        let units = collect(ScanMode::SubsetFragment, input.as_bytes(), &[1; 32]).unwrap();
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].1, input.as_bytes());
        // The separate declaration-unit parser must reject these before events.
    }
}

#[test]
fn syntax_failure_is_sticky_even_after_release_and_eof() {
    let mut scanner = DtdScanner::new(ScanMode::SubsetFragment, limits());
    let error = ScanError {
        kind: ScanErrorKind::Syntax,
        offset: 4,
    };
    assert_eq!(scanner.advance(b"%bad!;"), Err(error));
    let position = scanner.position();
    scanner.release();
    assert_eq!(scanner.advance(b"<!ELEMENT r EMPTY>"), Err(error));
    assert_eq!(scanner.finish_step(), Err(error));
    assert_eq!(scanner.position(), position);
    assert_eq!(scanner.current(), None);
}

#[test]
fn final_eof_seals_input_while_ready_units_are_drained() {
    let mut scanner = DtdScanner::new(ScanMode::SubsetFragment, limits());
    assert_eq!(scanner.advance(b" \r"), Ok(2));
    scanner.finish_step().unwrap();
    assert!(!scanner.is_complete());
    assert_eq!(
        scanner.current(),
        Some((ScanKind::Whitespace, b" \r".as_slice(), 0..2))
    );
    assert_eq!(scanner.advance(b"%ignored;"), Ok(0));
    scanner.release();
    assert_eq!(scanner.advance(b"%ignored;"), Ok(0));
    assert_eq!(scanner.position(), 2);
    scanner.finish_step().unwrap();
    assert!(scanner.is_complete());

    // A ready non-whitespace unit also seals further input on final EOF.
    let mut scanner = DtdScanner::new(ScanMode::SubsetFragment, limits());
    scanner.advance(b"%ok;").unwrap();
    scanner.finish_step().unwrap();
    scanner.release();
    assert_eq!(scanner.advance(b"%ignored;"), Ok(0));
    scanner.finish_step().unwrap();
    assert!(scanner.is_complete());
}

#[test]
fn incomplete_parameter_reference_reports_known_name_errors_before_missing_semicolon() {
    for (input, kind, offset) in [
        ("%1", ScanErrorKind::Syntax, 1),
        ("%a b", ScanErrorKind::Syntax, 2),
        ("%é!", ScanErrorKind::Syntax, 3),
        ("%", ScanErrorKind::UnexpectedEof, 1),
        ("%é", ScanErrorKind::UnexpectedEof, 3),
    ] {
        for split in 0..=input.len() {
            assert_eq!(
                collect(ScanMode::SubsetFragment, input.as_bytes(), &[split]),
                Err(ScanError { kind, offset })
            );
        }
    }
    assert_eq!(
        collect(ScanMode::SubsetFragment, b"%a\xc3", &[1]),
        Err(ScanError {
            kind: ScanErrorKind::InvalidUtf8,
            offset: 2
        })
    );
}

#[test]
fn each_resource_ceiling_fails_at_the_first_unconsumed_byte() {
    let input = "%é;".as_bytes();
    for ceiling in 0..=input.len() {
        for kind in [
            ScanLimit::UnitBytes,
            ScanLimit::ScratchBytes,
            ScanLimit::Work,
        ] {
            let mut ceilings = limits();
            match kind {
                ScanLimit::UnitBytes => ceilings.max_unit_bytes = ceiling,
                ScanLimit::ScratchBytes => ceilings.max_scratch_bytes = ceiling,
                ScanLimit::Work => ceilings.max_work = ceiling,
            }
            for split in 0..=input.len() {
                let result =
                    collect_with_limits(ScanMode::SubsetFragment, input, &[split], ceilings);
                if ceiling == input.len() {
                    assert_eq!(
                        result.unwrap(),
                        vec![(ScanKind::ParameterReference, input.to_vec(), 0..input.len())]
                    );
                } else {
                    assert_eq!(
                        result,
                        Err(ScanError {
                            kind: ScanErrorKind::Limit(kind),
                            offset: ceiling
                        }),
                        "kind={kind:?} ceiling={ceiling} split={split}"
                    );
                }
            }
        }
    }
}

#[test]
fn resource_error_precedence_and_failure_position_do_not_depend_on_chunking() {
    for (ceilings, kind) in [
        (
            ScanLimits {
                max_unit_bytes: 0,
                max_scratch_bytes: 0,
                max_work: 0,
            },
            ScanLimit::Work,
        ),
        (
            ScanLimits {
                max_unit_bytes: 0,
                max_scratch_bytes: 0,
                max_work: 1,
            },
            ScanLimit::UnitBytes,
        ),
        (
            ScanLimits {
                max_unit_bytes: 1,
                max_scratch_bytes: 0,
                max_work: 1,
            },
            ScanLimit::ScratchBytes,
        ),
    ] {
        let mut scanner = DtdScanner::new(ScanMode::SubsetFragment, ceilings);
        let error = ScanError {
            kind: ScanErrorKind::Limit(kind),
            offset: 0,
        };
        assert_eq!(scanner.advance(b"%a;"), Err(error));
        assert_eq!(scanner.position(), 0);
        scanner.release();
        assert_eq!(scanner.advance(b""), Err(error));
        assert_eq!(scanner.finish_step(), Err(error));
    }
}

#[test]
fn work_is_cumulative_across_releases_but_unit_and_scratch_bounds_are_not() {
    let input = b"%a;%b;";
    for work in 0..=input.len() {
        let ceilings = ScanLimits {
            max_unit_bytes: 3,
            max_scratch_bytes: 3,
            max_work: work,
        };
        for split in 0..=input.len() {
            let result = collect_with_limits(ScanMode::SubsetFragment, input, &[split], ceilings);
            if work == input.len() {
                assert_eq!(
                    result.unwrap(),
                    vec![
                        (ScanKind::ParameterReference, b"%a;".to_vec(), 0..3),
                        (ScanKind::ParameterReference, b"%b;".to_vec(), 3..6),
                    ]
                );
            } else {
                assert_eq!(
                    result,
                    Err(ScanError {
                        kind: ScanErrorKind::Limit(ScanLimit::Work),
                        offset: work
                    })
                );
            }
        }
    }
}

#[test]
fn whitespace_and_quoted_units_obey_the_same_limits() {
    for input in [b" \t\r\n".as_slice(), b"<!ENTITY e '>>>'>".as_slice()] {
        let maximum = input.len() - 1;
        for kind in [ScanLimit::UnitBytes, ScanLimit::ScratchBytes] {
            let mut ceilings = limits();
            match kind {
                ScanLimit::UnitBytes => ceilings.max_unit_bytes = maximum,
                ScanLimit::ScratchBytes => ceilings.max_scratch_bytes = maximum,
                ScanLimit::Work => unreachable!(),
            }
            assert_eq!(
                collect_with_limits(ScanMode::SubsetFragment, input, &[1; 32], ceilings),
                Err(ScanError {
                    kind: ScanErrorKind::Limit(kind),
                    offset: maximum
                })
            );
        }
    }
}

#[test]
fn many_small_units_reuse_bounded_scratch_without_retaining_the_subset() {
    let unit = b"<!ENTITY e 'x'>";
    let count = 4096;
    let ceilings = ScanLimits {
        max_unit_bytes: unit.len(),
        max_scratch_bytes: unit.len(),
        max_work: unit.len() * count,
    };
    let mut scanner = DtdScanner::new(ScanMode::SubsetFragment, ceilings);
    let mut capacity = None;
    for ordinal in 0..count {
        for byte in unit {
            assert_eq!(scanner.advance(std::slice::from_ref(byte)), Ok(1));
        }
        let start = ordinal * unit.len();
        assert_eq!(
            scanner.current(),
            Some((
                ScanKind::Declaration,
                unit.as_slice(),
                start..start + unit.len()
            ))
        );
        let retained = scanner.buffer.capacity();
        assert!(retained >= unit.len());
        if let Some(previous) = capacity {
            assert_eq!(retained, previous);
        }
        capacity = Some(retained);
        scanner.release();
        assert!(scanner.buffer.is_empty());
        assert_eq!(scanner.position(), (ordinal + 1) * unit.len());
        assert_eq!(scanner.work(), scanner.position());
    }
    scanner.finish_step().unwrap();
    assert!(scanner.is_complete());
}

#[test]
fn every_doctype_eof_prefix_reports_its_own_decoded_local_position() {
    let input = "<!DOCTYPE é SYSTEM 'a[>b' [\r\n<!ENTITY e '🙂>x'>%é;<!--é--><?é?>] \t>";
    for end in 0..input.len() {
        let prefix = &input.as_bytes()[..end];
        let expected = match std::str::from_utf8(prefix) {
            Ok(_) => ScanError {
                kind: ScanErrorKind::UnexpectedEof,
                offset: end,
            },
            Err(error) => ScanError {
                kind: ScanErrorKind::InvalidUtf8,
                offset: error.valid_up_to(),
            },
        };
        assert_eq!(
            collect(ScanMode::Doctype, prefix, &[1; 100]),
            Err(expected),
            "end={end}"
        );
    }
    assert!(collect(ScanMode::Doctype, input.as_bytes(), &[1; 100]).is_ok());
}

#[test]
fn deterministic_literal_model_is_independent_of_partition_and_unit_order() {
    let catalog = [
        (ScanKind::Declaration, "<!ELEMENT é (a|b)*>"),
        (ScanKind::Declaration, "<!ATTLIST é a CDATA '>>'>"),
        (ScanKind::Declaration, "<!ENTITY e '<a/>🙂>'>"),
        (ScanKind::Declaration, "<!NOTATION n SYSTEM 'a>b'>"),
        (ScanKind::Comment, "<!-- \" > ] %a; -->"),
        (ScanKind::Instruction, "<?é ' > ]'??>"),
        (ScanKind::ParameterReference, "%ns:é;"),
    ];
    for seed in 0..24 {
        let mut model = vec![(ScanKind::Header, "<!DOCTYPE é [")];
        for step in 0..17 {
            model.push(catalog[(seed * 13 + step * 7 + step * step) % catalog.len()]);
            model.push((
                ScanKind::Whitespace,
                if step % 2 == 0 { "\r\n" } else { " \t" },
            ));
        }
        model.push((ScanKind::End, "] >"));
        let input: String = model.iter().map(|(_, raw)| *raw).collect();
        let mut start = 0;
        let expected: Vec<_> = model
            .iter()
            .map(|(kind, raw)| {
                let range = start..start + raw.len();
                start = range.end;
                (*kind, raw.as_bytes().to_vec(), range)
            })
            .collect();
        for width in [1, 2, 3, 7, 31, 512] {
            let chunks = vec![width; input.len() / width + 1];
            assert_eq!(
                collect(ScanMode::Doctype, input.as_bytes(), &chunks).unwrap(),
                expected,
                "seed={seed} width={width}"
            );
        }
        for split in 0..=input.len() {
            assert_eq!(
                collect(ScanMode::Doctype, input.as_bytes(), &[split]).unwrap(),
                expected,
                "seed={seed} split={split}"
            );
        }
    }
}

#[test]
fn fragment_restart_reuses_only_released_complete_boundaries_and_preserves_work_cap() {
    let mut scanner = DtdScanner::new(ScanMode::SubsetFragment, limits());
    scanner.cap_work(8);
    scanner.cap_work(usize::MAX);
    assert_eq!(scanner.pending(), (&b""[..], 0..0));
    assert_eq!(scanner.advance(b"<!--x-->"), Ok(8));
    assert_eq!(scanner.pending(), (&b"<!--x-->"[..], 0..8));
    scanner.release();
    scanner.finish_step().unwrap();
    assert!(scanner.is_complete());
    scanner.restart_fragment().unwrap();
    assert_eq!(scanner.position(), 0);
    assert!(!scanner.is_complete());
    assert_eq!(scanner.advance(b"<!--x-->"), Ok(8));
    scanner.release();
    scanner.restart_fragment().unwrap();
    let error = scanner.advance(b"<!--xx-->").unwrap_err();
    assert_eq!(error.kind, ScanErrorKind::Limit(ScanLimit::Work));
    assert_eq!(scanner.position(), 8);
    assert_eq!(scanner.restart_fragment(), Err(error));
}

#[test]
fn fragment_restart_rejects_wrong_modes_ready_partial_and_final_whitespace_states() {
    for (mode, input) in [
        (ScanMode::Doctype, &b""[..]),
        (ScanMode::SubsetFragment, &b"<!--x-->"[..]),
        (ScanMode::SubsetFragment, &b"<!--"[..]),
        (ScanMode::SubsetFragment, &b" "[..]),
    ] {
        let mut scanner = DtdScanner::new(mode, limits());
        scanner.advance(input).unwrap();
        let error = scanner.restart_fragment().unwrap_err();
        assert_eq!(error.kind, ScanErrorKind::Syntax);
        assert_eq!(scanner.advance(b"<!--x-->"), Err(error));
    }
}
