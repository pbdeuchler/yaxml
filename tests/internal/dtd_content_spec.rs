use super::*;

use crate::dtd_frames::FrameLimit;
use crate::dtd_reference::ReferenceLimits;
use crate::origin::{ByteSpan, EntityDeclarationId, EntityInvocationId, OriginRepr};
use crate::{Attribute, Element, Limit, Limits, Name};
use std::borrow::Cow;
use std::num::NonZeroU64;

const DTD_WORK: usize = 64 * 1024 * 1024;
const TOKEN: usize = 4096;

fn text_limits(max_work: usize) -> TextLimits {
    TextLimits {
        max_work,
        reference: ReferenceLimits {
            max_bytes: TOKEN,
            max_scratch_bytes: TOKEN,
            max_work: DTD_WORK,
        },
        scratch: crate::tokenizer::text::TextScratchLimits {
            input_token_bytes: TOKEN,
            replacement_token_bytes: TOKEN,
            max_bytes: TOKEN * 3,
        },
    }
}

fn local_limits() -> LocalLimits {
    LocalLimits {
        max_token_bytes: TOKEN,
        max_scratch_bytes: TOKEN,
        max_work: DTD_WORK,
    }
}

fn declarations(
    raw: &str,
    controller_limits: ControllerLimits,
    parser_limits: Limits,
) -> (DtdBodyState, ParserState<'static>) {
    let mut parser = ParserState::new(parser_limits, 0);
    let mut dtd = DtdController::new(SourceEncoding::Utf8, 0, controller_limits, false);
    let mut cursor = 0;
    for _ in 0..256 {
        while dtd.take_current().is_some() {}
        if dtd.is_complete() {
            assert_eq!(cursor, raw.len());
            return (dtd.into_body_state().unwrap(), parser);
        }
        if cursor < raw.len() {
            let consumed = dtd
                .advance(&raw.as_bytes()[cursor..], parser.owned_budget())
                .unwrap();
            cursor += consumed;
            assert!(consumed != 0 || dtd.current().is_some());
        } else {
            dtd.finish_step(parser.owned_budget()).unwrap();
        }
    }
    panic!("declaration controller did not terminate")
}

fn borrowed_declarations(
    raw: &'static str,
    physical_start: usize,
    controller_limits: ControllerLimits,
    mut parser: ParserState<'static>,
) -> (DtdBodyState, ParserState<'static>) {
    let mut dtd = BorrowedDtdController::stable_utf8(
        raw,
        physical_start,
        physical_start,
        controller_limits,
        false,
    )
    .unwrap();
    for _ in 0..256 {
        while dtd.take_current().is_some() {}
        if dtd.is_complete() {
            return (dtd.into_body_state().unwrap(), parser);
        }
        dtd.advance_borrowed(parser.owned_budget()).unwrap();
    }
    panic!("borrowed declaration controller did not terminate")
}

fn make_with(
    dtd: &str,
    controller_limits: ControllerLimits,
    parser_limits: Limits,
    content_work: usize,
) -> ContentController {
    let (body, parser) = declarations(dtd, controller_limits, parser_limits);
    ContentController::new(
        body,
        parser,
        Tokenizer::new(TOKEN, TOKEN),
        text_limits(content_work),
        local_limits(),
    )
    .unwrap()
}

fn make(dtd: &str) -> ContentController {
    make_with(
        dtd,
        ControllerLimits::default(),
        Limits::default(),
        DTD_WORK,
    )
}

#[derive(Debug, Eq, PartialEq)]
enum SeenEvent {
    Start(SeenElement),
    Empty(SeenElement),
    End(SeenName),
    Text(String),
    Cdata(String),
    Comment(String),
    Pi(String, String),
    Declaration,
}

#[derive(Debug, Eq, PartialEq)]
struct SeenElement {
    name: SeenName,
    attributes: Vec<(SeenName, String)>,
}

#[derive(Debug, Eq, PartialEq)]
struct SeenName {
    qualified: String,
    prefix: Option<String>,
    local: String,
    namespace: Option<String>,
}

fn seen_name(name: &Name<'_>) -> SeenName {
    SeenName {
        qualified: name.qualified().to_owned(),
        prefix: name.prefix().map(str::to_owned),
        local: name.local().to_owned(),
        namespace: name.namespace().map(str::to_owned),
    }
}

fn seen_element(element: &Element<'_>) -> SeenElement {
    SeenElement {
        name: seen_name(&element.name),
        attributes: element
            .attributes
            .iter()
            .map(|Attribute { name, value }| (seen_name(name), value.to_string()))
            .collect(),
    }
}

fn seen_event(event: Event<'_>) -> SeenEvent {
    match event {
        Event::Start(element) => SeenEvent::Start(seen_element(&element)),
        Event::Empty(element) => SeenEvent::Empty(seen_element(&element)),
        Event::End(name) => SeenEvent::End(seen_name(&name)),
        Event::Text(value) => SeenEvent::Text(value.into_owned()),
        Event::Cdata(value) => SeenEvent::Cdata(value.into_owned()),
        Event::Comment(value) => SeenEvent::Comment(value.into_owned()),
        Event::ProcessingInstruction { target, data } => {
            SeenEvent::Pi(target.into_owned(), data.into_owned())
        }
        Event::Declaration(_) | Event::Doctype(_) => SeenEvent::Declaration,
    }
}

