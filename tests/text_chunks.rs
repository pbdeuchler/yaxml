#![forbid(unsafe_code)]

use yaxml::{Document, EditLimits, Limits, OwnedDocument};

fn assert_chunks<'a, I: Iterator<Item = &'a str>>(make: impl Fn() -> I, expected: &[&str]) {
    assert_eq!(make().collect::<Vec<_>>(), expected);
    assert_eq!(make().collect::<String>(), expected.concat());
    assert_eq!(make().count(), expected.len());
    assert_eq!(make().last(), expected.last().copied());
    let mut iterator = make();
    for (index, chunk) in expected.iter().enumerate() {
        let remaining = expected.len() - index;
        let (lower, upper) = iterator.size_hint();
        assert!(lower <= remaining);
        assert!(upper.is_none_or(|upper| upper >= remaining));
        assert_eq!(iterator.next(), Some(*chunk));
    }
    assert_eq!(iterator.next(), None);
    assert_eq!(iterator.next(), None);
    assert_eq!(iterator.nth(1), None);
    for skip in [0, expected.len() / 2, expected.len(), usize::MAX] {
        let mut iterator = make();
        assert_eq!(iterator.nth(skip), expected.get(skip).copied());
        assert_eq!(
            iterator.count(),
            expected.len().saturating_sub(skip.saturating_add(1))
        );
    }
}

#[test]
fn every_node_kind_has_the_exact_string_value_without_sibling_leaks() {
    let input = "<?xml version='1.0'?><!--pre--><?before outer?><r a='ignored'>A&amp;<empty/><c>中&#13;<![CDATA[😀]]></c><!--inside--><?inside payload?><tail>z</tail>B</r><!--post--><?after end?>";
    let expected: &[&[&str]] = &[
        &["A&", "中\r", "😀", "z", "B"],
        &["pre"],
        &["outer"],
        &["A&", "中\r", "😀", "z", "B"],
        &["A&"],
        &[],
        &["中\r", "😀"],
        &["中\r"],
        &["😀"],
        &["inside"],
        &["payload"],
        &["z"],
        &["z"],
        &["B"],
        &["post"],
        &["end"],
    ];
    let borrowed = Document::parse(input).unwrap();
    let owned = OwnedDocument::parse(input).unwrap();
    assert_eq!(borrowed.nodes().len(), expected.len());
    assert_eq!(owned.nodes().len(), expected.len());
    for ((borrowed, owned), expected) in borrowed.nodes().zip(owned.nodes()).zip(expected) {
        assert_chunks(|| borrowed.text_chunks(), expected);
        assert_chunks(|| owned.text_chunks(), expected);
    }
    assert_eq!(borrowed.root_element().text(), None);
    assert_eq!(owned.root_element().text(), None);
}

#[test]
fn empty_leaves_remain_chunks_and_empty_containers_have_none() {
    for input in ["<r/>", "<r><e/></r>", "<r><!--ignored--><?p ignored?></r>"] {
        let borrowed = Document::parse(input).unwrap();
        let owned = OwnedDocument::parse(input).unwrap();
        assert_chunks(|| borrowed.root().text_chunks(), &[]);
        assert_chunks(|| owned.root().text_chunks(), &[]);
    }
    let borrowed = Document::parse("<r><!----><?p?><![CDATA[]]></r>").unwrap();
    let owned = OwnedDocument::parse("<r><!----><?p?><![CDATA[]]></r>").unwrap();
    assert_chunks(|| borrowed.root().text_chunks(), &[""]);
    assert_chunks(|| owned.root().text_chunks(), &[""]);
    for (borrowed, owned) in borrowed
        .root_element()
        .children()
        .zip(owned.root_element().children())
    {
        assert_chunks(|| borrowed.text_chunks(), &[""]);
        assert_chunks(|| owned.text_chunks(), &[""]);
    }
}

