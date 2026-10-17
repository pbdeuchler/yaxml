use super::*;
fn limits() -> ReferenceLimits {
    ReferenceLimits {
        max_bytes: 128,
        max_scratch_bytes: 128,
        max_work: 4096,
    }
}
#[test]
fn classifications_keep_raw_spelling_and_do_not_read_suffix() {
    for (raw, kind) in [
        ("&e;", ReferenceKind::General),
        ("&a:b:c;", ReferenceKind::General),
        ("&根;", ReferenceKind::General),
        ("&amp;", ReferenceKind::Predefined('&')),
        ("&#60;", ReferenceKind::Numeric('<')),
        ("&#xD;", ReferenceKind::Numeric('\r')),
    ] {
        let mut s = ReferenceScratch::new(20, limits());
        let text = format!("{raw}post");
        assert_eq!(s.advance(text.as_bytes()).unwrap(), raw.len());
        let (actual, spelling, range) = s.current().unwrap();
        assert_eq!(actual, kind);
        assert_eq!(spelling, raw);
        assert_eq!(range, 20..20 + raw.len());
        assert_eq!(
            s.usage(),
            ReferenceUsage {
                scanned: raw.len(),
                copied: raw.len(),
                classified: raw.len(),
                work: raw.len() * 3
            }
        );
        s.release();
        s.restart(100).unwrap();
        assert_eq!(s.usage().work, raw.len() * 3);
        assert_eq!(s.position(), 100);
    }
}
#[test]
fn invalid_and_truncated_references_fail_at_local_coordinates() {
    for (raw, kind, offset) in [
        ("&a b;", ReferenceErrorKind::Syntax, 12),
        ("&#xZ;", ReferenceErrorKind::Syntax, 13),
        ("&#0;", ReferenceErrorKind::InvalidCharacter, 10),
        ("&a", ReferenceErrorKind::UnexpectedEof, 12),
        ("&1", ReferenceErrorKind::Syntax, 11),
    ] {
        let mut s = ReferenceScratch::new(10, limits());
        let error = match s.advance(raw.as_bytes()) {
            Err(e) => e,
            Ok(_) => s.finish_step().unwrap_err(),
        };
        assert_eq!(error.kind, kind);
        assert_eq!(error.offset, offset);
        assert_eq!(s.advance(b";"), Err(error));
        assert_eq!(s.restart(0), Err(error));
    }
}
#[test]
fn work_scratch_byte_and_final_input_bounds_are_explicit() {
    for (limits, kind) in [
        (
            ReferenceLimits {
                max_bytes: 2,
                ..limits()
            },
            ReferenceLimit::Bytes,
        ),
        (
            ReferenceLimits {
                max_scratch_bytes: 2,
                ..limits()
            },
            ReferenceLimit::ScratchBytes,
        ),
        (
            ReferenceLimits {
                max_work: 8,
                ..limits()
            },
            ReferenceLimit::Work,
        ),
    ] {
        let mut s = ReferenceScratch::new(0, limits);
        assert_eq!(
            s.advance(b"&a;").unwrap_err().kind,
            ReferenceErrorKind::Limit(kind)
        );
        assert!(s.current().is_none());
    }
    let mut s = ReferenceScratch::new(0, limits());
    s.advance(b"&a;").unwrap();
    s.finish_step().unwrap();
    assert_eq!(
        s.advance(b"x").unwrap_err().kind,
        ReferenceErrorKind::Sealed
    );
    let mut s = ReferenceScratch::new(usize::MAX, limits());
    assert_eq!(
        s.advance(b"&").unwrap_err().kind,
        ReferenceErrorKind::PositionOverflow
    );
    let mut s = ReferenceScratch::new(0, limits());
    assert_eq!(
        s.restart(3).unwrap_err().kind,
        ReferenceErrorKind::InvalidState
    );
}

#[test]
fn reference_work_cap_covers_final_classification_and_never_restores() {
    let mut s = ReferenceScratch::new(0, limits());
    s.advance(b"&a").unwrap();
    assert_eq!(s.usage().work, 4);
    s.cap_work(8);
    s.cap_work(usize::MAX);
    let error = s.advance(b";").unwrap_err();
    assert_eq!(error.kind, ReferenceErrorKind::Limit(ReferenceLimit::Work));
    assert_eq!(s.usage().work, 6);
    assert!(s.current().is_none());
    let mut s = ReferenceScratch::new(0, limits());
    s.advance(b"&a;").unwrap();
    s.release();
    s.cap_work(17);
    s.restart(10).unwrap();
    assert_eq!(
        s.advance(b"&a;").unwrap_err().kind,
        ReferenceErrorKind::Limit(ReferenceLimit::Work)
    );
}

#[test]
fn literal_non_xml_characters_are_distinct_from_invalid_name_grammar() {
    for (raw, offset) in [("&\0;", 11), ("&\u{fffe};", 11), ("&#\0;", 12), ("&\0", 11)] {
        let mut s = ReferenceScratch::new(10, limits());
        let error = match s.advance(raw.as_bytes()) {
            Err(error) => error,
            Ok(_) => s.finish_step().unwrap_err(),
        };
        assert_eq!(error.kind, ReferenceErrorKind::InvalidCharacter);
        assert_eq!(error.offset, offset);
    }
    let mut s = ReferenceScratch::new(10, limits());
    let error = s.advance(b"&<;").unwrap_err();
    assert_eq!(error.kind, ReferenceErrorKind::Syntax);
    assert_eq!(error.offset, 11);
}

#[test]
fn reference_partitions_preserve_spelling_errors_work_and_release_position() {
    for raw in [
        "&根:é;",
        "&amp;",
        "&#x1F980;",
        "&a b;",
        "&\0;",
        "&#xZ;",
        "&根",
        "&1",
    ] {
        let mut baseline = None;
        for split in 0..=raw.len() {
            let mut s = ReferenceScratch::new(9, limits());
            let mut error = None;
            for chunk in [&raw.as_bytes()[..split], &raw.as_bytes()[split..]] {
                if error.is_none()
                    && let Err(e) = s.advance(chunk)
                {
                    error = Some(e)
                }
            }
            if error.is_none() && s.current().is_none() {
                error = Some(s.finish_step().unwrap_err())
            }
            let actual = (
                error,
                s.current()
                    .map(|(kind, text, range)| (kind, text.to_owned(), range)),
                s.position(),
                s.usage(),
            );
            if let Some(expected) = &baseline {
                assert_eq!(&actual, expected, "{raw:?}, split {split}")
            } else {
                baseline = Some(actual)
            }
            if s.current().is_some() {
                let before = s.position();
                s.release();
                assert_eq!(s.position(), before);
                assert!(s.current().is_none())
            }
        }
    }
    // The raw ampersand spelling is classified once; its '&' scalar output is
    // never rescanned together with the untouched "T;" suffix.
    let mut s = ReferenceScratch::new(0, limits());
    assert_eq!(s.advance(b"&amp;T;").unwrap(), 5);
    assert_eq!(s.current().unwrap().0, ReferenceKind::Predefined('&'));
    assert_eq!(s.current().unwrap().1, "&amp;");
}