#[derive(Debug, Eq, PartialEq)]
struct SeenOrigin {
    input: Option<std::ops::Range<usize>>,
    replacement: Option<std::ops::Range<usize>>,
    physical: std::ops::Range<usize>,
}

fn seen_origin(origin: SourceOrigin) -> SeenOrigin {
    SeenOrigin {
        input: origin.input_range(),
        replacement: origin.replacement_range(),
        physical: origin.physical_context(),
    }
}

#[derive(Debug)]
struct Seen {
    event: SeenEvent,
    origin: SeenOrigin,
    source_origin: SourceOrigin,
    active_invocation: Option<u64>,
}

fn emit(controller: &mut ContentController) -> Result<Seen, ContentError> {
    let mut captured = None;
    controller.emit(&mut |event, origin, _| {
        captured = Some((seen_event(event), seen_origin(origin), origin));
        Ok(())
    })?;
    let (event, origin, source_origin) = captured.expect("semantic event callback");
    Ok(Seen {
        event,
        origin,
        source_origin,
        active_invocation: controller
            .active
            .last()
            .map(|active| active.invocation.get()),
    })
}

fn run(
    mut controller: ContentController,
    body: &str,
) -> Result<(Vec<Seen>, ContentController), ContentError> {
    let mut cursor = 0;
    let mut events = Vec::new();
    for _ in 0..1024 {
        let step = if cursor < body.len() {
            controller.advance(&body.as_bytes()[cursor..])?
        } else {
            controller.finish_step()?
        };
        cursor += step.consumed;
        match step.outcome {
            ContentOutcome::EventReady => events.push(emit(&mut controller)?),
            ContentOutcome::NeedPhysicalInput => {
                assert!(cursor < body.len(), "asked for input at EOF");
            }
            ContentOutcome::Progress => {}
            ContentOutcome::Complete => {
                assert_eq!(cursor, body.len());
                return Ok((events, controller));
            }
        }
    }
    panic!("content controller did not terminate")
}

fn failed(result: Result<(Vec<Seen>, ContentController), ContentError>) -> ContentError {
    match result {
        Err(error) => error,
        Ok(_) => panic!("expected content failure"),
    }
}

fn plain(qualified: &str) -> SeenName {
    SeenName {
        qualified: qualified.to_owned(),
        prefix: None,
        local: qualified.to_owned(),
        namespace: None,
    }
}

fn replacement_origin(
    declaration: usize,
    invocation: u64,
    local: std::ops::Range<usize>,
    physical_reference: std::ops::Range<usize>,
) -> SourceOrigin {
    SourceOrigin::from_repr(OriginRepr::Replacement {
        declaration: EntityDeclarationId::from_index(declaration),
        invocation: EntityInvocationId(NonZeroU64::new(invocation).unwrap()),
        local: ByteSpan::new(local).unwrap(),
        physical_reference: ByteSpan::new(physical_reference).unwrap(),
    })
}

#[test]
fn physical_prefix_general_replacement_and_suffix_have_literal_origins() {
    let dtd = "<!DOCTYPE r [<!ENTITY e 'x'>]>";
    let controller = make(dtd);
    let inherited_work = controller.usage().work;
    let (seen, controller) = run(controller, "<r>a&e;b</r>").unwrap();
    let base = dtd.len();
    assert_eq!(base, 30);
    assert_eq!(seen.len(), 5);
    assert_eq!(
        seen[0].event,
        SeenEvent::Start(SeenElement {
            name: plain("r"),
            attributes: vec![]
        })
    );
    assert_eq!(
        seen[0].origin,
        SeenOrigin {
            input: Some(30..33),
            replacement: None,
            physical: 30..33
        }
    );
    assert_eq!(seen[1].event, SeenEvent::Text("a".into()));
    assert_eq!(
        seen[1].origin,
        SeenOrigin {
            input: Some(33..34),
            replacement: None,
            physical: 33..34
        }
    );
    assert_eq!(seen[2].event, SeenEvent::Text("x".into()));
    assert_eq!(
        seen[2].origin,
        SeenOrigin {
            input: None,
            replacement: Some(0..1),
            physical: 34..37
        }
    );
    assert_eq!(seen[2].active_invocation, Some(1));
    assert_eq!(seen[3].event, SeenEvent::Text("b".into()));
    assert_eq!(
        seen[3].origin,
        SeenOrigin {
            input: Some(37..38),
            replacement: None,
            physical: 37..38
        }
    );
    assert_eq!(seen[4].event, SeenEvent::End(plain("r")));
    assert_eq!(
        seen[4].origin,
        SeenOrigin {
            input: Some(38..42),
            replacement: None,
            physical: 38..42
        }
    );
    assert_eq!(controller.frame_counters().reference_attempts, 1);
    assert_eq!(controller.frame_counters().work, 2);
    assert_eq!(controller.frame_counters().expanded_bytes, 1);
    let usage = controller.usage();
    assert_eq!(usage.source_scanned, 10);
    assert_eq!(usage.reference_scanned, 3);
    assert_eq!(usage.reference_copied, 3);
    assert_eq!(usage.reference_classified, 3);
    assert_eq!(usage.appended_reference_copied, 0);
    assert_eq!(usage.validated, 10);
    assert_eq!(
        controller.usage().work - inherited_work,
        41,
        "10 non-reference source bytes + 10 validation bytes + 9 reference scan/copy/classification + 10 complete-token bytes + 2 frame work",
    );
}

