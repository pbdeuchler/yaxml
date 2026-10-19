//! `save::to_vec` over save options and output encodings, derived from a hash
//! of the input so every input is a document. Both DOMs must agree, only
//! markup an encoding lacks may fail, and saving the reparsed output again
//! must reproduce it byte for byte: the output is a fixed point, which
//! checks the regenerated internal subset, escaping, references, formatting
//! and the namespace declarations a subtree gains.

use yaxml::{Document, ErrorKind, OwnedDocument, OwnedNodeKind, ParseOptions, options, save};

const FLAGS: [u32; 8] = [
    save::FORMAT,
    save::NO_DECL,
    save::NO_EMPTY,
    save::NO_XHTML,
    save::XHTML,
    save::WSNONSIG,
    save::NO_INDENT,
    save::EMPTY,
];

const ENCODINGS: [Option<&str>; 8] = [
    None,
    Some("UTF-8"),
    Some("UTF-16"),
    Some("UTF-16BE"),
    Some("ISO-8859-1"),
    Some("US-ASCII"),
    Some("windows-1252"),
    Some("ISO-8859-7"),
];

const INDENTS: [&str; 3] = ["  ", "\t", " \t "];

pub(super) fn exercise(input: &[u8]) {
    let seed = input.iter().fold(0xcbf2_9ce4_8422_2325_u64, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3)
    });
    // Without `NOENT` content and attribute references are kept and saved as
    // `&name;`; with it they are substituted.
    let parse = (seed >> 63 == 1) as u32 * options::NOENT;
    let Ok(owned) = OwnedDocument::parse_with(input, ParseOptions::new(parse)) else {
        return;
    };
    let flags = FLAGS
        .iter()
        .enumerate()
        .filter(|(index, _)| seed >> index & 1 == 1)
        .fold(0, |flags, (_, flag)| flags | flag);
    // XHTML additions are separated by one space even under WSNONSIG, as in
    // libxml2, so that pair is not a fixed point.
    let flags = if flags & save::WSNONSIG != 0 {
        flags & !save::XHTML | save::NO_XHTML
    } else {
        flags
    };
    let options = save::Options {
        flags,
        encoding: ENCODINGS[(seed >> 8) as usize % ENCODINGS.len()],
        indent: INDENTS[(seed >> 11) as usize % INDENTS.len()],
    };
    if seed >> 24 & 7 == 0 {
        rejected_options(&owned, seed);
    }
    let output = save::to_vec(&owned, &options);
    let borrowed = || Document::parse_with(input, ParseOptions::new(parse));
    if let Ok(borrowed) = borrowed() {
        assert_eq!(
            save::to_vec(&borrowed, &options),
            output,
            "DOM disagreement"
        );
    }
    let label = options.encoding.or_else(|| {
        owned
            .declaration()
            .and_then(|declaration| declaration.encoding)
    });
    // A kept reference to an entity that is not declared (or not loaded)
    // cannot reparse, in libxml2 either.
    let resolvable = owned
        .nodes()
        .all(|node| node.entity_name().is_none() || node.first_child().is_some());
    match output {
        Ok(_) if !resolvable => {}
        Ok(output) => fixed_point(
            &output,
            parse,
            &save::Options {
                encoding: label,
                ..options
            },
        ),
        Err(error) => assert_lossy(error.kind, label),
    }
    // One subtree, chosen by the hash.
    let count = owned.nodes().len();
    let node = owned.nodes().nth((seed >> 16) as usize % count).unwrap();
    let subtree = save::to_vec(node, &options);
    if let Ok(borrowed) = borrowed() {
        let view = borrowed.nodes().nth((seed >> 16) as usize % count).unwrap();
        assert_eq!(save::to_vec(view, &options), subtree, "DOM disagreement");
    }
    match subtree {
        // A subtree has no declaration, so only UTF-8 output reparses.
        Ok(output)
            if node.kind() == OwnedNodeKind::Element
                && matches!(options.encoding, None | Some("UTF-8"))
                && node.descendants().all(|node| {
                    // References need the document's DTD to reparse.
                    node.entity_name().is_none()
                        && (0..node.attributes().len())
                            .all(|position| node.unexpanded_attribute(position).is_none())
                }) =>
        {
            let reparsed = OwnedDocument::parse_bytes(&output)
                .expect("an element subtree is a namespace-well-formed document");
            assert_eq!(
                save::to_vec(reparsed.root_element(), &options).unwrap(),
                output,
                "subtree output is not a fixed point"
            );
        }
        Ok(_) => {}
        Err(error) => assert_lossy(error.kind, options.encoding),
    }
}

/// Unknown flags and indents outside 1..=60 spaces or tabs are rejected;
/// `AS_HTML` writes an XML document node with the HTML serializer and
/// rejects any other node.
fn rejected_options(owned: &OwnedDocument, seed: u64) {
    let long = " ".repeat(61);
    let indent = ["", "x", " \n", &long][(seed >> 27) as usize & 3];
    let unsupported = Err(ErrorKind::UnsupportedOption);
    let flags = save::Options::new(1 << (11 + (seed >> 29 & 15)));
    assert_eq!(save::to_vec(owned, &flags).map_err(|e| e.kind), unsupported);
    let bad = save::Options {
        indent,
        ..save::Options::new(0)
    };
    assert_eq!(save::to_vec(owned, &bad).map_err(|e| e.kind), unsupported);
    let html = save::Options::new(save::AS_HTML);
    if let Err(error) = save::to_vec(owned, &html) {
        let lossy = [ErrorKind::UnsupportedEncoding, ErrorKind::InvalidCharacter];
        assert!(lossy.contains(&error.kind), "{error:?}");
    }
    let child = owned.root().first_child().expect("a document has a child");
    assert_eq!(save::to_vec(child, &html).map_err(|e| e.kind), unsupported);
}

/// Only markup an encoding cannot represent fails.
fn assert_lossy(kind: ErrorKind, label: Option<&str>) {
    assert_eq!(kind, ErrorKind::InvalidCharacter);
    assert!(
        label.is_some_and(|label| !label.to_ascii_uppercase().starts_with("UTF")),
        "{label:?} represents every character"
    );
}

/// Saving the reparsed output in the same encoding reproduces it, when it
/// can be reparsed: without a declaration, only UTF-8, ASCII-only output or
/// UTF-16 behind a BOM is detected.
fn fixed_point(output: &[u8], parse: u32, options: &save::Options<'_>) {
    let detected = match options.encoding.map(str::to_ascii_uppercase).as_deref() {
        None | Some("UTF-8") => true,
        Some("UTF-16") => output.starts_with(&[0xFF, 0xFE]),
        Some(label) => !label.starts_with("UTF") && output.is_ascii(),
    };
    if options.flags & save::NO_DECL != 0 && !detected {
        return;
    }
    let resave = |output: &[u8]| {
        let reparsed = OwnedDocument::parse_with(output, ParseOptions::new(parse));
        let reparsed = reparsed.unwrap_or_else(|error| {
            panic!(
                "saved output does not reparse ({error}): {:?}",
                String::from_utf8_lossy(output)
            )
        });
        save::to_vec(&reparsed, options).unwrap()
    };
    // libxml2 normalizes `(a? | b? | c)+` to `(a? | b | c)*` and that to
    // `(a | b | c)*`, so a content model can take one more round to settle.
    let second = resave(output);
    if second != output {
        assert_eq!(resave(&second), second, "saved output does not settle");
    }
}
