use super::*;
use crate::dtd_reference::{ReferenceLimits, ReferenceScratch};
use crate::tokenizer::local::{LocalErrorKind, LocalLimits, LocalTokenizer};

fn scratch(start: usize) -> ReferenceScratch {
    ReferenceScratch::new(
        start,
        ReferenceLimits {
            max_bytes: 100,
            max_scratch_bytes: 100,
            max_work: 1000,
        },
    )
}
fn collect(
    mut cursor: ReferenceCursor,
    reference: &mut ReferenceScratch,
    raw: &[u8],
) -> ReferenceExtent {
    assert_eq!(cursor.decoded_position(), reference.position());
    for byte in raw {
        let next = cursor.checked_advance(std::slice::from_ref(byte)).unwrap();
        assert_eq!(reference.advance(std::slice::from_ref(byte)).unwrap(), 1);
        cursor = next;
    }
    cursor.finish(reference.validated().unwrap()).unwrap()
}

#[test]
fn named_prefix_release_raw_character_append_and_saved_delimiter_are_ordered() {
    let input = b"pre&amp;x&e;post<r/>";
    let mut scanner = Tokenizer::new(100, 100);
    assert_eq!(
        scanner.advance_content(input).unwrap(),
        ContentStep {
            consumed: 3,
            stop: ContentStop::ReferenceStart
        }
    );
    let mut reference = scratch(3);
    assert!(reference.validated().is_none());
    let extent = collect(
        scanner.reference_cursor().unwrap(),
        &mut reference,
        b"&amp;",
    );
    assert_eq!(extent.input_span().unwrap().source, 3..8);
    assert!(extent.local_range().is_none());
    scanner
        .append_character_reference(reference.validated().unwrap(), &extent)
        .unwrap();
    reference.release();
    assert!(reference.validated().is_none());
    assert_eq!(
        scanner.advance_content(&input[8..]).unwrap(),
        ContentStep {
            consumed: 1,
            stop: ContentStop::ReferenceStart
        }
    );
    reference.restart(9).unwrap();
    let extent = collect(scanner.reference_cursor().unwrap(), &mut reference, b"&e;");
    assert!(scanner.flush_text_before_reference().unwrap());
    let (_, raw, span) = scanner.current().unwrap();
    assert_eq!(raw, b"pre&amp;x");
    assert_eq!(span.decoded, 0..9);
    assert_eq!(span.source, 0..9);
    assert_eq!(scanner.source_position(), 9);
    assert!(
        scanner
            .skip_general_reference(reference.validated().unwrap(), &extent)
            .is_err()
    );
    scanner.release();
    scanner
        .skip_general_reference(reference.validated().unwrap(), &extent)
        .unwrap();
    assert_eq!(scanner.source_position(), 12);
    assert_eq!(
        scanner.advance_content(&input[12..]).unwrap(),
        ContentStep {
            consumed: 5,
            stop: ContentStop::TokenReady
        }
    );
    assert_eq!(scanner.current().unwrap().1, b"post");
    assert_eq!(scanner.current().unwrap().2.source, 12..16);
    scanner.release();
    assert_eq!(scanner.advance_content(&input[17..]).unwrap().consumed, 3);
    assert_eq!(scanner.current().unwrap().1, b"<r/>");
    assert_eq!(scanner.current().unwrap().2.source, 16..20);
}

#[test]
fn only_dtd_text_entry_recognizes_ampersands() {
    for raw in [
        b"<r a='&e;'/>".as_slice(),
        b"<!--&e;-->",
        b"<![CDATA[&e;]]>",
        b"<?p &e;?>",
    ] {
        let mut scanner = Tokenizer::new(100, 100);
        assert_eq!(
            scanner.advance_content(raw).unwrap(),
            ContentStep {
                consumed: raw.len(),
                stop: ContentStop::TokenReady
            }
        );
        assert_eq!(scanner.current().unwrap().1, raw);
        assert!(scanner.reference_cursor().is_err());
    }
    let mut ordinary = Tokenizer::new(100, 100);
    assert_eq!(ordinary.advance(b"&e;").unwrap(), 3);
    ordinary.finish_step().unwrap();
    assert_eq!(ordinary.current().unwrap().1, b"&e;");
}

