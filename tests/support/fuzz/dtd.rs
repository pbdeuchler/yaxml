//! Invariants of the public declaration views (`valid::Dtd::entity` and
//! friends): every declaration is consistent with its kind and its lookup,
//! stored text and content models are fixed points of re-declaration, and
//! the escapers' output reparses to the text they escaped.

use yaxml::valid::{AttributeKind, ContentKind, Dtd, Entity, EntityKind};
use yaxml::{Document, OwnedDocument, ParseOptions, chars, options, save};

/// Both DOMs under the default and the substituting options.
pub(super) fn exercise(input: &[u8]) {
    for flags in [0, options::NOENT | options::DTDATTR] {
        let owned = OwnedDocument::parse_with(input, ParseOptions::new(flags));
        let Some(dtd) = owned.as_ref().ok().and_then(OwnedDocument::dtd) else {
            continue;
        };
        check(dtd);
        if let Ok(borrowed) = Document::parse_with(input, ParseOptions::new(flags)) {
            assert_eq!(
                listing(borrowed.dtd().expect("both DOMs keep the DTD")),
                listing(dtd)
            );
        }
    }
}

fn listing(dtd: &Dtd) -> String {
    let entities = dtd.entities().map(|e| format!("{e:?}"));
    let elements = dtd.elements().flat_map(|e| {
        std::iter::once(format!("{e:?}")).chain(e.attributes().map(|a| format!("{a:?}")))
    });
    let notations = dtd.notations().map(|n| format!("{n:?}"));
    entities
        .chain(elements)
        .chain(notations)
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn check(dtd: &Dtd) {
    for entity in dtd.entities() {
        check_entity(dtd, entity);
    }
    for element in dtd.elements() {
        assert_eq!(
            dtd.element(element.name).map(|e| e.name),
            Some(element.name)
        );
        let printed = matches!(element.kind, ContentKind::Mixed | ContentKind::Element);
        assert_eq!(element.model.is_some(), printed, "{element:?}");
        if let Some(model) = element.model {
            assert!(model.starts_with('('), "{model}");
            let again = redeclare(&format!("<!ELEMENT x {model}>"));
            let reparsed = again.as_ref().and_then(|dtd| dtd.element("x"));
            assert_eq!(
                reparsed.map(|e| (e.kind, e.model)),
                Some((element.kind, Some(model))),
                "model {model} is not a fixed point"
            );
        }
        for attribute in element.attributes() {
            assert_eq!(dtd.attribute(element.name, attribute.name), Some(attribute));
            assert_eq!(attribute.element, element.name);
            let listed = matches!(
                attribute.kind,
                AttributeKind::Enumeration | AttributeKind::Notation
            );
            assert_eq!(!attribute.values.is_empty(), listed, "{attribute:?}");
        }
    }
    for notation in dtd.notations() {
        assert_eq!(dtd.notation(notation.name), Some(notation));
        assert!(notation.public_id.is_some() || notation.system_id.is_some());
    }
}

fn check_entity(dtd: &Dtd, entity: Entity<'_>) {
    let internal = matches!(
        entity.kind,
        EntityKind::InternalGeneral | EntityKind::InternalParameter
    );
    let parameter = matches!(
        entity.kind,
        EntityKind::InternalParameter | EntityKind::ExternalParameter
    );
    assert_ne!(
        entity.kind,
        EntityKind::Predefined,
        "declarations are never predefined"
    );
    assert_eq!(entity.system_id.is_none(), internal, "{entity:?}");
    assert_eq!(entity.uri.is_none(), internal, "{entity:?}");
    assert!(!internal || entity.public_id.is_none());
    assert_eq!(
        entity.notation.is_some(),
        entity.kind == EntityKind::ExternalUnparsedGeneral
    );
    if entity.kind == EntityKind::ExternalUnparsedGeneral {
        assert_eq!(
            entity.content, entity.notation,
            "libxml2 stores the notation as content"
        );
    }
    if internal {
        let content = entity.content.expect("internal entities have text");
        assert!(content.chars().all(chars::is_char), "{content:?}");
        reparse(entity.name, content, parameter);
    }
    if parameter {
        assert_eq!(dtd.parameter_entity(entity.name), Some(entity));
    } else if let Some(found) = dtd.entity(entity.name) {
        // A later bound declaration can shadow an unbound standalone one.
        assert_eq!(found.name, entity.name);
        assert!(
            found.kind != EntityKind::Predefined
                || matches!(entity.name, "lt" | "gt" | "amp" | "apos" | "quot")
        );
    }
    escapes(content_or_name(entity));
}

fn content_or_name<'a>(entity: Entity<'a>) -> &'a str {
    entity.content.unwrap_or(entity.name)
}

/// A DTD holding only `declarations`, or `None` when it does not parse.
fn redeclare(declarations: &str) -> Option<Dtd> {
    let document = OwnedDocument::parse(&format!("<!DOCTYPE r [{declarations}]><r/>")).ok()?;
    document.dtd().cloned()
}

/// Stored replacement text without character references is a fixed point:
/// declaring it again (quotes, `%` and CR escaped) stores the same text.
fn reparse(name: &str, content: &str, parameter: bool) {
    if content.contains("&#") {
        return;
    }
    let literal = content
        .replace('%', "&#37;")
        .replace('"', "&#34;")
        .replace('\r', "&#13;");
    let declaration = match parameter {
        true => format!("<!ENTITY % x \"{literal}\">"),
        false => format!("<!ENTITY x \"{literal}\">"),
    };
    let Some(dtd) = redeclare(&declaration) else {
        // A bare `&` from `&#38;` cannot be written back without a reference.
        assert!(
            content.contains('&'),
            "{name}: {content:?} does not reparse"
        );
        return;
    };
    let again = match parameter {
        true => dtd.parameter_entity("x"),
        false => dtd.entity("x"),
    };
    assert_eq!(again.and_then(|e| e.content), Some(content), "{name}");
}

/// Escaped text reparses as the same element content, except C0 controls,
/// which the escapers replace with U+FFFD.
fn escapes(text: &str) {
    if text
        .chars()
        .any(|c| (c as u32) < 0x20 && !chars::is_blank(c))
    {
        return;
    }
    for (escaped, ascii) in [
        (save::encode_entities(text, save::Escape::Ascii), true),
        (save::encode_entities(text, save::Escape::Xml), false),
        (save::encode_special_chars(text), false),
    ] {
        assert!(!ascii || escaped.is_ascii(), "{escaped:?}");
        let xml = format!("<r>{escaped}</r>");
        let document = Document::parse(&xml).expect("escaped text parses");
        let value: String = document.root_element().text_chunks().collect();
        assert_eq!(value, text, "{escaped:?}");
    }
}
