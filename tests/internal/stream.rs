use super::*;

#[test]
fn tokens_remain_available_until_explicit_release() {
    let mut stream = Stream::new(Limits::default(), DecodeLimits::default());
    assert_eq!(stream.advance(b"<r/>tail").unwrap(), 4);
    assert_eq!(stream.position(), 4);
    let (event, range) = stream.event().unwrap();
    assert!(matches!(event, Event::Empty(_)));
    assert_eq!(range, 0..4);
    stream.release();
    assert_eq!(stream.advance(b" ").unwrap(), 1);
    assert!(stream.finish_step().unwrap());
    assert!(stream.is_non_event());
    stream.release();
    assert!(!stream.finish_step().unwrap());
    stream.validate_finish().unwrap();
}
