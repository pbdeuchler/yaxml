//! Parser options across surfaces. A leading byte with its high bit set
//! selects `xmlParserOption` flags (it can never start XML, so plain inputs
//! keep their meaning and run with no flags); the rest is the document.

use std::io::BufReader;
use std::ops::ControlFlow;

use yaxml::options::{
    DTDATTR, DTDVALID, HUGE, IGNORE_ENC, NOBLANKS, NOCDATA, NOENT, NSCLEAN, RECOVER, SKIP_IDS,
    XINCLUDE,
};
use yaxml::{
    BufferedReader, Document, ErrorKind, Event, Limits, NodeKind, OwnedDocument, ParseOptions,
    PushParser, Reader, TextReader, save,
};

/// Flags the slice reader lacks; push and pull streams drop blanks too.
const TREE_ONLY: u32 = NOBLANKS | DTDVALID;

/// Bits 0..=6 of a selector byte, in order.
const SELECTABLE: [u32; 7] = [
    NOENT,
    DTDATTR,
    NOCDATA | NSCLEAN,
    RECOVER,
    NOBLANKS,
    DTDVALID,
    IGNORE_ENC | SKIP_IDS,
];

/// The selected flags and document.
pub(super) fn split(input: &[u8]) -> (u32, &[u8]) {
    match input.split_first() {
        Some((&selector, document)) if selector & 0x80 != 0 => {
            let flags = SELECTABLE
                .iter()
                .enumerate()
                .filter(|(bit, _)| selector >> bit & 1 == 1)
                .fold(0, |flags, (_, flag)| flags | flag);
            (flags, document)
        }
        _ => (0, input),
    }
}

pub(super) fn exercise(input: &[u8]) {
    // Without a selector the other drivers already cover the default flags.
    if input.first().is_some_and(|byte| byte & 0x80 != 0) {
        let (flags, document) = split(input);
        exercise_flags(flags, document);
    }
}

/// Every surface on `document` under `flags`.
pub(super) fn exercise_flags(flags: u32, document: &[u8]) {
    // `HUGE` only raises ceilings; streams reject tree-only flags.
    let huge = ParseOptions::new(flags | HUGE);
    let (limits, _) = huge.effective_limits();
    assert!(limits.max_depth >= 2048 && format!("{huge:?}").contains("flags"));
    assert!(PushParser::with_options(ParseOptions::new(flags | XINCLUDE)).is_err());
    if flags & RECOVER != 0 {
        return check_recovered(flags, document);
    }
    let stream = flags & !TREE_ONLY;
    let tree = Document::parse_with(document, ParseOptions::new(flags));
    let reader = events(document, stream);
    assert_eq!(
        tree.is_ok(),
        reader.is_some(),
        "Document/Reader acceptance differs under {flags:#x}"
    );
    decoding_surfaces_agree(document, flags);
    let Ok(tree) = tree else {
        return;
    };
    check_tree(&tree, flags);
    if let Some(events) = &reader
        && flags & NOBLANKS == 0
    {
        super::check_projection(&tree, events);
    }
    // Substitution changes structure, never acceptance or string values.
    let substituted = Document::parse_with(document, ParseOptions::new(flags | NOENT));
    let substituted = substituted.expect("NOENT changed acceptance");
    // Kept attribute references never change a value (xmlGetProp substitutes).
    let attributes = |document: &Document<'_>| -> Vec<String> {
        let elements = document.nodes().filter(|node| node.name().is_some());
        let values = elements.flat_map(|node| node.attributes().iter());
        values
            .map(|attribute| attribute.value.to_string())
            .collect()
    };
    assert_eq!(
        attributes(&tree),
        attributes(&substituted),
        "kept references changed attribute values"
    );
    if flags & NOENT != 0 {
        let unexpanded = tree.nodes().flat_map(|node| {
            (0..node.attributes().len()).filter_map(move |at| node.unexpanded_attribute(at))
        });
        assert_eq!(unexpanded.count(), 0, "NOENT kept an attribute reference");
    }
    if flags & NOBLANKS == 0 {
        assert!(
            tree.root().text_chunks().collect::<String>()
                == substituted.root().text_chunks().collect::<String>(),
            "references changed the document's string value: {:?}",
            String::from_utf8_lossy(document)
        );
    }
    let substituted_events = events(document, stream | NOENT).expect("NOENT stream rejected");
    assert_eq!(
        logical(reader.as_deref().unwrap_or_default()),
        logical(&substituted_events),
        "entity boundaries changed stream values"
    );
    // Both DOMs save identically when the owned DOM parses the same text.
    if !super::encoding::recognizes(document)
        && let Ok(owned) = OwnedDocument::parse_with(document, ParseOptions::new(flags))
    {
        assert_eq!(
            save::to_vec(&tree, &Default::default()),
            save::to_vec(&owned, &Default::default()),
            "DOM save outputs differ under {flags:#x}"
        );
        assert_eq!(tree.validity().len(), owned.validity().len());
    }
}

