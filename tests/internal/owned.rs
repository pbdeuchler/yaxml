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
        OwnedBuilder::new(0, 0, Global)
            .unwrap()
            .finish_with_counter(0, &counter, false)
            .unwrap_err(),
        Error::new(ErrorKind::MissingRoot, 0)
    );
    assert_eq!(counter.load(Ordering::Relaxed), 0);

    for last in [0, usize::MAX] {
        let counter = AtomicUsize::new(last);
        let mut reader = Reader::new(b"<r/>").unwrap();
        let mut builder = OwnedBuilder::new(4, 0, Global).unwrap();
        let event = reader.read_event().unwrap().unwrap();
        builder
            .consume(
                &event,
                reader.event_origin().expect("parsed origin"),
                reader.owned_budget(),
            )
            .unwrap();
        let result = builder.finish_with_counter(4, &counter, false);
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
            &Name::new("é:中", Some(Cow::Borrowed("urn:🙂"))).unwrap(),
            4,
            reader.owned_budget(),
        )
        .unwrap();
    let plain = pool
        .name(&Name::new("plain", None).unwrap(), 8, reader.owned_budget())
        .unwrap();
    pool.store(&"growth".repeat(100), 0, reader.owned_budget())
        .unwrap();
    let view = prefixed.view(&pool.0);
    assert_eq!(view.qualified(), "é:中");
    assert_eq!(view.prefix(), Some("é"));
    assert_eq!(view.local(), "中");
    assert_eq!(view.namespace(), Some("urn:🙂"));
    let view = plain.view(&pool.0);
    assert_eq!(view.qualified(), "plain");
    assert_eq!(view.prefix(), None);
    assert_eq!(view.local(), "plain");
    assert_eq!(view.namespace(), None);
}

#[test]
fn name_views_preserve_default_prefixed_and_empty_namespace_ranges() {
    let mut reader = reader(4096);
    let mut pool = Pool::default();
    let cases = [
        ("plain", None, "plain", None),
        ("é中", None, "é中", None),
        ("root", None, "root", Some("urn:default")),
        ("é:中", Some("é"), "中", Some("urn:🙂")),
        ("empty", None, "empty", Some("")),
    ];
    let stored: Vec<_> = cases
        .iter()
        .map(|&(qualified, _, _, namespace)| {
            pool.name(
                &Name::new(qualified, namespace.map(Cow::Borrowed)).unwrap(),
                0,
                reader.owned_budget(),
            )
            .unwrap()
        })
        .collect();
    pool.store(&"growth".repeat(100), 0, reader.owned_budget())
        .unwrap();
    for (stored, expected) in stored.iter().zip(cases) {
        let view = stored.view(&pool.0);
        assert_eq!(
            (
                view.qualified(),
                view.prefix(),
                view.local(),
                view.namespace()
            ),
            expected
        );
        if let Some(range) = &stored.namespace {
            assert_eq!(
                view.namespace().unwrap().as_ptr(),
                pool.0[range.clone()].as_ptr()
            );
        }
    }
}

#[test]
fn empty_prefixes_are_rejected_before_storage() {
    assert_eq!(
        OwnedDocument::parse("<p:r/>").unwrap_err().kind,
        ErrorKind::Namespace
    );
    assert_eq!(
        Name::new(":r", Some(Cow::Borrowed("urn:p"))).unwrap_err(),
        Error::new(ErrorKind::InvalidName, 0),
    );
}

