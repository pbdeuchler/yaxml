use super::*;

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
    assert_eq!(pool.store("", 9, &mut reader).unwrap(), 0..0);
    assert_eq!(pool.0.capacity(), 0);
    assert_eq!(reader.owned_budget().remaining_owned(), 0);
    assert_eq!(
        pool.store("a", 9, &mut reader).unwrap_err(),
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
        assert_eq!(pool.store("ab", 0, &mut reader).unwrap(), 0..2);
        assert_eq!(reader.owned_budget().remaining_owned(), limit - 2);
        let result = pool.store("c", 7, &mut reader);
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
        let range = pool.store("", 8, &mut reader).unwrap();
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
        ranges.push(pool.store("x", 0, &mut reader).unwrap());
    }
    assert_eq!(pool.0.capacity(), 32);
    assert_eq!(reader.owned_budget().remaining_owned(), 0);
    for range in ranges {
        assert_eq!(&pool.0[range], "x");
    }
    assert!(pool.store("x", 11, &mut reader).is_err());
    assert_eq!(pool.0.len(), 32);
}

#[test]
fn qualified_name_views_split_at_unicode_byte_boundaries() {
    let mut reader = reader(4096);
    let mut pool = Pool::default();
    pool.store("🙂", 0, &mut reader).unwrap();
    let prefixed = pool
        .name(
            &Name {
                qualified: "é:中",
                prefix: Some("é"),
                local: "中",
                namespace: Some(Cow::Borrowed("urn:🙂")),
            },
            4,
            &mut reader,
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
            &mut reader,
        )
        .unwrap();
    pool.store(&"growth".repeat(100), 0, &mut reader).unwrap();
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
        pool.name(&name, 3, &mut reader).unwrap_err(),
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
                &mut reader,
            )
            .unwrap(),
        value: pool.store("\n\r\té🙂", 0, &mut reader).unwrap(),
    };
    let version = pool.store("1.00", 0, &mut reader).unwrap();
    let encoding = pool.store("uTf-8", 0, &mut reader).unwrap();
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
    pool.store("r", 0, &mut reader).unwrap();
    assert_eq!(reader.owned_budget().remaining_owned(), 7);
    let Some(Event::Text(value)) = reader.read_event().unwrap() else {
        panic!("expected normalized text")
    };
    assert_eq!(reader.owned_budget().remaining_owned(), 2);
    pool.store(&value, 3, &mut reader).unwrap();
    assert_eq!(reader.owned_budget().remaining_owned(), 0);
    assert_eq!(pool.0, "r&");
    let before = pool.0.clone();
    assert!(pool.store("x", 3, &mut reader).is_err());
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