/// The reader's events, or `None` when it rejects the document.
fn events(document: &[u8], flags: u32) -> Option<Vec<Event<'_>>> {
    let mut reader = Reader::with_options(document, ParseOptions::new(flags)).ok()?;
    let mut events = Vec::new();
    while let Some(event) = reader.read_event().ok()? {
        events.push(event);
    }
    Some(events)
}

/// Events without entity boundaries, adjacent text joined.
fn logical(events: &[Event<'_>]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for event in events {
        match (event, out.last_mut()) {
            (Event::EntityStart(_) | Event::EntityEnd, _) => {}
            (Event::Text(text), Some(last)) if last.starts_with("Text:") => last.push_str(text),
            (Event::Text(text), _) => out.push(format!("Text:{text}")),
            (event, _) => out.push(format!("{event:?}")),
        }
    }
    out
}

/// An event with its attributes as a plain list, however they are lent.
fn render(event: &Event<'_>) -> String {
    match event {
        Event::Start(element) | Event::Empty(element) => format!(
            "{} {:?} {:?}",
            matches!(event, Event::Empty(_)),
            element.name,
            &element.attributes[..]
        ),
        other => format!("{other:?}"),
    }
}

/// Push, buffered and text readers, and the owned DOM, decode alike.
fn decoding_surfaces_agree(document: &[u8], flags: u32) {
    let stream = flags & !DTDVALID;
    let mut push = PushParser::with_options(ParseOptions::new(stream)).unwrap();
    let mut pushed = Vec::new();
    let push_ok = push
        .feed(document, |event, _| {
            pushed.push(render(event));
            ControlFlow::Continue(())
        })
        .and_then(|()| {
            push.finish(|event, _| {
                pushed.push(render(event));
                ControlFlow::Continue(())
            })
        })
        .is_ok();
    let reader = BufReader::with_capacity(7, document);
    let mut buffered = BufferedReader::with_options(reader, ParseOptions::new(stream)).unwrap();
    let mut pulled = Vec::new();
    let pull_ok = loop {
        match buffered.read_event() {
            Ok(Some((event, _))) => pulled.push(render(&event)),
            Ok(None) => break true,
            Err(_) => break false,
        }
    };
    assert_eq!(
        push_ok, pull_ok,
        "push/buffered acceptance under {flags:#x}"
    );
    if push_ok {
        assert_eq!(pushed, pulled, "push/buffered events under {flags:#x}");
        assert_eq!(push.dtd().is_some(), buffered.dtd().is_some());
    }
    // The slice reader refuses what it lacks; push and pull need no tree but to validate.
    let options = || ParseOptions::new(flags);
    for (refused, lacks) in [
        (Reader::with_options(document, options()).err(), TREE_ONLY),
        (PushParser::with_options(options()).err(), DTDVALID),
        (
            BufferedReader::with_options(document, options()).err(),
            DTDVALID,
        ),
    ] {
        let unsupported = refused.is_some_and(|error| error.kind == ErrorKind::UnsupportedOption);
        assert_eq!(unsupported, flags & lacks != 0, "{flags:#x}");
    }
    // Push and pull drop blanks as the text reader does.
    if flags & NOBLANKS != 0 && push_ok {
        check_blanks(document, flags & !DTDVALID, &pushed);
    }
    // The text reader also drops blanks, which never changes acceptance.
    for flags in [stream, stream | NOBLANKS] {
        let mut text = TextReader::with_options(document, ParseOptions::new(flags)).unwrap();
        let text_ok = loop {
            match text.read() {
                Ok(true) => {}
                Ok(false) => break true,
                Err(_) => break false,
            }
        };
        assert_eq!(push_ok, text_ok, "TextReader acceptance under {flags:#x}");
    }
    let owned = OwnedDocument::parse_with(document, ParseOptions::new(flags));
    assert_eq!(push_ok, owned.is_ok(), "owned acceptance under {flags:#x}");
}

