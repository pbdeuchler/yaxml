use crate::dtd::{DtdLimits, DtdUnit, DtdUnitView};

use super::*;

fn limits() -> ScanLimits {
    ScanLimits {
        max_unit_bytes: 256,
        max_scratch_bytes: 256,
        max_work: 4096,
    }
}

fn check_unit(scanner: &DtdScanner, expected: &[(ScanKind, &str)], next: &mut usize) {
    let (kind, bytes, range) = scanner.current().expect("one ready unit");
    let &(wanted_kind, wanted_raw) = expected.get(*next).expect("no extra unit");
    let start: usize = expected[..*next].iter().map(|(_, raw)| raw.len()).sum();
    assert_eq!(kind, wanted_kind);
    assert_eq!(bytes, wanted_raw.as_bytes());
    assert_eq!(range, start..start + wanted_raw.len());
    if matches!(
        kind,
        ScanKind::Header | ScanKind::Declaration | ScanKind::Comment | ScanKind::Instruction
    ) {
        let raw = std::str::from_utf8(bytes).expect("scanner validated the completed unit");
        let unit = DtdUnit::parse(raw, DtdLimits::default()).expect(raw);
        assert_eq!(unit.raw(), wanted_raw);
        match (kind, unit.view()) {
            (ScanKind::Header, DtdUnitView::Header(_))
            | (ScanKind::Declaration, DtdUnitView::Element(_))
            | (ScanKind::Declaration, DtdUnitView::AttributeList(_))
            | (ScanKind::Declaration, DtdUnitView::Entity(_))
            | (ScanKind::Declaration, DtdUnitView::Notation(_))
            | (ScanKind::Comment, DtdUnitView::Comment(_))
            | (ScanKind::Instruction, DtdUnitView::ProcessingInstruction(_)) => {}
            actual => panic!("framing/grammar family mismatch: {actual:?}"),
        }
    }
    *next += 1;
}

fn run(mode: ScanMode, expected: &[(ScanKind, &str)], chunks: &[&[u8]]) {
    let mut scanner = DtdScanner::new(mode, limits());
    let mut next = 0;
    for chunk in chunks {
        let mut offset = 0;
        while offset < chunk.len() {
            let consumed = scanner.advance(&chunk[offset..]).unwrap();
            assert!(consumed <= chunk.len() - offset);
            offset += consumed;
            if scanner.current().is_some() {
                check_unit(&scanner, expected, &mut next);
                scanner.release();
            } else {
                assert!(consumed > 0, "framing must make progress");
            }
        }
    }
    // A fragment ending in whitespace needs one call to emit it and another
    // call after release to validate final EOF. Keep the driver bounded.
    for _ in 0..=2 {
        scanner.finish_step().unwrap();
        if scanner.current().is_some() {
            check_unit(&scanner, expected, &mut next);
            scanner.release();
        } else {
            break;
        }
    }
    assert_eq!(next, expected.len());
    assert!(scanner.is_complete());
    let bytes: usize = expected.iter().map(|(_, raw)| raw.len()).sum();
    assert_eq!(scanner.position(), bytes);
    assert_eq!(scanner.work(), bytes);
}

#[test]
fn complete_internal_subset_units_feed_the_grammar_at_every_byte_boundary() {
    let units = [
        (ScanKind::Header, "<!DOCTYPE π ["),
        (ScanKind::Whitespace, "\r\n"),
        (ScanKind::Declaration, "<!ELEMENT π (a,(b|c)*)>"),
        (
            ScanKind::Declaration,
            "<!ATTLIST π x CDATA 'a>b' y (1|:x) #IMPLIED>",
        ),
        (ScanKind::Declaration, "<!ENTITY e '&#60;a/>tail'>"),
        (ScanKind::Declaration, "<!ENTITY % p '<!ELEMENT a EMPTY>'>"),
        (ScanKind::Declaration, "<!NOTATION n PUBLIC 'public-only'>"),
        (ScanKind::Comment, "<!-- ]> %p; <?x?> -->"),
        (ScanKind::Instruction, "<?build ]> <x> 'quoted?>"),
        (ScanKind::ParameterReference, "%p;"),
        (ScanKind::Whitespace, "\t"),
        (ScanKind::End, "] \r\n>"),
    ];
    let input: String = units.iter().map(|(_, raw)| *raw).collect();
    for split in 0..=input.len() {
        run(
            ScanMode::Doctype,
            &units,
            &[&input.as_bytes()[..split], &input.as_bytes()[split..]],
        );
    }
    let chunks: Vec<_> = input.as_bytes().chunks(1).collect();
    run(ScanMode::Doctype, &units, &chunks);
}

#[test]
fn fragment_pipeline_preserves_literal_cr_and_ends_only_between_units() {
    let units = [
        (ScanKind::Declaration, "<!ENTITY e 'a\r\nb&#13;'>"),
        (ScanKind::Whitespace, "\r"),
        (ScanKind::Declaration, "<!ATTLIST a x CDATA 'v\t\r\n'>"),
        (ScanKind::ParameterReference, "%名;"),
        (ScanKind::Whitespace, " \r\n\t"),
    ];
    let input: String = units.iter().map(|(_, raw)| *raw).collect();
    for size in [1, 2, 3, 7, input.len()] {
        let chunks: Vec<_> = input.as_bytes().chunks(size).collect();
        run(ScanMode::SubsetFragment, &units, &chunks);
    }
}

#[test]
fn delimiter_ownership_allows_the_controller_to_leave_body_input_untouched() {
    for (input, expected) in [
        ("<!DOCTYPE r><r/>", "<!DOCTYPE r>"),
        ("<!DOCTYPE r []><r/>", "<!DOCTYPE r []>"),
    ] {
        let mut scanner = DtdScanner::new(ScanMode::Doctype, limits());
        let mut offset = 0;
        while !scanner.is_complete() {
            let consumed = scanner.advance(&input.as_bytes()[offset..]).unwrap();
            offset += consumed;
            assert!(scanner.current().is_some());
            scanner.release();
        }
        assert_eq!(offset, expected.len());
        assert_eq!(&input[offset..], "<r/>");
        assert_eq!(scanner.advance(&input.as_bytes()[offset..]).unwrap(), 0);
    }
}

#[test]
fn framing_success_cannot_stand_in_for_grammar_or_semantic_success() {
    for raw in ["<!ELEMENT r (a|)>", "<!ATTLIST r a NOTATION () #IMPLIED>"] {
        let mut scanner = DtdScanner::new(ScanMode::SubsetFragment, limits());
        assert_eq!(scanner.advance(raw.as_bytes()).unwrap(), raw.len());
        let (kind, bytes, _) = scanner.current().unwrap();
        assert_eq!(kind, ScanKind::Declaration);
        assert!(DtdUnit::parse(std::str::from_utf8(bytes).unwrap(), DtdLimits::default()).is_err());
    }
    // These declarations are grammatical. The unit layer cannot establish
    // binding, recursion safety, declaration order, or default normalization.
    for raw in ["<!ENTITY e '&e;'>", "<!ATTLIST r a CDATA '&later;'>"] {
        assert!(DtdUnit::parse(raw, DtdLimits::default()).is_ok());
    }
}
