use super::*;

#[test]
fn physical_origins_and_element_joins() {
    let opening = SourceOrigin::input(3..6);
    assert_eq!(opening.input_range(), Some(3..6));
    assert_eq!(opening.error_offset(), 3);
    assert_eq!(opening.cancellation_offset(), 6);
    for (end, expected) in [
        (6..10, Ok(Some(3..10))),
        (18..22, Ok(Some(3..22))),
        (2..5, Err(Error::new(ErrorKind::InvalidState, 2))),
        (5..9, Err(Error::new(ErrorKind::InvalidState, 5))),
    ] {
        assert_eq!(
            opening
                .close_element(SourceOrigin::input(end))
                .map(SourceOrigin::input_range),
            expected
        );
    }
    assert_eq!(opening.input_range(), Some(3..6));
}