/// What each selected flag guarantees about the tree.
fn check_tree(document: &Document<'_>, flags: u32) {
    for node in document.nodes() {
        match node.kind() {
            NodeKind::EntityRef(_) => assert_eq!(flags & NOENT, 0, "NOENT kept a reference"),
            NodeKind::Cdata(_) => assert_eq!(flags & NOCDATA, 0, "NOCDATA kept a section"),
            NodeKind::Element(element) if flags & NSCLEAN != 0 => {
                for attribute in element.attributes.iter() {
                    let name = &attribute.name;
                    let prefix = match name.prefix() {
                        None if name.qualified() == "xmlns" => "",
                        Some("xmlns") => name.local(),
                        _ => continue,
                    };
                    assert_ne!(
                        inherited(node, prefix),
                        Some(attribute.value.as_ref()),
                        "NSCLEAN kept a redundant declaration"
                    );
                }
            }
            _ => {}
        }
        if flags & SKIP_IDS != 0 {
            for attribute in node.attributes() {
                assert!(document.element_by_id(&attribute.value).is_none());
            }
        }
    }
    if flags & DTDVALID == 0 {
        assert!(document.validity().is_empty());
    } else {
        assert_eq!(document.validity().len(), document.validate().len());
    }
}

/// The URI `prefix` is bound to by `element`'s ancestors.
fn inherited<'a>(element: yaxml::Node<'a, '_>, prefix: &str) -> Option<&'a str> {
    std::iter::successors(element.parent(), |node| node.parent()).find_map(|node| {
        node.attributes().iter().find_map(|attribute| {
            let name = &attribute.name;
            let bound = match name.prefix() {
                None if name.qualified() == "xmlns" => "",
                Some("xmlns") => name.local(),
                _ => return None,
            };
            (bound == prefix).then_some(attribute.value.as_ref())
        })
    })
}

