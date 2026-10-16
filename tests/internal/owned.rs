use super::*;

#[test]
fn document_identities_are_nonzero_nonrecycled_and_exhaust_without_wrapping() {
    let counter = AtomicUsize::new(0);
    assert_eq!(document_identity(&counter).unwrap().get(), 1);
    assert_eq!(document_identity(&counter).unwrap().get(), 2);
    let counter = AtomicUsize::new(usize::MAX - 1);
    assert_eq!(document_identity(&counter).unwrap().get(), usize::MAX);
    for _ in 0..2 {
        assert_eq!(
            document_identity(&counter).unwrap_err(),
            Error::new(ErrorKind::IdentifierExhausted, 0)
        );
        assert_eq!(counter.load(Ordering::Relaxed), usize::MAX);
    }
}

#[test]
fn concurrent_identity_assignment_is_unique() {
    let counter = std::sync::Arc::new(AtomicUsize::new(0));
    let threads: Vec<_> = (0..4)
        .map(|_| {
            let counter = counter.clone();
            std::thread::spawn(move || {
                (0..128)
                    .map(|_| document_identity(&counter).unwrap())
                    .collect::<Vec<_>>()
            })
        })
        .collect();
    let identities: std::collections::HashSet<_> = threads
        .into_iter()
        .flat_map(|thread| thread.join().unwrap())
        .collect();
    assert_eq!(identities.len(), 512);
    assert_eq!(counter.load(Ordering::Relaxed), 512);
}

#[test]
fn construction_finishes_xml_before_assigning_identity() {
    let counter = AtomicUsize::new(0);
    assert_eq!(
        OwnedBuilder::new(0)
            .unwrap()
            .finish_with_counter(0, &counter)
            .unwrap_err(),
        Error::new(ErrorKind::MissingRoot, 0)
    );
    assert_eq!(counter.load(Ordering::Relaxed), 0);

    for last in [0, usize::MAX] {
        let counter = AtomicUsize::new(last);
        let mut reader = Reader::new(b"<r/>").unwrap();
        let mut builder = OwnedBuilder::new(4).unwrap();
        let event = reader.read_event().unwrap().unwrap();
        builder
            .consume(event, reader.event_range(), reader.owned_budget())
            .unwrap();
        let result = builder.finish_with_counter(4, &counter);
        if last == 0 {
            let document = result.unwrap();
            assert_eq!(document.identity.get(), 1);
            assert_eq!(document.root().range(), Some(0..4));
            let invalid = OwnedNodeId {
                document: document.identity,
                slot: NodeId(usize::MAX),
            };
            assert!(document.node(invalid).is_none());
        } else {
            assert_eq!(
                result.unwrap_err(),
                Error::new(ErrorKind::IdentifierExhausted, 0)
            );
        }
    }
}

fn reader(limit: usize) -> Reader<'static> {
    Reader::with_limits(
        b"<r/>",
        Limits {
            max_owned_bytes: limit,
            ..Limits::default()
        },
    )
    .unwrap()
}

#[test]
fn pool_empty_strings_do_not_reserve_or_charge() {
    let mut reader = reader(0);
    let mut pool = Pool::default();
    assert_eq!(pool.store("", 9, reader.owned_budget()).unwrap(), 0..0);
    assert_eq!(pool.0.capacity(), 0);
    assert_eq!(reader.owned_budget().remaining_owned(), 0);
    assert_eq!(
        pool.store("a", 9, reader.owned_budget()).unwrap_err(),
        Error::new(ErrorKind::LimitExceeded(Limit::OwnedBytes), 9)
    );
    assert!(pool.0.is_empty());
    assert_eq!(pool.0.capacity(), 0);
}

#[test]
fn pool_charges_full_new_capacity_and_clamps_growth() {
    for (limit, accepted, final_capacity) in [(4, false, 2), (5, true, 3), (6, true, 4)] {
        let mut reader = reader(limit);
        let mut pool = Pool::default();
        assert_eq!(pool.store("ab", 0, reader.owned_budget()).unwrap(), 0..2);
        assert_eq!(reader.owned_budget().remaining_owned(), limit - 2);
        let result = pool.store("c", 7, reader.owned_budget());
        assert_eq!(result.is_ok(), accepted);
        assert_eq!(pool.0.capacity(), final_capacity);
        if accepted {
            assert_eq!(result.unwrap(), 2..3);
            assert_eq!(pool.0, "abc");
            assert_eq!(reader.owned_budget().remaining_owned(), 0);
        } else {
            assert_eq!(
                result.unwrap_err(),
                Error::new(ErrorKind::LimitExceeded(Limit::OwnedBytes), 7)
            );
            assert_eq!(pool.0, "ab");
            assert_eq!(reader.owned_budget().remaining_owned(), 2);
        }
        let range = pool.store("", 8, reader.owned_budget()).unwrap();
        assert_eq!(range.start, range.end);
        assert_eq!(pool.0.capacity(), final_capacity);
    }
}