#[test]
fn name_storage_failure_preserves_the_aggregate_budget() {
    let mut reader = reader(4);
    let mut pool = Pool::default();
    let name = Name::new("é:p", Some(Cow::Borrowed("urn:p"))).unwrap();
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
            .name(&Name::new("a", None).unwrap(), 0, reader.owned_budget())
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
    assert_eq!(view.name.qualified(), "a");
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
fn owned_links_are_reciprocal_and_subtree_counts_match_ancestor_membership() {
    for input in [
        "<r/>",
        "<!--pre--><r><a>A<b/>B</a><c/><d>D</d></r><!--post-->",
        "<?pre?><r><![CDATA[x]]><!--y--><?pi z?><a/></r><?post?>",
    ] {
        let document = OwnedDocument::parse(input).unwrap();
        let mut expected_sizes = vec![0; document.nodes.len()];
        for (index, record) in document.nodes.iter().enumerate() {
            assert_eq!(record.id, NodeId(index));
            assert!(!matches!(record.content, Content::Removed));
            let mut ancestor = Some(NodeId(index));
            while let Some(id) = ancestor {
                expected_sizes[id.0] += 1;
                ancestor = document.nodes[id.0].parent.get();
            }
            let mut children = Vec::new();
            let mut child = record.first_child.get();
            let mut previous = None;
            while let Some(id) = child {
                let child_record = &document.nodes[id.0];
                assert_eq!(child_record.parent.get(), Some(NodeId(index)));
                assert_eq!(child_record.previous_sibling.get(), previous);
                if let Some(previous) = previous {
                    assert_eq!(document.nodes[previous.0].next_sibling.get(), Some(id));
                }
                children.push(id);
                previous = Some(id);
                child = child_record.next_sibling.get();
            }
            assert_eq!(record.last_child.get(), children.last().copied());
            if let Some(last) = record.last_child.get() {
                assert_eq!(document.nodes[last.0].next_sibling.get(), None);
            }
        }
        for (record, expected) in document.nodes.iter().zip(expected_sizes) {
            assert_eq!(record.subtree_size, expected);
        }
        assert_eq!(document.nodes[0].subtree_size, document.nodes.len());
    }
}

#[test]
fn compact_links_preserve_every_slot_below_the_private_sentinel() {
    assert_eq!(Link::NONE.get(), None);
    for index in [0, 1, usize::MAX - 1] {
        assert_eq!(Link::new(NodeId(index)).get(), Some(NodeId(index)));
    }
}

#[test]
fn compact_owned_record_layout_is_smaller_than_optional_usize_links() {
    #[allow(dead_code)]
    struct NaiveRecord {
        id: NodeId,
        content: Content,
        source: Option<Range<usize>>,
        parent: Option<NodeId>,
        first_child: Option<NodeId>,
        last_child: Option<NodeId>,
        previous_sibling: Option<NodeId>,
        next_sibling: Option<NodeId>,
        subtree_size: usize,
    }
    println!(
        "pointer={} option_node_id={} link={} stored_name={} stored_attribute={} content={} naive_owned_record={} compact_owned_record={} owned_node={}",
        size_of::<usize>(),
        size_of::<Option<NodeId>>(),
        size_of::<Link>(),
        size_of::<StoredName>(),
        size_of::<StoredAttribute>(),
        size_of::<Content>(),
        size_of::<NaiveRecord>(),
        size_of::<OwnedRecord>(),
        size_of::<OwnedNode<'_>>(),
    );
    assert_eq!(size_of::<Link>(), size_of::<usize>());
    assert!(size_of::<OwnedRecord>() < size_of::<NaiveRecord>());
    assert_eq!(size_of::<OwnedNode<'_>>(), 2 * size_of::<usize>());
    println!(
        "owned_offsets content={} source={} parent={} first={} last={} previous={} next={} size={}",
        std::mem::offset_of!(OwnedRecord, content),
        std::mem::offset_of!(OwnedRecord, source),
        std::mem::offset_of!(OwnedRecord, parent),
        std::mem::offset_of!(OwnedRecord, first_child),
        std::mem::offset_of!(OwnedRecord, last_child),
        std::mem::offset_of!(OwnedRecord, previous_sibling),
        std::mem::offset_of!(OwnedRecord, next_sibling),
        std::mem::offset_of!(OwnedRecord, subtree_size),
    );
}

#[test]
fn linked_preorder_visits_a_later_slot_before_the_epilog_and_bounds_subtrees() {
    let mut document = OwnedDocument::parse("<r><a/></r><!--end-->").unwrap();
    let root = document.root_element;
    let previous = document.nodes[root.0].last_child;
    let late = NodeId(document.nodes.len());
    let late_link = Link::new(late);
    document.nodes.push(OwnedRecord {
        id: late,
        content: Content::Comment(0..0),
        source: None,
        parent: Link::new(root),
        first_child: Link::NONE,
        last_child: Link::NONE,
        previous_sibling: previous,
        next_sibling: Link::NONE,
        subtree_size: 1,
    });
    document.nodes[previous.get().unwrap().0].next_sibling = late_link;
    document.nodes[root.0].last_child = late_link;
    document.nodes[root.0].subtree_size += 1;
    document.nodes[0].subtree_size += 1;
    document.slots_are_preorder = false;

    // Independently derive preorder with a test-owned stack, even when the
    // physical slot order differs.
    let mut expected = Vec::new();
    let mut pending = vec![document.root()];
    while let Some(node) = pending.pop() {
        expected.push(node.record.id.0);
        let children: Vec<_> = node.children().collect();
        pending.extend(children.into_iter().rev());
    }
    assert_eq!(expected, [0, 1, 2, 4, 3]);

    assert_eq!(
        document
            .nodes()
            .map(|node| node.record.id.0)
            .collect::<Vec<_>>(),
        [0, 1, 2, 4, 3]
    );
    for (node, expected) in [
        (document.root(), vec![0, 1, 2, 4, 3]),
        (document.root_element(), vec![1, 2, 4]),
        (document.root_element().first_child().unwrap(), vec![2]),
        (document.root().last_child().unwrap(), vec![3]),
    ] {
        let mut iterator = node.descendants();
        assert_eq!(node.descendants().count(), expected.len());
        for (position, expected) in expected.iter().copied().enumerate() {
            assert_eq!(iterator.len(), node.record().subtree_size - position);
            assert_eq!(iterator.size_hint(), (iterator.len(), Some(iterator.len())));
            assert_eq!(iterator.next().unwrap().record.id.0, expected);
        }
        assert_eq!(iterator.len(), 0);
        assert_eq!(iterator.size_hint(), (0, Some(0)));
        assert!(iterator.next().is_none());
        assert!(iterator.next().is_none());
        assert_eq!(iterator.count(), 0);
        let mut remaining = node.descendants();
        remaining.next();
        assert_eq!(remaining.count(), expected.len() - 1);
    }
    let late = document.root_element().last_child().unwrap();
    assert_eq!(late.range(), None);
    assert_eq!(late.text(), Some(""));
    assert_eq!(document.node(late.id()).unwrap().id(), late.id());
    let mut writer = crate::Writer::new(Vec::new());
    writer.write_owned_document(&document).unwrap();
    assert_eq!(writer.into_inner(), b"<r><a/><!----></r><!--end-->");
}

#[test]
fn removed_slots_are_rejected_by_lookup_and_existing_edit_boundaries() {
    let mut document = OwnedDocument::parse("<r/><!--removed-->").unwrap();
    let removed = document.root().last_child().unwrap().id();
    document.nodes[removed.slot.0] = OwnedRecord {
        id: removed.slot,
        content: Content::Removed,
        source: None,
        parent: Link::NONE,
        first_child: Link::NONE,
        last_child: Link::NONE,
        previous_sibling: Link::NONE,
        next_sibling: Link::NONE,
        subtree_size: 0,
    };
    let root = document.root_element;
    document.nodes[root.0].next_sibling = Link::NONE;
    document.nodes[0].last_child = Link::new(root);
    document.nodes[0].subtree_size = 2;
    document.slots_are_preorder = false;
    assert!(document.node(removed).is_none());
    assert_eq!(document.nodes().count(), 2);
    let mut editor = document.edit(EditLimits::default()).unwrap();
    for result in [
        editor.set_node_value(removed, "valid"),
        editor.set_attribute_value(removed, None, "a", "valid"),
    ] {
        assert_eq!(
            result,
            Err(EditError::new(EditErrorKind::InvalidHandle, None))
        );
    }
}

#[test]
fn value_edits_preserve_both_contiguous_and_linked_traversal_modes() {
    let mut document = OwnedDocument::parse("<r a='v'>old<c/>tail</r>").unwrap();
    assert!(document.slots_are_preorder);
    let expected: Vec<_> = document.nodes().map(|node| node.id()).collect();
    let text = document.root_element().first_child().unwrap().id();
    let element = document.root_element().id();
    for contiguous in [true, false] {
        document.slots_are_preorder = contiguous;
        let mut editor = document.edit(EditLimits::default()).unwrap();
        editor.set_node_value(text, "updated").unwrap();
        editor
            .set_attribute_value(element, None, "a", "updated")
            .unwrap();
        assert_eq!(editor.document().slots_are_preorder, contiguous);
        for id in &expected {
            assert_eq!(editor.document().node(*id).unwrap().id(), *id);
        }
        assert_eq!(
            editor
                .document()
                .nodes()
                .map(|node| node.id())
                .collect::<Vec<_>>(),
            expected
        );
    }
}

#[test]
fn linked_mode_walks_deep_and_wide_trees_without_recursion() {
    std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(|| {
            for (input, depth, count) in [
                (
                    format!("{}{}", "<n>".repeat(8192), "</n>".repeat(8192)),
                    8192,
                    8193,
                ),
                (format!("<r>{}</r>", "<c/>".repeat(20_000)), 256, 20_002),
            ] {
                let mut document = OwnedDocument::with_limits(
                    input.as_bytes(),
                    Limits {
                        max_depth: depth,
                        ..Limits::default()
                    },
                )
                .unwrap();
                document.slots_are_preorder = false;
                let mut nodes = document.nodes();
                for index in 0..count {
                    assert_eq!(nodes.len(), count - index);
                    assert_eq!(nodes.next().unwrap().record.id.0, index);
                }
                assert_eq!(nodes.size_hint(), (0, Some(0)));
                assert!(nodes.next().is_none());
                assert!(nodes.next().is_none());
            }
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn owned_builder_errors_are_not_remapped_or_replaced_by_cancellation() {
    use std::ops::ControlFlow;
    let builder = OwnedBuilder::new(0, 0, Global).unwrap();
    assert_eq!(
        builder
            .finish(10, None, Default::default(), Vec::new())
            .unwrap_err(),
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
            assert_eq!(range.input_range(), Some(2..10));
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

#[test]
fn node_reservation_follows_markup_not_length() {
    let text = format!("<r>{}</r>", "text ".repeat(100_000));
    let flat = format!("<r>{}</r>", "<i a='v'>t</i>".repeat(10_000));
    let late = format!("<r>{}{}</r>", "t".repeat(20_000), "<i/>".repeat(10_000));
    // (input, expected_nodes, nodes in the parsed document)
    let table: &[(&str, usize, usize)] = &[
        ("", 0, 0),
        ("<r/>", 3, 2),
        ("<r>t</r>", 5, 3),
        (&text, 91, 3),
        (&flat, 4096, 20_002),
        // A text prefix underestimates; the vector grows as usual.
        (&late, 10, 10_003),
    ];
    for &(input, expected, nodes) in table {
        assert_eq!(
            expected_nodes(input.as_bytes()),
            expected,
            "{}",
            &input[..input.len().min(20)]
        );
        if nodes == 0 {
            continue;
        }
        let document = OwnedDocument::parse(input).unwrap();
        assert_eq!(document.nodes.len(), nodes);
        assert!(document.nodes.capacity() <= (1 + expected).max(nodes) * 2);
    }
}
