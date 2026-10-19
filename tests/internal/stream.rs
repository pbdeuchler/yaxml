use super::*;

#[test]
fn tokens_remain_available_until_explicit_release() {
    let mut stream = Stream::new(
        Limits::default(),
        DecodeLimits::default(),
        Default::default(),
    );
    assert_eq!(stream.advance(b"<r/>tail").unwrap(), 4);
    assert_eq!(stream.position(), 4);
    let (event, range) = stream.event().unwrap();
    assert!(matches!(event, Event::Empty(_)));
    assert_eq!(range.input_range(), Some(0..4));
    stream.release();
    assert_eq!(stream.advance(b" ").unwrap(), 1);
    assert!(stream.finish_step().unwrap());
    assert!(stream.is_non_event());
    stream.release();
    assert!(!stream.finish_step().unwrap());
    stream.validate_finish().unwrap();
}

#[test]
fn unavailable_or_non_event_tokens_cannot_be_lent_as_events() {
    let mut stream = Stream::new(
        Limits::default(),
        DecodeLimits::default(),
        Default::default(),
    );
    assert_eq!(
        stream.event().unwrap_err(),
        Error::new(ErrorKind::InvalidState, 0)
    );
    assert_eq!(stream.advance(b" <r/>").unwrap(), 2);
    assert!(stream.is_non_event());
    assert_eq!(
        stream.event().unwrap_err(),
        Error::new(ErrorKind::InvalidState, 2)
    );
    let mut calls = 0;
    stream
        .emit(&mut |_, _, _| {
            calls += 1;
            Ok(())
        })
        .unwrap();
    assert_eq!(calls, 0);
}

#[test]
fn sibling_history_does_not_grow_retained_state() {
    for count in [64, 256, 1024, 4096] {
        let mut input = String::from("<r>");
        for index in 0..count {
            input.push_str(&format!("<p:n xmlns:p='urn:{index}' a='x'>v</p:n>"));
        }
        input.push_str("</r>");
        for capacity in [1, 7, 8192] {
            let mut stream = Stream::new(
                Limits::default(),
                DecodeLimits::default(),
                Default::default(),
            );
            let mut events = 0;
            let mut state_peaks = [(0usize, 0usize); 4];
            let mut token_peak = 0;
            for chunk in input.as_bytes().chunks(capacity) {
                let mut remaining = chunk;
                while !remaining.is_empty() {
                    let consumed = stream.advance(remaining).unwrap();
                    remaining = &remaining[consumed..];
                    if stream.has_token() {
                        stream
                            .emit(&mut |_, _, _| {
                                events += 1;
                                Ok(())
                            })
                            .unwrap();
                        for (peak, (entries, capacity)) in
                            state_peaks.iter_mut().zip(stream.state.retained_snapshot())
                        {
                            peak.0 = peak.0.max(entries);
                            peak.1 = peak.1.max(capacity);
                            assert!(
                                entries <= 2,
                                "{entries} retained entries after {events} events"
                            );
                            assert!(
                                capacity <= 32,
                                "{capacity} retained capacity after {events} events"
                            );
                        }
                        token_peak = token_peak.max(stream.tokenizer.retained_snapshot().1);
                        assert!(token_peak <= 64);
                        stream.release();
                    }
                }
            }
            assert!(!stream.finish_step().unwrap());
            stream.validate_finish().unwrap();
            assert_eq!(stream.position(), input.len());
            assert_eq!(events, count * 3 + 2);
            assert_eq!(stream.tokenizer.retained_snapshot().0, 0);
            assert!(
                stream
                    .state
                    .retained_snapshot()
                    .iter()
                    .all(|(entries, _)| *entries == 0)
            );
            eprintln!(
                "siblings={count} bytes={} chunk={capacity} events={events} token_capacity_peak={token_peak} state_entry_capacity_peaks={state_peaks:?}",
                input.len()
            );
        }
    }
}