#[test]
fn geometric_pool_growth_retains_ranges_and_counts_previous_buffers() {
    let mut reader = reader(63);
    let mut pool = Pool::default();
    let mut ranges = Vec::new();
    for _ in 0..32 {
        ranges.push(pool.store("x", 0, reader.owned_budget()).unwrap());
    }
    assert_eq!(pool.0.capacity(), 32);
    assert_eq!(reader.owned_budget().remaining_owned(), 0);
    for range in ranges {
        assert_eq!(&pool.0[range], "x");
    }
    assert!(pool.store("x", 11, reader.owned_budget()).is_err());
    assert_eq!(pool.0.len(), 32);
}

#[test]
fn qualified_name_views_split_at_unicode_byte_boundaries() {
    let mut reader = reader(4096);
    let mut pool = Pool::default();
    pool.store("🙂", 0, reader.owned_budget()).unwrap();
    let prefixed = pool
        .name(
            &Name {
                qualified: "é:中",
                prefix: Some("é"),
                local: "中",
                namespace: Some(Cow::Borrowed("urn:🙂")),
            },
            4,
            reader.owned_budget(),
        )
        .unwrap();
    let plain = pool
        .name(
            &Name {
                qualified: "plain",
                prefix: None,
                local: "plain",
                namespace: None,
            },
            8,
            reader.owned_budget(),
        )
        .unwrap();
    pool.store(&"growth".repeat(100), 0, reader.owned_budget())
        .unwrap();
    let view = prefixed.view(&pool.0);
    assert_eq!(view.qualified, "é:中");
    assert_eq!(view.prefix, Some("é"));
    assert_eq!(view.local, "中");
    assert!(matches!(view.namespace, Some(Cow::Borrowed("urn:🙂"))));
    let view = plain.view(&pool.0);
    assert_eq!(view.qualified, "plain");
    assert_eq!(view.prefix, None);
    assert_eq!(view.local, "plain");
    assert_eq!(view.namespace, None);
}

#[test]
fn name_storage_failure_preserves_the_aggregate_budget() {
    let mut reader = reader(4);
    let mut pool = Pool::default();
    let name = Name {
        qualified: "é:p",
        prefix: Some("é"),
        local: "p",
        namespace: Some(Cow::Borrowed("urn:p")),
    };
    assert_eq!(
        pool.name(&name, 3, reader.owned_budget()).unwrap_err(),
        Error::new(ErrorKind::LimitExceeded(Limit::OwnedBytes), 3)
    );
    assert_eq!(pool.0, "é:p");
    assert_eq!(reader.owned_budget().remaining_owned(), 0);
}

#[test]
fn attribute_and_declaration_views_borrow_the_pool() {
    let mut reader = reader(4096);
    let mut pool = Pool::default();
    let attribute = StoredAttribute {
        name: pool
            .name(
                &Name {
                    qualified: "a",
                    prefix: None,
                    local: "a",
                    namespace: None,
                },
                0,
                reader.owned_budget(),
            )
            .unwrap(),
        value: pool.store("\n\r\té🙂", 0, reader.owned_budget()).unwrap(),
    };
    let version = pool.store("1.00", 0, reader.owned_budget()).unwrap();
    let encoding = pool.store("uTf-8", 0, reader.owned_budget()).unwrap();
    for encoding in [None, Some(encoding)] {
        let declaration = StoredDeclaration {
            version: version.clone(),
            encoding: encoding.clone(),
            standalone: Some(false),
        };
        let view = declaration.view(&pool.0);
        assert_eq!(view.version, "1.00");
        assert_eq!(view.encoding, encoding.map(|_| "uTf-8"));
        assert_eq!(view.standalone, Some(false));
    }
    let view = attribute.view(&pool.0);
    assert_eq!(view.name.qualified, "a");
    assert!(matches!(view.value, Cow::Borrowed("\n\r\té🙂")));
}