#[test]
fn terminal_prefix_callback_error_precedes_lookup_and_suffix() {
    let dtd = "<!DOCTYPE r [<!ENTITY e 'x'>]>";
    let mut controller = make(dtd);
    let body = b"<r>a&e;b</r>";
    let first = controller.advance(body).unwrap();
    assert_eq!(first.outcome, ContentOutcome::EventReady);
    emit(&mut controller).unwrap();
    let second = controller.advance(&body[first.consumed..]).unwrap();
    assert_eq!(second.outcome, ContentOutcome::EventReady);
    let before = controller.frame_counters();
    let expected = Error::new(ErrorKind::InvalidState, 777);
    let error = controller
        .emit(&mut |event, origin, _| {
            assert_eq!(seen_event(event), SeenEvent::Text("a".into()));
            assert_eq!(seen_origin(origin).input, Some(33..34));
            Err(expected)
        })
        .unwrap_err();
    assert_eq!(
        error.kind,
        ContentErrorKind::Callback(ErrorKind::InvalidState)
    );
    assert_eq!(error.physical_offset, expected.offset);
    assert_eq!(error.local_offset, None);
    assert_eq!(controller.frame_counters(), before);
    assert_eq!(before.reference_attempts, 0);
    assert_eq!(controller.position(), (37, 37));
    assert_eq!(controller.advance(b"b</r>"), Err(error));
    assert_eq!(controller.emit(&mut |_, _, _| Ok(())), Err(error));
}

#[test]
fn nested_and_repeated_entities_keep_distinct_invocations_and_exact_frame_costs() {
    let dtd = "<!DOCTYPE r [<!ENTITY b 'B'><!ENTITY a '<n>&b;</n>'>]>";
    let (seen, controller) = run(make(dtd), "<r>&a;&a;</r>").unwrap();
    let generated: Vec<_> = seen
        .iter()
        .filter_map(|item| {
            item.active_invocation
                .map(|id| (id, &item.event, &item.origin, item.source_origin))
        })
        .collect();
    assert_eq!(generated.len(), 6);
    assert_eq!(
        generated.iter().map(|item| item.0).collect::<Vec<_>>(),
        [1, 2, 1, 3, 4, 3]
    );
    assert_eq!(
        generated[0].1,
        &SeenEvent::Start(SeenElement {
            name: plain("n"),
            attributes: vec![]
        })
    );
    assert_eq!(generated[0].2.replacement, Some(0..3));
    assert_eq!(generated[0].2.physical, 57..60);
    assert_eq!(generated[1].1, &SeenEvent::Text("B".into()));
    assert_eq!(generated[1].2.replacement, Some(0..1));
    assert_eq!(generated[2].1, &SeenEvent::End(plain("n")));
    assert_eq!(generated[2].2.replacement, Some(6..10));
    assert_eq!(generated[3].2.physical, 60..63);
    assert_eq!(
        generated.iter().map(|item| item.3).collect::<Vec<_>>(),
        [
            replacement_origin(1, 1, 0..3, 57..60),
            replacement_origin(0, 2, 0..1, 57..60),
            replacement_origin(1, 1, 6..10, 57..60),
            replacement_origin(1, 3, 0..3, 60..63),
            replacement_origin(0, 4, 0..1, 60..63),
            replacement_origin(1, 3, 6..10, 60..63),
        ]
    );
    assert_eq!(controller.frame_counters().reference_attempts, 4);
    assert_eq!(controller.frame_counters().work, 28);
    assert_eq!(controller.frame_counters().expanded_bytes, 22);
}

#[test]
fn empty_entity_is_finite_progress_without_empty_text() {
    let dtd = "<!DOCTYPE r [<!ENTITY e ''>]>";
    let mut controller = make(dtd);
    let mut cursor = 0;
    let body = b"<r>a&e;b</r>";
    let mut progress = 0;
    let mut values = Vec::new();
    for _ in 0..64 {
        let step = if cursor < body.len() {
            controller.advance(&body[cursor..]).unwrap()
        } else {
            controller.finish_step().unwrap()
        };
        cursor += step.consumed;
        match step.outcome {
            ContentOutcome::EventReady => values.push(emit(&mut controller).unwrap().event),
            ContentOutcome::Progress => progress += 1,
            ContentOutcome::NeedPhysicalInput => assert!(cursor < body.len()),
            ContentOutcome::Complete => break,
        }
    }
    assert_eq!(
        values,
        [
            SeenEvent::Start(SeenElement {
                name: plain("r"),
                attributes: vec![]
            }),
            SeenEvent::Text("a".into()),
            SeenEvent::Text("b".into()),
            SeenEvent::End(plain("r")),
        ]
    );
    assert_eq!(progress, 2, "entry and pop are distinct finite transitions");
    assert_eq!(controller.frame_counters().reference_attempts, 1);
    assert_eq!(controller.frame_counters().work, 1);
    assert_eq!(controller.frame_counters().expanded_bytes, 0);
}

