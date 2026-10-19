//! Validated names are caller input independently of the XML byte grammar.

use std::borrow::Cow;

use yaxml::{Element, Error, ErrorKind, Event, Name, Reader, Writer};

#[derive(Clone, Copy)]
enum Expected {
    Name {
        prefix: Option<&'static str>,
        local: &'static str,
    },
    Invalid(usize),
}

const CASES: &[(&str, Expected)] = &[
    (
        "r",
        Expected::Name {
            prefix: None,
            local: "r",
        },
    ),
    (
        "p:r",
        Expected::Name {
            prefix: Some("p"),
            local: "r",
        },
    ),
    (
        "π:δ",
        Expected::Name {
            prefix: Some("π"),
            local: "δ",
        },
    ),
    (
        "é",
        Expected::Name {
            prefix: None,
            local: "é",
        },
    ),
    (
        "_a-0.1",
        Expected::Name {
            prefix: None,
            local: "_a-0.1",
        },
    ),
    (
        "r\u{b7}\u{300}",
        Expected::Name {
            prefix: None,
            local: "r\u{b7}\u{300}",
        },
    ),
    (
        "\u{200c}:r",
        Expected::Name {
            prefix: Some("\u{200c}"),
            local: "r",
        },
    ),
    (
        "\u{effff}",
        Expected::Name {
            prefix: None,
            local: "\u{effff}",
        },
    ),
    (
        "xmlns:p",
        Expected::Name {
            prefix: Some("xmlns"),
            local: "p",
        },
    ),
    (
        "xml:lang",
        Expected::Name {
            prefix: Some("xml"),
            local: "lang",
        },
    ),
    ("", Expected::Invalid(0)),
    (":r", Expected::Invalid(0)),
    ("p:", Expected::Invalid(2)),
    ("a:b:c", Expected::Invalid(3)),
    ("a::c", Expected::Invalid(2)),
    ("1r", Expected::Invalid(0)),
    ("a:1r", Expected::Invalid(2)),
    ("r x", Expected::Invalid(1)),
    (".r", Expected::Invalid(0)),
    ("r\0", Expected::Invalid(1)),
    ("é/", Expected::Invalid(2)),
    ("é:1", Expected::Invalid(3)),
    ("\u{a0}", Expected::Invalid(0)),
    ("\u{b7}", Expected::Invalid(0)),
    ("\u{300}", Expected::Invalid(0)),
    ("\u{3000}", Expected::Invalid(0)),
    ("\u{f0000}", Expected::Invalid(0)),
    ("r\u{ffff}", Expected::Invalid(1)),
];

pub(super) fn exercise(input: &[u8]) {
    let selector = usize::from(input.first().copied().unwrap_or(0));
    let namespace = match input.get(1).copied().unwrap_or(0) % 4 {
        0 => None,
        1 => Some(""),
        2 => Some("urn:caller"),
        _ => Some("urn:π"),
    };
    let (qualified, expected) = CASES[selector % CASES.len()];
    check_literal(qualified, namespace, expected);
    if selector & 15 == 0 {
        check_owned_pi(input.get(1).copied().unwrap_or(0));
    }

    // Keep arbitrary caller-controlled strings bounded independently of the
    // XML driver's byte ceiling, including malformed UTF-8 via replacement.
    let arbitrary = String::from_utf8_lossy(&input[..input.len().min(96)]);
    match Name::new(arbitrary.as_ref(), namespace.map(Cow::Borrowed)) {
        Ok(name) => {
            check_views(&name, &arbitrary, namespace);
            assert_eq!(name.qualified().as_ptr(), arbitrary.as_ptr());
            let owned = Name::new(
                arbitrary.as_ref().to_owned(),
                namespace.map(|value| Cow::Owned(value.to_owned())),
            )
            .unwrap();
            assert_eq!(name, owned);
            check_views(&owned, &arbitrary, namespace);
        }
        Err(error) => {
            assert_eq!(error.kind, ErrorKind::InvalidName);
            assert!(error.offset <= arbitrary.len());
            assert!(arbitrary.is_char_boundary(error.offset));
            assert_eq!(
                Name::new(arbitrary.as_ref().to_owned(), namespace.map(Cow::Borrowed)).unwrap_err(),
                error
            );
        }
    }
}

fn check_literal(qualified: &str, namespace: Option<&str>, expected: Expected) {
    let borrowed = Name::new(Cow::Borrowed(qualified), namespace.map(Cow::Borrowed));
    let owned_buffer = qualified.to_owned();
    let owned_pointer = owned_buffer.as_ptr();
    let owned = Name::new(
        Cow::Owned(owned_buffer),
        namespace.map(|value| Cow::Owned(value.to_owned())),
    );
    match expected {
        Expected::Name { prefix, local } => {
            let borrowed = borrowed.unwrap();
            let owned = owned.unwrap();
            assert_eq!(borrowed, owned);
            assert_eq!(borrowed.qualified().as_ptr(), qualified.as_ptr());
            assert_eq!(owned.qualified().as_ptr(), owned_pointer);
            for name in [&borrowed, &owned] {
                check_views(name, qualified, namespace);
                assert_eq!(name.prefix(), prefix);
                assert_eq!(name.local(), local);
            }
        }
        Expected::Invalid(offset) => {
            let error = Error {
                kind: ErrorKind::InvalidName,
                offset,
            };
            assert_eq!(borrowed.unwrap_err(), error, "{qualified:?}");
            assert_eq!(owned.unwrap_err(), error, "{qualified:?}");
        }
    }
}

fn check_owned_pi(selector: u8) {
    let target = format!("π{selector}");
    let data = format!("payload{selector}");
    let expected = format!("<?{target} {data}?><r/>");
    let event: Event<'static> = Event::ProcessingInstruction {
        target: Cow::Owned(target),
        data: Cow::Owned(data),
    };
    let mut writer = Writer::new(Vec::new());
    writer.write_event(&event).unwrap();
    writer
        .write_event(&Event::Empty(Element {
            name: Name::new("r", None).unwrap(),
            attributes: Vec::new().into(),
        }))
        .unwrap();
    writer.finish().unwrap();
    let output = writer.into_inner();
    assert_eq!(output, expected.as_bytes());
    let mut reader = Reader::new(&output).unwrap();
    assert_eq!(reader.read_event().unwrap().unwrap(), event);
    assert!(matches!(
        reader.read_event().unwrap(),
        Some(Event::Empty(_))
    ));
    assert!(reader.read_event().unwrap().is_none());
}

fn check_views(name: &Name<'_>, qualified: &str, namespace: Option<&str>) {
    assert_eq!(name.qualified(), qualified);
    assert_eq!(name.namespace(), namespace);
    match name.prefix() {
        Some(prefix) => {
            assert!(!prefix.is_empty());
            assert!(!name.local().is_empty());
            assert_eq!(format!("{prefix}:{}", name.local()), qualified);
        }
        None => assert_eq!(name.local(), qualified),
    }
}

#[cfg(test)]
mod tests {
    use super::{CASES, exercise};

    #[test]
    fn literal_name_catalog_covers_every_namespace_form() {
        for selector in 0..CASES.len() {
            for namespace in 0..4 {
                exercise(&[u8::try_from(selector).unwrap(), namespace]);
            }
        }
    }
}