#[test]
fn parser_normalization_and_pool_growth_share_one_budget() {
    for (input, minimum) in [
        (b"<r/>".as_slice(), 1),
        (b"<r>a</r>", 3),
        (b"<r>&amp;</r>", 8),
        (b"<r a='&#9;'/>", 10),
    ] {
        let limits = Limits {
            max_owned_bytes: minimum,
            ..Limits::default()
        };
        OwnedDocument::with_limits(input, limits).unwrap();
        let error = OwnedDocument::with_limits(
            input,
            Limits {
                max_owned_bytes: minimum - 1,
                ..limits
            },
        )
        .unwrap_err();
        assert_eq!(error.kind, ErrorKind::LimitExceeded(Limit::OwnedBytes));
        assert!(error.offset <= input.len());
    }
    let mut reader = Reader::with_limits(
        b"<r>&amp;</r>",
        Limits {
            max_owned_bytes: 8,
            ..Limits::default()
        },
    )
    .unwrap();
    let mut pool = Pool::default();
    assert!(matches!(
        reader.read_event().unwrap(),
        Some(Event::Start(_))
    ));
    pool.store("r", 0, reader.owned_budget()).unwrap();
    assert_eq!(reader.owned_budget().remaining_owned(), 7);
    let Some(Event::Text(value)) = reader.read_event().unwrap() else {
        panic!("expected normalized text")
    };
    assert_eq!(reader.owned_budget().remaining_owned(), 2);
    pool.store(&value, 3, reader.owned_budget()).unwrap();
    assert_eq!(reader.owned_budget().remaining_owned(), 0);
    assert_eq!(pool.0, "r&");
    let before = pool.0.clone();
    assert!(pool.store("x", 3, reader.owned_budget()).is_err());
    assert_eq!(pool.0, before);
    assert!(matches!(reader.read_event().unwrap(), Some(Event::End(_))));
    assert!(reader.read_event().unwrap().is_none());
}

#[test]
fn shared_topology_append_links_children_and_siblings() {
    let mut records = vec![Record::root(OwnedNodeKind::Document, 20)];
    let first = append_record(&mut records, NodeId(0), OwnedNodeKind::Element, 1..19).unwrap();
    let child = append_record(&mut records, first, OwnedNodeKind::Text, 4..8).unwrap();
    let last = append_record(&mut records, NodeId(0), OwnedNodeKind::Comment, 19..20).unwrap();
    assert_eq!(records[0].parent, None);
    assert_eq!(records[0].first_child, Some(first));
    assert_eq!(records[0].last_child, Some(last));
    assert_eq!(records[first.0].parent, Some(NodeId(0)));
    assert_eq!(records[first.0].next_sibling, Some(last));
    assert_eq!(records[first.0].first_child, Some(child));
    assert_eq!(records[first.0].last_child, Some(child));
    assert_eq!(records[child.0].parent, Some(first));
    assert_eq!(records[child.0].next_sibling, None);
    assert_eq!(records[child.0].subtree_end, child.0 + 1);
    assert_eq!(records[child.0].range, 4..8);
}

#[test]
fn owned_builder_errors_are_not_remapped_or_replaced_by_cancellation() {
    use std::ops::ControlFlow;
    let mut builder = OwnedBuilder::new(10).unwrap();
    let mut reader = reader(1);
    assert_eq!(
        builder.consume(
            Event::End(Name {
                qualified: "r",
                prefix: None,
                local: "r",
                namespace: None
            }),
            6..10,
            reader.owned_budget()
        ),
        Err(Error::new(ErrorKind::MismatchedEndTag, 6))
    );
    assert_eq!(
        builder.finish(10).unwrap_err(),
        Error::new(ErrorKind::MissingRoot, 10)
    );
    let mut parser = PushParser::new();
    let expected = Error::new(ErrorKind::Allocation, 7);
    let input: Vec<u8> = [0xff, 0xfe]
        .into_iter()
        .chain("<é/>".encode_utf16().flat_map(u16::to_le_bytes))
        .collect();
    assert_eq!(
        parser.feed_with(&input, |_, range, _| {
            assert_eq!(range, 2..10);
            Err(expected)
        }),
        Err(expected)
    );
    assert_eq!(
        parser.finish(|_, _| ControlFlow::Continue(())),
        Err(expected)
    );
    assert_eq!(
        parser.feed(&[], |_, _| ControlFlow::Continue(())),
        Err(expected)
    );
    let mut parser = PushParser::new();
    parser
        .feed(b"<r>text", |_, _| ControlFlow::Continue(()))
        .unwrap();
    assert_eq!(parser.finish_with(|_, _, _| Err(expected)), Err(expected));
}

#[test]
fn utf8_owned_fast_path_applies_decoded_limits_without_retention_charges() {
    let limits = Limits {
        max_owned_bytes: 1,
        ..Limits::default()
    };
    let decode = DecodeLimits {
        max_decoded_bytes: 4,
        max_decoded_token_bytes: 4,
    };
    OwnedDocument::with_decode_limits(b"<r/>", limits, decode).unwrap();
    OwnedDocument::with_decode_limits(b"\xef\xbb\xbf<r/>", limits, decode).unwrap();
    for (decode, kind) in [
        (
            DecodeLimits {
                max_decoded_bytes: 3,
                ..decode
            },
            Limit::DecodedBytes,
        ),
        (
            DecodeLimits {
                max_decoded_token_bytes: 3,
                ..decode
            },
            Limit::DecodedTokenBytes,
        ),
    ] {
        assert_eq!(
            OwnedDocument::with_decode_limits(b"<r/>", limits, decode).unwrap_err(),
            Error::new(ErrorKind::LimitExceeded(kind), 0)
        );
    }
}