#[test]
fn entity_value_markup_and_content_reference_outputs_obey_two_phase_rules() {
    let dtd = "<!DOCTYPE r [<!ENTITY e '&#60;n/>K'>]>";
    let (seen, _) = run(make(dtd), "<r>&e;</r>").unwrap();
    assert!(seen.iter().any(|item| item.event
        == SeenEvent::Empty(SeenElement {
            name: plain("n"),
            attributes: vec![]
        })));
    assert!(
        seen.iter()
            .any(|item| item.event == SeenEvent::Text("K".into()))
    );

    let (direct, _) = run(make("<!DOCTYPE r>"), "<r>&#60;&lt;&amp;</r>").unwrap();
    assert!(
        direct
            .iter()
            .any(|item| item.event == SeenEvent::Text("<<&".into()))
    );

    let (no_rescan, _) = run(make("<!DOCTYPE r [<!ENTITY e 'AT&amp;T;'>]>"), "<r>&e;</r>").unwrap();
    assert!(
        no_rescan
            .iter()
            .any(|item| item.event == SeenEvent::Text("AT&T;".into()))
    );
}

#[test]
fn prior_parameter_entity_and_body_general_entity_share_nonce_and_counters() {
    let dtd = "<!DOCTYPE r [<!ENTITY % p \"<!ENTITY g 'G'>\">%p;]>";
    let (body, parser) = declarations(dtd, ControllerLimits::default(), Limits::default());
    let inherited = body.frames.counters();
    assert_eq!(inherited.reference_attempts, 1);
    let controller = ContentController::new(
        body,
        parser,
        Tokenizer::new(TOKEN, TOKEN),
        text_limits(DTD_WORK),
        local_limits(),
    )
    .unwrap();
    let (seen, controller) = run(controller, "<r>&g;</r>").unwrap();
    let generated = seen
        .iter()
        .find(|item| item.event == SeenEvent::Text("G".into()))
        .unwrap();
    assert_eq!(generated.active_invocation, Some(2));
    let after = controller.frame_counters();
    assert_eq!(after.reference_attempts, inherited.reference_attempts + 1);
    assert_eq!(after.work, inherited.work + 2);
    assert_eq!(after.expanded_bytes, inherited.expanded_bytes + 1);
}

#[test]
fn generated_defaults_and_namespaces_use_the_handoff_catalog_once() {
    let dtd =
        "<!DOCTYPE r [<!ATTLIST n xmlns CDATA 'u' a NMTOKENS '  x   y  '><!ENTITY e '<n/>'>]>";
    let (seen, _) = run(make(dtd), "<r>&e;</r>").unwrap();
    let element = seen
        .iter()
        .find_map(|item| match &item.event {
            SeenEvent::Empty(element) if element.name.qualified == "n" => Some(element),
            _ => None,
        })
        .unwrap();
    assert_eq!(element.name.namespace.as_deref(), Some("u"));
    assert_eq!(element.attributes.len(), 2);
    assert_eq!(element.attributes[0].0.qualified, "xmlns");
    assert_eq!(element.attributes[0].1, "u");
    assert_eq!(element.attributes[1].0, plain("a"));
    assert_eq!(element.attributes[1].1, "x y");

    let explicit = "<!DOCTYPE r [<!ATTLIST n xmlns CDATA 'u' a NMTOKENS 'default'><!ENTITY e \"<n a=' z '/>\">]>";
    let (seen, _) = run(make(explicit), "<r>&e;</r>").unwrap();
    let element = seen
        .iter()
        .find_map(|item| match &item.event {
            SeenEvent::Empty(element) if element.name.qualified == "n" => Some(element),
            _ => None,
        })
        .unwrap();
    assert_eq!(element.attributes.len(), 2);
    assert_eq!(element.attributes[0].0, plain("a"));
    assert_eq!(element.attributes[0].1, "z");
    assert_eq!(element.attributes[1].0.qualified, "xmlns");
}