#[test]
fn checked_extents_and_reference_kinds_cannot_bypass_cursor_or_token_limits() {
    let mut scanner = Tokenizer::new(3, 100);
    scanner.advance_content(b"pre&e;").unwrap();
    let mut reference = scratch(3);
    let extent = collect(scanner.reference_cursor().unwrap(), &mut reference, b"&e;");
    assert!(
        scanner
            .append_character_reference(reference.validated().unwrap(), &extent)
            .is_err()
    );
    scanner.flush_text_before_reference().unwrap();
    scanner.release();
    scanner
        .skip_general_reference(reference.validated().unwrap(), &extent)
        .unwrap();
    assert!(
        scanner
            .skip_general_reference(reference.validated().unwrap(), &extent)
            .is_err()
    );
    let mut scanner = Tokenizer::new(5, 100);
    scanner.advance_content(b"p&amp;").unwrap();
    let mut reference = scratch(1);
    let extent = collect(
        scanner.reference_cursor().unwrap(),
        &mut reference,
        b"&amp;",
    );
    assert_eq!(
        scanner
            .append_character_reference(reference.validated().unwrap(), &extent)
            .unwrap_err()
            .kind,
        ErrorKind::LimitExceeded(Limit::TokenBytes)
    );
    assert_eq!(scanner.source_position(), 1);
    let mut short = Tokenizer::new(2, 100);
    assert_eq!(
        short
            .reference_cursor()
            .unwrap()
            .checked_advance(b"&e;")
            .unwrap_err()
            .kind,
        ReferenceCursorErrorKind::Limit(Limit::TokenBytes)
    );
    short.advance(b"<r/>").unwrap_err();
    assert!(short.reference_cursor().is_err());

    let mut at_end = Tokenizer::new(100, 100);
    at_end.configure(SourceEncoding::Utf8, usize::MAX);
    assert_eq!(
        at_end
            .reference_cursor()
            .unwrap()
            .checked_advance(b"&")
            .unwrap_err()
            .kind,
        ReferenceCursorErrorKind::PositionOverflow
    );
    let scanner = Tokenizer::new(100, 100);
    assert_eq!(
        scanner
            .reference_cursor()
            .unwrap()
            .finish(reference.validated().unwrap())
            .unwrap_err()
            .kind,
        ReferenceCursorErrorKind::InvalidState
    );
    let mut empty = Tokenizer::new(100, 100);
    assert!(!empty.flush_text_before_reference().unwrap());
}

#[test]
fn local_reference_copy_is_separate_from_new_source_scan_and_obeys_work_cap() {
    let limits = LocalLimits {
        max_token_bytes: 100,
        max_scratch_bytes: 100,
        max_work: 100,
    };
    let mut local = LocalTokenizer::new(10, limits);
    assert_eq!(local.advance_content(b"a&amp;").unwrap().consumed, 1);
    let mut reference = scratch(11);
    let extent = collect(local.reference_cursor().unwrap(), &mut reference, b"&amp;");
    assert_eq!(extent.local_range(), Some(11..16));
    assert!(extent.input_span().is_none());
    local
        .append_character_reference(reference.validated().unwrap(), &extent)
        .unwrap();
    assert_eq!(local.usage().scanned, 1);
    assert_eq!(local.usage().reference_copied, 5);
    assert_eq!(local.position(), 16);
    local.finish_step().unwrap();
    assert_eq!(local.current().unwrap().1, b"a&amp;");
    assert_eq!(local.current().unwrap().2, 10..16);
    assert_eq!(local.usage().work, 12);
    local.release();
    assert!(local.is_complete());
    assert_eq!(
        local.reference_cursor().unwrap_err().kind,
        LocalErrorKind::Sealed
    );
    assert_eq!(local.restart(0).unwrap_err().kind, LocalErrorKind::Sealed);

    let mut local = LocalTokenizer::new(0, limits);
    local.advance_content(b"a&amp;").unwrap();
    let mut reference = scratch(1);
    let extent = collect(local.reference_cursor().unwrap(), &mut reference, b"&amp;");
    local.cap_work(5);
    assert_eq!(
        local
            .append_character_reference(reference.validated().unwrap(), &extent)
            .unwrap_err()
            .kind,
        LocalErrorKind::Work
    );
    assert_eq!(local.position(), 1);
    assert_eq!(local.usage().reference_copied, 0);
}

#[test]
fn local_general_prefix_requires_release_and_accepts_no_input_extent() {
    let limits = LocalLimits {
        max_token_bytes: 100,
        max_scratch_bytes: 100,
        max_work: 100,
    };
    let mut local = LocalTokenizer::new(0, limits);
    local.advance_content(b"pre&e;").unwrap();
    let mut reference = scratch(3);
    let extent = collect(local.reference_cursor().unwrap(), &mut reference, b"&e;");
    local.flush_text_before_reference().unwrap();
    assert_eq!(local.current().unwrap().1, b"pre");
    local.release();
    local
        .skip_general_reference(reference.validated().unwrap(), &extent)
        .unwrap();
    assert_eq!(local.position(), 6);
    assert_eq!(local.usage().work, 6);

    let mut input = Tokenizer::new(100, 100);
    input.advance_content(b"pre&e;").unwrap();
    input.flush_text_before_reference().unwrap();
    input.release();
    assert_eq!(
        input
            .skip_general_reference(reference.validated().unwrap(), &extent)
            .unwrap_err()
            .kind,
        ErrorKind::InvalidState
    );
}