#[test]
fn normalization_and_utf16_input_preserve_logical_chunks() {
    let source = "<r xmlns:p='urn:p'>a\r\nb\rc&#13;&#10;&amp;<p:c><![CDATA[x\r\ny]]></p:c></r>";
    let expected = ["a\nb\nc\r\n&", "x\ny"];
    let borrowed = Document::parse(source).unwrap();
    assert_chunks(|| borrowed.root().text_chunks(), &expected);
    for little in [true, false] {
        let mut encoded = if little {
            vec![0xff, 0xfe]
        } else {
            vec![0xfe, 0xff]
        };
        for unit in source.encode_utf16() {
            encoded.extend_from_slice(&if little {
                unit.to_le_bytes()
            } else {
                unit.to_be_bytes()
            });
        }
        let owned = OwnedDocument::parse_bytes(&encoded).unwrap();
        drop(encoded);
        assert_chunks(|| owned.root().text_chunks(), &expected);
    }
}

#[test]
fn owned_edits_keep_empty_adjacent_chunks_and_live_document_order() {
    let mut document =
        OwnedDocument::parse("<!--pre--><r>old<separator/>tail</r><!--post-->").unwrap();
    let root = document.root_element().id();
    let text = document.root_element().first_child().unwrap().id();
    let separator = document.node(text).unwrap().next_sibling().unwrap().id();
    let (empty, cdata, comment, pi);
    {
        let mut editor = document.edit(EditLimits::default()).unwrap();
        editor.set_node_value(text, "new").unwrap();
        editor.remove_subtree(separator).unwrap();
        empty = editor.append_text(root, "").unwrap();
        cdata = editor.append_cdata(root, "<data>").unwrap();
        comment = editor.append_comment(root, "ignored").unwrap();
        pi = editor
            .append_processing_instruction(root, "p", "ignored")
            .unwrap();
        editor.append_text(root, "last").unwrap();
        editor
            .append_comment(editor.document().root().id(), "epilog")
            .unwrap();
    }
    assert!(document.node(separator).is_none());
    assert_chunks(
        || document.root().text_chunks(),
        &["new", "tail", "", "<data>", "last"],
    );
    assert_chunks(
        || document.root_element().text_chunks(),
        &["new", "tail", "", "<data>", "last"],
    );
    assert_chunks(|| document.node(empty).unwrap().text_chunks(), &[""]);
    assert_chunks(|| document.node(cdata).unwrap().text_chunks(), &["<data>"]);
    assert_chunks(
        || document.node(comment).unwrap().text_chunks(),
        &["ignored"],
    );
    assert_chunks(|| document.node(pi).unwrap().text_chunks(), &["ignored"]);
    document
        .edit(EditLimits::default())
        .unwrap()
        .remove_subtree(cdata)
        .unwrap();
    assert_chunks(
        || document.root().text_chunks(),
        &["new", "tail", "", "last"],
    );
}

#[test]
fn deeply_nested_chunks_are_iterative_before_and_after_owned_edits() {
    std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(|| {
            const DEPTH: usize = 8_192;
            let source = format!(
                "<r>start{}middle{}end</r>",
                "<n>".repeat(DEPTH),
                "</n>".repeat(DEPTH)
            );
            let limits = Limits::default().max_depth(DEPTH + 1);
            let borrowed = Document::with_limits(source.as_bytes(), limits).unwrap();
            assert_chunks(
                || borrowed.root().text_chunks(),
                &["start", "middle", "end"],
            );
            let mut owned = OwnedDocument::with_limits(source.as_bytes(), limits).unwrap();
            assert_chunks(|| owned.root().text_chunks(), &["start", "middle", "end"]);
            let nested = owned
                .root_element()
                .first_child()
                .unwrap()
                .next_sibling()
                .unwrap()
                .id();
            let deepest = owned
                .node(nested)
                .unwrap()
                .descendants()
                .last()
                .unwrap()
                .parent()
                .unwrap()
                .id();
            owned
                .edit(EditLimits::default())
                .unwrap()
                .append_text(deepest, "!")
                .unwrap();
            assert_chunks(
                || owned.root().text_chunks(),
                &["start", "middle", "!", "end"],
            );
            owned
                .edit(EditLimits::default())
                .unwrap()
                .remove_subtree(nested)
                .unwrap();
            assert_chunks(|| owned.root().text_chunks(), &["start", "end"]);
        })
        .unwrap()
        .join()
        .unwrap();
}