#[test]
fn generated_callback_moves_owned_names_values_and_inherited_namespace() {
    let (owned_name, owned_attributes, owned_text) = {
        let dtd = String::from("<!DOCTYPE r [<!ATTLIST p:n a CDATA 'v'><!ENTITY e '<p:n/>x'>]>");
        let body = String::from("<r xmlns:p='urn:u'>&e;</r>");
        let mut controller = make(&dtd);
        let mut cursor = 0;
        let mut owned_name = None;
        let mut owned_attributes = None;
        let mut owned_text = None;
        for _ in 0..128 {
            let step = if cursor < body.len() {
                controller.advance(&body.as_bytes()[cursor..]).unwrap()
            } else {
                controller.finish_step().unwrap()
            };
            cursor += step.consumed;
            match step.outcome {
                ContentOutcome::EventReady => controller
                    .emit(&mut |event, origin, _| {
                        if origin.replacement_range().is_some() {
                            match event {
                                Event::Empty(element) => {
                                    owned_name = Some(element.name.try_into_static(0).unwrap());
                                    owned_attributes = Some(
                                        element
                                            .attributes
                                            .into_iter()
                                            .map(|Attribute { name, value }| {
                                                let name = name.try_into_static(0).unwrap();
                                                let Cow::Owned(value) = value else {
                                                    panic!("generated default must be owned")
                                                };
                                                (name, value)
                                            })
                                            .collect::<Vec<_>>(),
                                    );
                                }
                                Event::Text(value) => {
                                    let Cow::Owned(value) = value else {
                                        panic!("generated text must be owned")
                                    };
                                    owned_text = Some(value);
                                }
                                _ => {}
                            }
                        }
                        Ok(())
                    })
                    .unwrap(),
                ContentOutcome::NeedPhysicalInput => assert!(cursor < body.len()),
                ContentOutcome::Progress => {}
                ContentOutcome::Complete => break,
            }
        }
        (
            owned_name.expect("generated Empty name"),
            owned_attributes.expect("generated default attribute"),
            owned_text.expect("generated Text"),
        )
    };
    assert_eq!(owned_name.qualified(), "p:n");
    assert_eq!(owned_name.prefix(), Some("p"));
    assert_eq!(owned_name.local(), "n");
    assert_eq!(owned_name.namespace(), Some("urn:u"));
    assert_eq!(owned_attributes.len(), 1);
    assert_eq!(owned_attributes[0].0.qualified(), "a");
    assert_eq!(owned_attributes[0].0.namespace(), None);
    assert_eq!(owned_attributes[0].1, "v");
    assert_eq!(owned_text, "x");
}

#[test]
fn replacement_leaf_phases_preserve_cr_and_do_not_expand_non_text_references() {
    let dtd = concat!(
        "<!DOCTYPE r [",
        "<!ENTITY t 'a&#13;b'>",
        "<!ENTITY c '<![CDATA[&missing;&#13;]]>'>",
        "<!ENTITY p '<?pi &missing;&#13;?>'>",
        "<!ENTITY m '<!--&missing;&#13;-->'>",
        "]>"
    );
    let (seen, controller) = run(make(dtd), "<r>&t;&c;&p;&m;</r>").unwrap();
    assert!(
        seen.iter()
            .any(|item| item.event == SeenEvent::Text("a\rb".into()))
    );
    assert!(
        seen.iter()
            .any(|item| item.event == SeenEvent::Cdata("&missing;\r".into()))
    );
    assert!(
        seen.iter()
            .any(|item| item.event == SeenEvent::Pi("pi".into(), "&missing;\r".into()))
    );
    assert!(
        seen.iter()
            .any(|item| item.event == SeenEvent::Comment("&missing;\r".into()))
    );
    assert_eq!(controller.frame_counters().reference_attempts, 4);

    let error = failed(run(
        make("<!DOCTYPE r [<!ENTITY e \"<?xml version='1.0'?>\">]>"),
        "<r>&e;</r>",
    ));
    assert_eq!(
        error.kind,
        ContentErrorKind::Parser(ErrorKind::InvalidDeclaration)
    );
    assert_eq!(error.local_offset, Some(0));
}

fn run_chunks(
    mut controller: ContentController,
    body: &str,
    chunk: usize,
) -> Result<Vec<Seen>, ContentError> {
    let mut supplied = 0;
    let mut available = 0;
    let mut seen = Vec::new();
    for _ in 0..2048 {
        if available < body.len() && supplied == available {
            available = (available + chunk).min(body.len());
        }
        let step = if supplied < available {
            controller.advance(&body.as_bytes()[supplied..available])?
        } else if available == body.len() {
            controller.finish_step()?
        } else {
            continue;
        };
        supplied += step.consumed;
        match step.outcome {
            ContentOutcome::EventReady => seen.push(emit(&mut controller)?),
            ContentOutcome::NeedPhysicalInput => {
                assert_eq!(supplied, available);
                assert!(available < body.len());
            }
            ContentOutcome::Progress => {}
            ContentOutcome::Complete => {
                assert_eq!(supplied, body.len());
                return Ok(seen);
            }
        }
    }
    panic!("chunked drive did not terminate")
}

#[test]
fn every_one_byte_physical_split_preserves_complete_replacement_units() {
    let cases = [
        ("x", SeenEvent::Text("x".into())),
        (
            "<a/>",
            SeenEvent::Empty(SeenElement {
                name: plain("a"),
                attributes: vec![],
            }),
        ),
        ("<!--x-->", SeenEvent::Comment("x".into())),
        ("<![CDATA[x]]>", SeenEvent::Cdata("x".into())),
        ("<?p x?>", SeenEvent::Pi("p".into(), "x".into())),
    ];
    for (replacement, expected) in cases {
        let dtd = format!("<!DOCTYPE r [<!ENTITY e '{replacement}'>]>");
        let seen = run_chunks(make(&dtd), "<r>&e;</r>", 1).unwrap();
        assert!(
            seen.iter().any(|item| item.event == expected),
            "{replacement:?}"
        );
    }
}

