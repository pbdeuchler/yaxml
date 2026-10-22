//! Recorded expansions replay what expanding the entity again would
//! publish: the same events, origins and errors at every ceiling around
//! each document's threshold, on the reader and both DOMs.

use super::*;
use crate::{Document, Limits, OwnedDocument, ParseOptions, Reader, SourceOrigin};

thread_local! {
    /// Whether templates are used, and how many were replayed.
    pub(super) static TEMPLATES: std::cell::Cell<(bool, usize)> = const { std::cell::Cell::new((true, 0)) };
}

/// Events (by their debug form) with origins, and the error that ended them.
type Outcome = (Vec<(String, Option<SourceOrigin>)>, Option<Error>);

fn options(limits: Limits) -> ParseOptions {
    ParseOptions::new(crate::options::LEGACY).with_limits(limits)
}

fn read(input: &[u8], limits: Limits) -> Outcome {
    let mut events = Vec::new();
    let mut reader = match Reader::with_options(input, options(limits)) {
        Ok(reader) => reader,
        Err(error) => return (events, Some(error)),
    };
    loop {
        match reader.read_event() {
            Ok(Some(event)) => events.push((format!("{event:?}"), reader.event_origin())),
            Ok(None) => return (events, None),
            Err(error) => return (events, Some(error)),
        }
    }
}

/// Every surface's outcome, and the templates replayed for it.
fn parse(input: &[u8], limits: Limits, templates: bool) -> (Outcome, String, String, usize) {
    TEMPLATES.set((templates, 0));
    let save = crate::save::Options::DEFAULT;
    let dom = Document::parse_with(input, options(limits))
        .map(|document| crate::save::to_vec(&document, &save).expect("saved"));
    let owned = OwnedDocument::parse_with(input, options(limits))
        .map(|document| crate::save::to_vec(&document, &save).expect("saved"));
    let outcome = read(input, limits);
    let replays = TEMPLATES.get().1;
    (outcome, format!("{dom:?}"), format!("{owned:?}"), replays)
}

/// Each document references its entities often enough to be replayed,
/// unless its expansion is ineligible.
const DOCUMENTS: [(&str, &str, bool); 11] = [
    (
        "text",
        r#"<!DOCTYPE r [<!ENTITY z "zz">]><r>&z;&z;&z;<p>&z;</p>&z;</r>"#,
        true,
    ),
    (
        "elements",
        r#"<!DOCTYPE r [<!ENTITY q "<q a='1'>x<b/>y</q>">]><r>&q;&q;&q;&q;</r>"#,
        true,
    ),
    (
        "nested",
        r#"<!DOCTYPE r [<!ENTITY x "a&y;b&y;c"><!ENTITY y "<y>&z;-&z;</y>"><!ENTITY z "zz">]><r>&x;&x;&x;<p>&x;</p>&x;</r>"#,
        true,
    ),
    (
        "defaults",
        r#"<!DOCTYPE r [<!ATTLIST q d CDATA "v&amp;w"><!ENTITY q "<q/><q d='&#x31;'/>">]><r>&q;&q;&q;</r>"#,
        true,
    ),
    (
        "default namespace",
        r#"<!DOCTYPE r [<!ENTITY q "<q><s/></q>">]><r xmlns="urn:a">&q;<p xmlns="urn:b">&q;&q;</p>&q;<p xmlns="">&q;&q;</p>&q;</r>"#,
        true,
    ),
    (
        "attribute references",
        r#"<!DOCTYPE r [<!ENTITY z "zz"><!ENTITY q "<q a='&z;&#x20;&z;' b='&amp;'/>">]><r>&q;&q;&q;&q;&q;&q;&q;&q;</r>"#,
        true,
    ),
    (
        "adjacent text",
        r#"<!DOCTYPE r [<!ENTITY z "zz">]><r>x&z;y&z;&z;<![CDATA[c]]>&z;<p/>&z;</r>"#,
        true,
    ),
    (
        "prefixed",
        r#"<!DOCTYPE r [<!ENTITY q "<p:q/>">]><r xmlns:p="urn:a">&q;&q;<s xmlns:p="urn:b">&q;&q;</s></r>"#,
        false,
    ),
    (
        "declarations",
        r#"<!DOCTYPE r [<!ENTITY q "<q xmlns='urn:q'><s/></q>">]><r>&q;&q;&q;</r>"#,
        false,
    ),
    (
        "instructions and warnings",
        r#"<!DOCTYPE r [<!ENTITY q "<?pi x?><q xml:space='odd'/>">]><r>&q;&q;&q;</r>"#,
        false,
    ),
    (
        "comments and cdata",
        r#"<!DOCTYPE r [<!ENTITY q "<!--c--><![CDATA[<d>]]>t">]><r>&q;&q;&q;&q;</r>"#,
        true,
    ),
];

/// The smallest value of the ceiling `set` accepts `input` with.
fn threshold(input: &[u8], set: fn(usize) -> Limits) -> usize {
    (0..)
        .find(|&value| read(input, set(value)).1.is_none())
        .expect("accepted")
}

#[test]
fn replays_match_expansions_at_every_ceiling() {
    type Ceiling = (&'static str, fn(usize) -> Limits);
    let ceilings: [Ceiling; 7] = [
        ("max_events", |v| Limits::DEFAULT.max_events(v)),
        ("max_depth", |v| Limits::DEFAULT.max_depth(v)),
        ("max_owned_bytes", |v| Limits::DEFAULT.max_owned_bytes(v)),
        ("max_entity_references", |v| {
            Limits::DEFAULT.max_entity_references(v)
        }),
        ("max_entity_depth", |v| Limits::DEFAULT.max_entity_depth(v)),
        ("max_entity_expansion_bytes", |v| {
            Limits::DEFAULT.max_entity_expansion_bytes(v)
        }),
        ("max_entity_amplification", |v| {
            (Limits::DEFAULT.max_entity_expansion_bytes(16)).max_entity_amplification(v)
        }),
    ];
    for (name, document, eligible) in DOCUMENTS {
        let input = document.as_bytes();
        let (outcome, dom, owned, replays) = parse(input, Limits::DEFAULT, true);
        assert!(outcome.1.is_none(), "{name}");
        assert_eq!(replays > 0, eligible, "{name} replays");
        let expanded = parse(input, Limits::DEFAULT, false);
        assert_eq!(
            (outcome, dom, owned),
            (expanded.0, expanded.1, expanded.2),
            "{name}"
        );
        for (ceiling, set) in ceilings {
            let accepted = threshold(input, set);
            for value in accepted.saturating_sub(256)..=accepted + 2 {
                let (outcome, dom, owned, _) = parse(input, set(value), true);
                let expanded = parse(input, set(value), false);
                let actual = (outcome, dom, owned);
                assert_eq!(
                    actual,
                    (expanded.0, expanded.1, expanded.2),
                    "{name} {ceiling}={value}"
                );
            }
        }
    }
}