/// `RECOVER`: the streams recover alike (see `recover`); a well-formed document parses as without it; a
/// recovered tree is structurally sound, lists only recoverable errors and
/// saves to bytes that recover to themselves; both DOMs agree; resource
/// limits still fail the parse.
fn check_recovered(flags: u32, document: &[u8]) {
    super::recover::exercise(
        document,
        flags & !(TREE_ONLY | RECOVER),
        super::selector(document),
    );
    let saved = |tree: &Document<'_>| save::to_vec(tree, &Default::default()).expect("save");
    let strict = Document::parse_with(document, ParseOptions::new(flags & !RECOVER));
    let recovered = Document::parse_with(document, ParseOptions::new(flags));
    if let Ok(strict) = &strict {
        let tree = recovered
            .as_ref()
            .expect("RECOVER rejected a well-formed document");
        assert!(tree.errors().is_empty(), "errors in a well-formed document");
        assert_eq!(
            saved(strict),
            saved(tree),
            "RECOVER changed a well-formed tree"
        );
    }
    if let (Err(error), Ok(_)) = (&strict, &recovered)
        && matches!(
            error.kind,
            ErrorKind::LimitExceeded(_) | ErrorKind::EntityLoop
        )
    {
        panic!("RECOVER bypassed {error:?}");
    }
    // The owned DOM decodes a declared charset (even one a malformed
    // declaration names), where the borrowed DOM recovers it as UTF-8, and
    // the slice and decoding readers can find different first errors. Only
    // the owned DOM repairs invalid UTF-8, as the borrowed one borrows text.
    let declared = document
        .windows(8)
        .any(|w| w.eq_ignore_ascii_case(b"encoding"));
    let owned = OwnedDocument::parse_with(document, ParseOptions::new(flags));
    if let Ok(owned) = &owned {
        assert!(owned.errors().len() <= 100);
        super::check_diagnostics(document, owned.errors());
        assert!(owned.errors().iter().all(|error| !matches!(
            error.kind,
            ErrorKind::LimitExceeded(_) | ErrorKind::Allocation | ErrorKind::EntityLoop
        )));
    }
    if !super::encoding::recognizes(document)
        && !declared
        && std::str::from_utf8(document).is_ok()
        && let (Ok(borrowed), Ok(owned)) = (&recovered, &owned)
    {
        let owned_saved = save::to_vec(owned, &Default::default()).unwrap();
        assert_eq!(saved(borrowed), owned_saved, "DOMs recover differently");
        assert_eq!(
            borrowed.errors(),
            owned.errors(),
            "DOMs recover differently"
        );
    }
    // Decoded input recovers only through the owned DOM's push parser.
    if (super::encoding::recognizes(document) || declared)
        && let Ok(owned) = OwnedDocument::parse_with(document, ParseOptions::new(flags))
    {
        assert!(owned.errors().len() <= 100);
    }
    // Only the owned DOM repairs invalid UTF-8, recording the repair.
    if !super::encoding::recognizes(document)
        && !declared
        && std::str::from_utf8(document).is_err()
        && let Ok(owned) = OwnedDocument::parse_with(document, ParseOptions::new(flags))
    {
        assert!(
            !owned.errors().is_empty(),
            "repaired UTF-8 without an error"
        );
    }
    let Ok(tree) = recovered else {
        return;
    };
    // The structural checks look `xml:id`s up, which `SKIP_IDS` does not record.
    if flags & SKIP_IDS == 0 {
        super::check_document(&tree, document.len(), Limits::default().max_events);
    }
    assert_eq!(tree.errors().is_empty(), strict.is_ok());
    assert!(tree.errors().len() <= 100);
    // Recovery makes progress: every node but the root starts at a later
    // input byte than the one before, so without entity references there
    // are no more nodes than bytes.
    if !document.contains(&b'&') {
        assert!(
            tree.nodes().len() <= document.len() + 1,
            "recovery reread input"
        );
    }
    for error in tree.errors() {
        assert!(error.offset <= document.len(), "{error:?}");
        assert!(!matches!(
            error.kind,
            ErrorKind::LimitExceeded(_) | ErrorKind::Allocation | ErrorKind::EntityLoop
        ));
    }
    let bytes = saved(&tree);
    // A BOM-detected label `IGNORE_ENC` recorded is saved, then ignored without it.
    let reparse = ParseOptions::new(flags & !IGNORE_ENC);
    let again = Document::parse_with(&bytes, reparse).unwrap_or_else(|error| {
        panic!("{error:?} reparsing {:?}", String::from_utf8_lossy(&bytes))
    });
    assert_eq!(saved(&again), bytes, "recovery is not a fixed point");
    // Tight limits hold under recovery.
    let limits = Limits::default().max_depth(4).max_attributes(2);
    if let Ok(tree) = Document::parse_with(document, ParseOptions::new(flags).with_limits(limits)) {
        for node in tree.nodes() {
            let elements = std::iter::successors(node.parent(), |node| node.parent());
            let depth = elements.filter(|node| node.name().is_some()).count();
            assert!(depth <= limits.max_depth, "depth {depth}");
        }
    }
}

/// `NOBLANKS` on push and pull, which deliver `kept`: only blank text is
/// dropped from the slice reader's events, and outside entity references
/// (whose content passes as it is) what is kept projects onto the DOM.
fn check_blanks(document: &[u8], flags: u32, kept: &[String]) {
    let Some(events) = events(document, flags & !NOBLANKS) else {
        return;
    };
    let mut remaining = kept.iter().peekable();
    let mut filtered = Vec::new();
    for event in events {
        if remaining.peek() == Some(&&render(&event)) {
            remaining.next();
            filtered.push(event);
        } else {
            let blank = matches!(&event, Event::Text(text) if text.bytes().all(|b| b" \t\r\n".contains(&b)));
            assert!(blank, "NOBLANKS dropped {event:?} from {document:?}");
        }
    }
    assert!(
        remaining.next().is_none(),
        "NOBLANKS changed events of {document:?}"
    );
    if !document.contains(&b'&')
        && let Ok(tree) = Document::parse_with(document, ParseOptions::new(flags))
    {
        super::check_projection(&tree, &filtered);
    }
}