#[test]
fn incomplete_replacement_units_fail_before_resuming_physical_input() {
    for (replacement, expected) in [
        (
            "<a",
            TextErrorKind::Local(crate::tokenizer::local::LocalErrorKind::Xml(
                ErrorKind::UnexpectedEof,
            )),
        ),
        (
            "</a",
            TextErrorKind::Local(crate::tokenizer::local::LocalErrorKind::Xml(
                ErrorKind::UnexpectedEof,
            )),
        ),
        (
            "<!--x",
            TextErrorKind::Local(crate::tokenizer::local::LocalErrorKind::Xml(
                ErrorKind::UnexpectedEof,
            )),
        ),
        (
            "<![CDATA[x",
            TextErrorKind::Local(crate::tokenizer::local::LocalErrorKind::Xml(
                ErrorKind::UnexpectedEof,
            )),
        ),
        (
            "<?p x",
            TextErrorKind::Local(crate::tokenizer::local::LocalErrorKind::Xml(
                ErrorKind::UnexpectedEof,
            )),
        ),
        (
            "&#38;name",
            TextErrorKind::Reference(crate::dtd_reference::ReferenceErrorKind::UnexpectedEof),
        ),
        (
            "&#38;#60",
            TextErrorKind::Reference(crate::dtd_reference::ReferenceErrorKind::UnexpectedEof),
        ),
    ] {
        let dtd = format!("<!DOCTYPE r [<!ENTITY e '{replacement}'>]>");
        let error = failed(run(make(&dtd), "<r>&e;post</r>"));
        assert_eq!(
            error.kind,
            ContentErrorKind::Text(expected),
            "{replacement:?}"
        );
        assert_eq!(error.physical_offset, dtd.len() + 3, "{replacement:?}");
        assert!(error.local_offset.is_some());
    }
}

#[test]
fn replacement_cannot_close_and_reopen_its_callers_element() {
    let dtd = "<!DOCTYPE r [<!ENTITY e '</r><r>'>]>";
    let mut controller = make(dtd);
    let error = run_error(&mut controller, "<r>&e;</r>");
    assert_eq!(
        error.kind,
        ContentErrorKind::Frame(FrameErrorKind::ElementBoundaryCrossing)
    );
    assert_eq!(error.physical_offset, dtd.len() + 3);
    assert_eq!(error.local_offset, Some(0));
    assert_eq!(error.origin.unwrap().replacement_range(), Some(0..4));
    assert_eq!(controller.parser.element_depth(), 1);
    assert_eq!(controller.advance(b""), Err(error));
}

#[test]
fn missing_external_unparsed_and_unused_bad_entities_have_literal_outcomes() {
    let cases = [
        (
            "<!DOCTYPE r>",
            "missing",
            FrameErrorKind::Lookup(LookupFailure::Undeclared),
        ),
        (
            "<!DOCTYPE r [<!NOTATION n SYSTEM 'n'><!ENTITY e SYSTEM 'x' NDATA n>]>",
            "e",
            FrameErrorKind::Lookup(LookupFailure::Unparsed),
        ),
    ];
    for (dtd, name, expected) in cases {
        let body = format!("<r>&{name};post</r>");
        let error = failed(run(make(dtd), &body));
        assert_eq!(error.kind, ContentErrorKind::Frame(expected), "{name}");
        assert_eq!(error.physical_offset, dtd.len() + 3);
    }
    // An external entity is never loaded; like libxml2's failed load, its
    // reference contributes no content.
    let (seen, _) = run(
        make("<!DOCTYPE r [<!ENTITY e SYSTEM 'x'>]>"),
        "<r>&e;post</r>",
    )
    .unwrap();
    assert_eq!(seen.len(), 3);
    assert!(matches!(&seen[1].event, SeenEvent::Text(text) if text == "post"));

    let unused = concat!(
        "<!DOCTYPE r [",
        "<!ENTITY missing '&notRead;'>",
        "<!ENTITY cycle '&cycle;'>",
        "<!ENTITY markup '<a'>",
        "<!NOTATION n SYSTEM 'n'><!ENTITY img SYSTEM 'x' NDATA n>",
        "]>"
    );
    let (seen, _) = run(make(unused), "<r/>").unwrap();
    assert_eq!(seen.len(), 1);
    assert!(matches!(seen[0].event, SeenEvent::Empty(_)));
}

#[test]
fn direct_and_indirect_cycles_fail_only_on_use_with_nested_local_origin() {
    for (dtd, expected_local) in [
        ("<!DOCTYPE r [<!ENTITY a '&a;'>]>", 0..3),
        ("<!DOCTYPE r [<!ENTITY a '&b;'><!ENTITY b '&a;'>]>", 0..3),
    ] {
        let error = failed(run(make(dtd), "<r>&a;post</r>"));
        assert_eq!(error.kind, ContentErrorKind::Frame(FrameErrorKind::Cycle));
        assert_eq!(error.physical_offset, dtd.len() + 3);
        assert_eq!(
            error.origin.unwrap().replacement_range(),
            Some(expected_local)
        );
    }
}

