use super::*;

fn limits() -> ScanLimits {
    ScanLimits {
        max_unit_bytes: 4096,
        max_scratch_bytes: 4096,
        max_work: 65536,
    }
}

fn collect(
    mode: ScanMode,
    input: &[u8],
    chunks: &[usize],
) -> Result<Vec<(ScanKind, Vec<u8>, Range<usize>)>, ScanError> {
    let mut scanner = DtdScanner::new(mode, limits());
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
fn structural_errors_have_exact_local_offsets_and_are_sticky() {
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