#[test]
fn frame_attempt_work_expansion_and_depth_limits_are_exact_and_sticky() {
    let dtd = "<!DOCTYPE r [<!ENTITY e 'x'>]>";
    for (configure, expected) in [
        (0, FrameLimit::ReferenceAttempts),
        (1, FrameLimit::Work),
        (2, FrameLimit::ExpandedBytes),
    ] {
        let mut limits = ControllerLimits::default();
        match configure {
            0 => limits.frames.max_reference_attempts = 0,
            1 => limits.frames.max_work = 1,
            _ => limits.frames.max_expanded_bytes = 0,
        }
        let mut controller = make_with(dtd, limits, Limits::default(), DTD_WORK);
        let error = run_error(&mut controller, "<r>&e;</r>");
        assert_eq!(
            error.kind,
            ContentErrorKind::Frame(FrameErrorKind::LimitExceeded(expected))
        );
        let counters = controller.frame_counters();
        assert_eq!(controller.advance(b""), Err(error));
        assert_eq!(controller.frame_counters(), counters);
    }

    let nested = "<!DOCTYPE r [<!ENTITY a '&b;'><!ENTITY b 'B'>]>";
    let mut limits = ControllerLimits::default();
    limits.frames.max_active_depth = 1;
    let error = failed(run(
        make_with(nested, limits, Limits::default(), DTD_WORK),
        "<r>&a;</r>",
    ));
    assert_eq!(
        error.kind,
        ContentErrorKind::Frame(FrameErrorKind::LimitExceeded(FrameLimit::ActiveDepth))
    );
}

fn run_error(controller: &mut ContentController, body: &str) -> ContentError {
    let mut cursor = 0;
    for _ in 0..256 {
        let result = if cursor < body.len() {
            controller.advance(&body.as_bytes()[cursor..])
        } else {
            controller.finish_step()
        };
        match result {
            Err(error) => return error,
            Ok(step) => {
                cursor += step.consumed;
                if step.outcome == ContentOutcome::EventReady
                    && let Err(error) = emit(controller)
                {
                    return error;
                }
            }
        }
    }
    panic!("expected content failure")
}

#[test]
fn global_work_exact_boundary_is_inherited_and_checked_before_publication() {
    let (body, parser) = declarations(
        "<!DOCTYPE r>",
        ControllerLimits::default(),
        Limits::default(),
    );
    let inherited = body.counters.work;
    let exact = ContentController::new(
        body,
        parser,
        Tokenizer::new(TOKEN, TOKEN),
        text_limits(inherited + 12),
        local_limits(),
    )
    .unwrap();
    let (seen, controller) = run(exact, "<r/>").unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(controller.usage().work, inherited + 12);

    let (body, parser) = declarations(
        "<!DOCTYPE r>",
        ControllerLimits::default(),
        Limits::default(),
    );
    let one_short = ContentController::new(
        body,
        parser,
        Tokenizer::new(TOKEN, TOKEN),
        text_limits(inherited + 11),
        local_limits(),
    )
    .unwrap();
    let error = failed(run(one_short, "<r/>"));
    assert_eq!(error.kind, ContentErrorKind::Work);
}

#[test]
fn generated_text_work_boundary_charges_the_complete_token_before_callback() {
    const DTD: &str = "<!DOCTYPE r [<!ENTITY e 'x'>]>";
    let (body, parser) = declarations(DTD, ControllerLimits::default(), Limits::default());
    let inherited = body.counters.work;
    let mut exact = ContentController::new(
        body,
        parser,
        Tokenizer::new(TOKEN, TOKEN),
        text_limits(inherited + 23),
        local_limits(),
    )
    .unwrap();
    let body_bytes = b"<r>&e;</r>";
    let mut cursor = 0;
    let mut callbacks = 0;
    let mut steps = 0;
    loop {
        steps += 1;
        assert!(steps <= 64, "exact work boundary did not make progress");
        let step = exact.advance(&body_bytes[cursor..]).unwrap();
        cursor += step.consumed;
        if step.outcome == ContentOutcome::EventReady {
            let mut generated_text = false;
            exact
                .emit(&mut |event, origin, _| {
                    callbacks += 1;
                    generated_text = matches!(event, Event::Text(ref value) if value == "x")
                        && origin.replacement_range() == Some(0..1);
                    Ok(())
                })
                .unwrap();
            if generated_text {
                break;
            }
        }
    }
    assert_eq!(exact.usage().work, inherited + 23);
    assert_eq!(callbacks, 2, "physical Start then generated Text");

    let (body, parser) = declarations(DTD, ControllerLimits::default(), Limits::default());
    let mut short = ContentController::new(
        body,
        parser,
        Tokenizer::new(TOKEN, TOKEN),
        text_limits(inherited + 22),
        local_limits(),
    )
    .unwrap();
    let mut cursor = 0;
    let body = body_bytes;
    let mut callbacks = 0;
    let mut steps = 0;
    let error = loop {
        steps += 1;
        assert!(steps <= 64, "short work boundary did not terminate");
        let result = if cursor < body.len() {
            short.advance(&body[cursor..])
        } else {
            short.finish_step()
        };
        match result {
            Err(error) => break error,
            Ok(step) => {
                cursor += step.consumed;
                if step.outcome == ContentOutcome::EventReady {
                    match short.emit(&mut |_, _, _| {
                        callbacks += 1;
                        Ok(())
                    }) {
                        Ok(()) => {}
                        Err(error) => break error,
                    }
                }
            }
        }
    };
    assert_eq!(error.kind, ContentErrorKind::Work);
    assert_eq!(callbacks, 1, "only physical Start is published");
    assert_eq!(short.parser.element_depth(), 1);
    assert_eq!(short.usage().work, inherited + 22);
    assert_eq!(short.advance(b""), Err(error));
}

#[test]
fn generated_owned_budget_one_short_is_atomic_and_sticky() {
    let dtd = "<!DOCTYPE r [<!ENTITY e \"<p:n xmlns:p='u'/>\">]>";
    // Streaming DTD units retain every byte except the two-byte `]>`. The
    // generated tag then requests 3 bytes for `p:n`, 7 for `xmlns:p`, the
    // catalog's documented first 64-byte normalized-value block. Namespace
    // publication then retains the URI key, two prefix views, binding URI, and
    // event namespace URI (five one-byte copies). The preceding physical `<r>`
    // also retains its one-byte stack name in this scratch-lending slice.
    let exact_limits = Limits {
        max_owned_bytes: dtd.len() - 2 + 80,
        ..Limits::default()
    };
    let exact = run(
        make_with(dtd, ControllerLimits::default(), exact_limits, DTD_WORK),
        "<r>&e;</r>",
    );
    if let Err(error) = exact {
        panic!("literal generated ownership total must fit: {error:?}");
    }

    let short_limits = Limits {
        max_owned_bytes: dtd.len() - 2 + 79,
        ..Limits::default()
    };
    let mut short = make_with(dtd, ControllerLimits::default(), short_limits, DTD_WORK);
    let error = run_error(&mut short, "<r>&e;</r>");
    assert_eq!(
        error.kind,
        ContentErrorKind::Tag(DtdTagErrorKind::Parser(ErrorKind::LimitExceeded(
            Limit::OwnedBytes
        )))
    );
    assert_eq!(
        short.parser.element_depth(),
        1,
        "generated Empty must not partially publish"
    );
    assert_eq!(short.advance(b""), Err(error));
}

fn controller_after_real_parser_preamble(limits: Limits) -> ContentController {
    const DTD: &str = "<!DOCTYPE r [<!ENTITY e 'x'>]>";
    let mut parser = ParserState::new(limits, 0);
    let preamble = parser
        .process_token(
            TokenKind::Instruction,
            "<?p x?>",
            0..7,
            |value, offset, budget| budget.copy(value, offset),
        )
        .unwrap()
        .unwrap();
    assert!(matches!(preamble, Event::ProcessingInstruction { .. }));
    parser.owned_budget().charge_owned(5, 7).unwrap();
    let (body, parser) = borrowed_declarations(DTD, 7, ControllerLimits::default(), parser);
    ContentController::new(
        body,
        parser,
        Tokenizer::new(TOKEN, TOKEN),
        text_limits(DTD_WORK),
        local_limits(),
    )
    .unwrap()
}

#[test]
fn handoff_keeps_the_same_parser_event_and_owned_budget_state() {
    let exact = Limits {
        max_events: 4,      // PI, Start r, generated Text x, End r.
        max_owned_bytes: 7, // Five prior + physical stack `r` + generated `x`.
        ..Limits::default()
    };
    let (seen, _) = run(controller_after_real_parser_preamble(exact), "<r>&e;</r>").unwrap();
    assert_eq!(seen.len(), 3);
    assert!(
        seen.iter()
            .any(|item| item.event == SeenEvent::Text("x".into()))
    );

    let mut event_short = exact;
    event_short.max_events = 3;
    let error = failed(run(
        controller_after_real_parser_preamble(event_short),
        "<r>&e;</r>",
    ));
    assert_eq!(
        error.kind,
        ContentErrorKind::Parser(ErrorKind::LimitExceeded(Limit::Events))
    );

    let mut owned_short = exact;
    owned_short.max_owned_bytes = 6;
    let mut controller = controller_after_real_parser_preamble(owned_short);
    let error = run_error(&mut controller, "<r>&e;</r>");
    assert_eq!(
        error.kind,
        ContentErrorKind::Parser(ErrorKind::LimitExceeded(Limit::OwnedBytes))
    );
    assert_eq!(controller.parser.element_depth(), 1);
}

#[test]
fn generated_only_steps_do_not_consume_the_physical_suffix() {
    let dtd = "<!DOCTYPE r [<!ENTITY e '<n/>K'>]>";
    let mut controller = make(dtd);
    let body = b"<r>&e;post</r>";
    let mut cursor = 0;
    let mut saw_generated = false;
    for _ in 0..64 {
        let step = controller.advance(&body[cursor..]).unwrap();
        cursor += step.consumed;
        if step.outcome == ContentOutcome::EventReady {
            let event = emit(&mut controller).unwrap();
            if event.origin.replacement.is_some() {
                saw_generated = true;
                assert_eq!(step.consumed, 0);
                assert_eq!(cursor, 6, "body cursor stops after &e;");
            }
        }
        if saw_generated && controller.active.is_empty() {
            break;
        }
    }
    assert!(saw_generated);
    assert_eq!(&body[cursor..], b"post</r>");
}
