//! `TextReader` against the slice `Reader`. A testcase is one control byte
//! followed by an XML document. The node sequence must equal the projection
//! of the reader's events under libxml2's node rules, and acceptance must
//! agree. Bit 0 trickles the input one byte per `fill_buf`; per node, the
//! rotated control byte selects content, expansion and navigation checks.
//! A second pass applies `next` at every node. The top three control bits
//! select a mode: 4 sets a base URI, 5 runs under small limits, 6 fails the
//! source with an I/O error midway and 7 serves external resources; modes
//! 5-7 check API consistency and termination rather than the reference.

use std::io::{self, BufRead, Read};

use yaxml::{
    ErrorKind, Event, Limits, NodeType, OwnedDocument, ParseOptions, ParserProperty, ReadError,
    ReadState, Reader, Request, TextReader, Writer, options, relaxng, xsd,
};

const XML: &str = "http://www.w3.org/XML/1998/namespace";

#[derive(Debug, PartialEq)]
struct Node {
    depth: usize,
    kind: NodeType,
    name: String,
    value: Option<String>,
    empty: bool,
    attributes: Vec<(String, String)>,
}

fn node(depth: usize, kind: NodeType, name: &str, value: Option<&str>) -> Node {
    Node {
        depth,
        kind,
        name: name.to_owned(),
        value: value.map(str::to_owned),
        empty: false,
        attributes: Vec::new(),
    }
}

/// The expected nodes, or None when the slice reader rejects the input.
fn reference(input: &[u8]) -> Option<Vec<Node>> {
    let mut reader = Reader::new(input).ok()?;
    let (mut nodes, mut depth, mut hidden) = (Vec::<Node>::new(), 0, 0_usize);
    while let Some(event) = reader.read_event().ok()? {
        // The reader does not visit entity-reference content.
        if hidden > 0 {
            match event {
                Event::EntityStart(_) => hidden += 1,
                Event::EntityEnd => hidden -= 1,
                _ => {}
            }
            continue;
        }
        let kind = match &event {
            Event::Declaration(_) => continue,
            Event::Doctype(doctype) => {
                nodes.push(node(depth, NodeType::DocumentType, doctype.name, None));
                continue;
            }
            Event::Start(element) | Event::Empty(element) => element,
            Event::End(name) => {
                depth -= 1;
                nodes.push(node(depth, NodeType::EndElement, name.qualified(), None));
                continue;
            }
            Event::Text(text) => {
                match nodes.last_mut() {
                    Some(last) if last.kind == NodeType::Text => {
                        last.value.as_mut().expect("text").push_str(text)
                    }
                    _ => nodes.push(node(depth, NodeType::Text, "#text", Some(text))),
                }
                continue;
            }
            Event::Cdata(text) => {
                nodes.push(node(depth, NodeType::Cdata, "#cdata-section", Some(text)));
                continue;
            }
            Event::Comment(text) => {
                nodes.push(node(depth, NodeType::Comment, "#comment", Some(text)));
                continue;
            }
            Event::ProcessingInstruction { target, data } => {
                let data = Some(data.as_ref()).filter(|data| !data.is_empty());
                nodes.push(node(depth, NodeType::ProcessingInstruction, target, data));
                continue;
            }
            Event::EntityStart(name) => {
                nodes.push(node(depth, NodeType::EntityReference, name, None));
                hidden = 1;
                continue;
            }
            Event::EntityEnd => continue,
        };
        let mut element = node(depth, NodeType::Element, kind.name.qualified(), None);
        element.empty = matches!(event, Event::Empty(_));
        for declarations in [true, false] {
            for attribute in &kind.attributes {
                let name = &attribute.name;
                if declarations == (name.prefix() == Some("xmlns") || name.qualified() == "xmlns") {
                    let value = attribute.value.to_string();
                    element
                        .attributes
                        .push((name.qualified().to_owned(), value));
                }
            }
        }
        depth += usize::from(!element.empty);
        nodes.push(element);
    }
    for node in &mut nodes {
        let blank = |value: &str| {
            value
                .bytes()
                .all(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r'))
        };
        if node.kind == NodeType::Text && node.value.as_deref().is_some_and(blank) {
            node.kind = NodeType::SignificantWhitespace;
        }
    }
    Some(nodes)
}

/// Yields one byte per `fill_buf`.
struct Trickle<'a>(&'a [u8]);

impl Read for Trickle<'_> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let n = self.0.len().min(out.len()).min(1);
        out[..n].copy_from_slice(&self.0[..n]);
        self.0 = &self.0[n..];
        Ok(n)
    }
}

impl BufRead for Trickle<'_> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        Ok(&self.0[..self.0.len().min(1)])
    }

    fn consume(&mut self, amount: usize) {
        self.0 = &self.0[amount..];
    }
}

fn current<R: BufRead>(reader: &mut TextReader<R>) -> Node {
    let mut node = node(
        reader.depth(),
        reader.node_type(),
        reader.name().expect("named node"),
        reader.value(),
    );
    node.empty = reader.is_empty_element();
    for index in 0..reader.attribute_count() {
        let indexed = reader.get_attribute_no(index).map(str::to_owned);
        assert!(reader.move_to_attribute_no(index));
        assert_eq!(reader.value(), indexed.as_deref());
        let name = reader.name().expect("attribute name").to_owned();
        node.attributes
            .push((name, indexed.expect("indexed attribute")));
        assert!(reader.move_to_element());
    }
    if node.kind == NodeType::Element {
        assert_eq!(reader.has_attributes(), !node.attributes.is_empty());
    }
    probe(reader);
    node
}

/// Name parts, namespaces, values and attribute lookups agree.
fn probe<R: BufRead>(reader: &mut TextReader<R>) {
    assert_eq!(reader.read_state(), ReadState::Interactive);
    let name = reader.name().expect("named node").to_owned();
    let local = reader.local_name().expect("local name").to_owned();
    match reader.prefix() {
        Some(prefix) => assert_eq!(name, format!("{prefix}:{local}")),
        None => assert_eq!(name, local),
    }
    if reader.node_type() == NodeType::Element {
        let bound = reader.lookup_namespace(reader.prefix());
        assert_eq!(bound.filter(|uri| !uri.is_empty()), reader.namespace_uri());
        assert_eq!(reader.lookup_namespace(Some("xml")), Some(XML));
        let count = reader.attribute_count();
        assert_eq!(reader.move_to_first_attribute(), count > 0);
        while reader.node_type() == NodeType::Attribute {
            let qualified = reader.name().expect("attribute name").to_owned();
            let value = reader.value().expect("attribute value").to_owned();
            let local = reader.local_name().expect("local name").to_owned();
            let namespace = reader.namespace_uri().map(str::to_owned);
            assert!(reader.has_value() && !reader.is_empty_element());
            assert_eq!(reader.attribute_count(), 0);
            assert_eq!(reader.get_attribute(&qualified), None);
            assert!(matches!(reader.read_string(), Ok(None)));
            assert!(reader.move_to_element());
            assert_eq!(reader.get_attribute(&qualified), Some(value.as_str()));
            assert!(
                reader
                    .get_attribute_ns(&local, namespace.as_deref())
                    .is_some()
            );
            if let Some(namespace) = &namespace {
                assert!(reader.move_to_attribute_ns(&local, namespace));
                assert!(reader.move_to_element());
            }
            assert!(reader.move_to_attribute(&qualified));
            assert_eq!(reader.value(), Some(value.as_str()));
            if !reader.move_to_next_attribute() {
                assert!(reader.move_to_element());
            }
        }
        assert!(!reader.move_to_attribute("::") && !reader.move_to_element());
        assert_eq!(reader.get_attribute("::"), None);
        assert!(!reader.move_to_attribute_no(count));
    } else {
        assert!(!reader.move_to_first_attribute() && !reader.move_to_attribute_ns("a", "u"));
        assert_eq!(
            (
                reader.get_attribute("a"),
                reader.get_attribute_ns("a", None)
            ),
            (None, None)
        );
        let _ = reader.has_attributes();
        assert_eq!(
            reader.has_value(),
            reader.value().is_some() || reader.node_type() == NodeType::ProcessingInstruction
        );
    }
    let _ = (reader.base_uri(), reader.xml_lang());
    for property in [
        ParserProperty::LoadDtd,
        ParserProperty::DefaultAttributes,
        ParserProperty::Validate,
        ParserProperty::SubstituteEntities,
    ] {
        let value = reader.parser_property(property);
        assert!(reader.set_parser_property(property, value).is_ok());
        assert!(reader.set_parser_property(property, !value).is_err());
    }
}

/// After the end or a failure the cursor stays there, on no node.
fn check_terminal<R: BufRead>(reader: &mut TextReader<R>, failed: bool) {
    if failed {
        assert_eq!(reader.read_state(), ReadState::Error);
        assert!(reader.read().is_err(), "errors are sticky");
    } else {
        assert_eq!(reader.read_state(), ReadState::Eof);
        assert!(!reader.read().expect("EOF is terminal"));
    }
    assert_eq!(reader.node_type(), NodeType::None);
    assert_eq!((reader.name(), reader.local_name()), (None, None));
    assert_eq!((reader.base_uri(), reader.xml_lang()), (None, None));
    assert_eq!(reader.lookup_namespace(None), None);
    assert!(!reader.has_value() && !reader.move_to_attribute_no(0));
    assert!(matches!(reader.read_string(), Ok(None)));
    assert!(matches!(reader.expand(), Ok(None)));
    assert_eq!(reader.value(), None);
}

/// Reads to the end, buffering every other subtree; true on failure.
fn drain<R: BufRead>(mut reader: TextReader<R>) -> bool {
    let mut count = 0usize;
    loop {
        let moved = if count % 3 == 2 {
            reader.next()
        } else {
            reader.read()
        };
        match moved {
            Ok(true) => {}
            Ok(false) => {
                check_terminal(&mut reader, false);
                return false;
            }
            Err(_) => {
                check_terminal(&mut reader, true);
                return true;
            }
        }
        count += 1;
        let _ = (reader.base_uri(), reader.xml_lang(), reader.depth());
        if count.is_multiple_of(2) {
            let _ = reader.read_outer_xml();
            let _ = reader.read_string();
        } else if count % 4 == 1 {
            let _ = reader.read_inner_xml();
            let _ = reader.expand();
        }
    }
}

/// Serves at most three bytes per `fill_buf`, then fails after `fail` bytes.
struct Failing<'a> {
    input: &'a [u8],
    fail: usize,
}

impl Read for Failing<'_> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let n = self.fill_buf()?.len().min(out.len());
        out[..n].copy_from_slice(&self.input[..n]);
        self.consume(n);
        Ok(n)
    }
}

impl BufRead for Failing<'_> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        if self.fail == 0 && !self.input.is_empty() {
            return Err(io::Error::other("injected"));
        }
        Ok(&self.input[..self.input.len().min(self.fail).min(3)])
    }

    fn consume(&mut self, amount: usize) {
        self.input = &self.input[amount..];
        self.fail -= amount;
    }
}

/// Modes 5-7; false for the reference modes.
fn special(control: u8, document: &[u8]) -> bool {
    match control >> 5 {
        5 => {
            let limits = Limits {
                max_owned_bytes: usize::from(control & 31) * 4,
                max_depth: 2 + usize::from(control & 7),
                ..Limits::default()
            };
            drain(TextReader::with_limits(document, limits));
        }
        6 => {
            let fail = document.len() * usize::from(control & 31) / 32;
            let failed = drain(TextReader::new(Failing {
                input: document,
                fail,
            }));
            // The whole input must be read, so the injected failure is met.
            assert!(failed || document.is_empty(), "I/O failure ignored");
        }
        7 => {
            let resolver = |request: Request<'_>| {
                Ok(match request.uri.as_bytes().last() {
                    Some(b'e') => Some(b"<x a='1'>e</x>".to_vec()),
                    Some(b'n') => None,
                    _ => Some(b"<!ENTITY e SYSTEM 'e'><!ATTLIST r d CDATA 'v'>".to_vec()),
                })
            };
            let flags = yaxml::options::LEGACY | yaxml::options::DTDLOAD;
            let options = ParseOptions::new(flags).with_resolver("d/doc.xml", Box::new(resolver));
            let reader = TextReader::with_options(document, options).unwrap();
            assert!(reader.parser_property(ParserProperty::LoadDtd));
            drain(reader);
        }
        _ => return false,
    }
    true
}

fn written(document: &OwnedDocument) -> Vec<u8> {
    let mut writer = Writer::new(Vec::new());
    writer
        .write_owned_document(document)
        .expect("owned documents serialize");
    writer.into_inner()
}

/// Attribute movement agrees with indexed and named lookup.
fn navigate<R: BufRead>(reader: &mut TextReader<R>, expected: &[(String, String)]) {
    let mut visited = Vec::new();
    while reader.move_to_next_attribute() {
        assert_eq!(reader.node_type(), NodeType::Attribute);
        let name = reader.name().expect("attribute name").to_owned();
        visited.push((
            name.clone(),
            reader.value().expect("attribute value").to_owned(),
        ));
        assert!(reader.move_to_element());
        assert_eq!(
            reader.get_attribute(&name),
            visited.last().map(|(_, value)| value.as_str())
        );
        assert!(reader.move_to_attribute(&name), "{name}");
    }
    assert_eq!(visited, expected);
    reader.move_to_element();
}

/// Subtree content agrees with the reference and with parsing its serialization.
fn content<R: BufRead>(reader: &mut TextReader<R>, nodes: &[Node]) -> Result<(), ReadError> {
    let end = match nodes[0].kind {
        NodeType::Element if !nodes[0].empty => nodes
            .iter()
            .position(|node| node.kind == NodeType::EndElement && node.depth == nodes[0].depth)
            .expect("balanced reference"),
        _ => 0,
    };
    let text: String = nodes[1..end.max(1)]
        .iter()
        .filter(|node| {
            matches!(
                node.kind,
                NodeType::Text | NodeType::SignificantWhitespace | NodeType::Cdata
            )
        })
        .filter_map(|node| node.value.as_deref())
        .collect();
    let string = reader.read_string()?;
    match nodes[0].kind {
        NodeType::Element if end > 1 => assert_eq!(string, Some(text)),
        NodeType::Text | NodeType::SignificantWhitespace | NodeType::Cdata => {
            assert_eq!(string, nodes[0].value)
        }
        _ => assert_eq!(string, None),
    }
    let outer = reader.read_outer_xml()?;
    let serializable = !matches!(nodes[0].kind, NodeType::DocumentType | NodeType::EndElement);
    assert_eq!(outer.is_some(), serializable);
    // Kept entity references serialize as `&name;`, which needs their DTD.
    let references = nodes[..=end]
        .iter()
        .any(|node| node.kind == NodeType::EntityReference);
    if nodes[0].kind == NodeType::Element && references {
        let expanded = reader.expand()?.expect("element expands");
        let name = expanded
            .root_element()
            .name()
            .map(|name| name.qualified().to_owned());
        assert_eq!(name.as_deref(), Some(nodes[0].name.as_str()));
    } else if nodes[0].kind == NodeType::Element {
        let outer = outer.expect("element serializes");
        let parsed = OwnedDocument::parse(&outer).expect("outer XML reparses");
        let expanded = reader.expand()?.expect("element expands");
        assert_eq!(written(&expanded), written(&parsed));
        let inner = reader.read_inner_xml()?.expect("element content");
        // Each child carries the declarations it uses, so the children stand alone.
        let children =
            OwnedDocument::parse(&format!("<w>{inner}</w>")).expect("inner XML reparses");
        assert_eq!(
            children.root_element().children().count(),
            parsed.root_element().children().count()
        );
    }
    Ok(())
}

pub fn exercise(input: &[u8]) {
    let Some((&control, document)) = input.split_first() else {
        return;
    };
    if special(control, document) {
        return;
    }
    let expected = reference(document);
    let trickle = control & 1 != 0;
    let mut nodes = Vec::new();
    let failed = if trickle {
        walk(
            TextReader::new(Trickle(document)),
            control,
            expected.as_deref(),
            &mut nodes,
        )
    } else {
        let base = (control >> 5 == 4).then(|| "http://h/d/doc.xml".to_owned());
        let options = ParseOptions {
            base,
            ..ParseOptions::default()
        };
        walk(
            TextReader::with_options(document, options).unwrap(),
            control,
            expected.as_deref(),
            &mut nodes,
        )
    };
    match &expected {
        Some(expected) => {
            assert!(!failed, "slice reader accepts, TextReader rejects");
            assert_eq!(&nodes, expected);
        }
        // Slice validation rejects encodings and characters before any event,
        // so a streamed prefix is not compared.
        None => assert!(failed || slice_rejects_encoding(document)),
    }
    if control & 0x0e == 0x0e {
        validated(document, control);
    }
    if let Some(expected) = &expected {
        let mut reader = TextReader::new(document);
        let mut skipped = Vec::new();
        let mut more = reader.read().expect("accepted input");
        while more {
            skipped.push(reader.name().expect("named node").to_owned());
            more = reader.next().expect("accepted input");
        }
        let top: Vec<_> = expected
            .iter()
            .filter(|node| node.depth == 0 && node.kind != NodeType::EndElement)
            .map(|node| node.name.clone())
            .collect();
        assert_eq!(skipped, top);
    }
}

/// The slice reader fails up front, or on a declared single-byte encoding
/// that only the streaming decoder switches to.
fn slice_rejects_encoding(document: &[u8]) -> bool {
    let Ok(mut reader) = Reader::new(document) else {
        return true;
    };
    loop {
        match reader.read_event() {
            Ok(Some(_)) => {}
            Ok(None) => return false,
            Err(error) => return error.kind == ErrorKind::UnsupportedEncoding,
        }
    }
}

/// Reads every node, comparing each with the reference as it goes; true on failure.
fn walk<R: BufRead>(
    mut reader: TextReader<R>,
    control: u8,
    expected: Option<&[Node]>,
    nodes: &mut Vec<Node>,
) -> bool {
    loop {
        match reader.read() {
            Ok(true) => {}
            Ok(false) => {
                check_terminal(&mut reader, false);
                return false;
            }
            Err(_) => {
                check_terminal(&mut reader, true);
                return true;
            }
        }
        let node = current(&mut reader);
        let reference = expected.and_then(|expected| expected.get(nodes.len()..));
        if let Some(reference) = reference.filter(|reference| !reference.is_empty()) {
            let check = control.rotate_left(nodes.len() as u32);
            if check & 2 != 0 {
                navigate(&mut reader, &reference[0].attributes);
            }
            if check & 4 != 0 && content(&mut reader, reference).is_err() {
                return true;
            }
        }
        nodes.push(node);
    }
}

/// Streaming RELAX NG and XSD validation through the reader: schemas attach
/// only before the first read, validity summarizes the diagnostics, and
/// tree-only flags are rejected.
fn validated(document: &[u8], control: u8) {
    const RNG: &[u8] = b"<element xmlns='http://relaxng.org/ns/structure/1.0'><anyName/>\
        <zeroOrMore><choice><attribute><anyName/></attribute><text/>\
        <element><anyName/><empty/></element></choice></zeroOrMore></element>";
    const XSD: &[u8] = b"<xs:schema xmlns:xs='http://www.w3.org/2001/XMLSchema'>\
        <xs:element name='a'><xs:complexType mixed='true'><xs:sequence>\
        <xs:any processContents='lax' minOccurs='0' maxOccurs='unbounded'/></xs:sequence>\
        <xs:anyAttribute processContents='lax'/></xs:complexType></xs:element></xs:schema>";
    let none = Box::new(|_: Request<'_>| Ok(None));
    let rng = relaxng::Schema::parse(RNG, "s.rng", none, &relaxng::Limits::default());
    let schema = xsd::Schema::parse(XSD, "s.xsd").expect("fixed schema");
    let mut reader = TextReader::new(document);
    let (with_rng, with_xsd) = (control & 0x10 == 0, control & 0x80 == 0);
    if with_rng {
        reader
            .relaxng_set_schema(Some(&rng.expect("fixed schema")))
            .unwrap();
    }
    if with_xsd {
        reader.xsd_set_schema(Some(&schema)).unwrap();
    }
    while reader.read().unwrap_or(false) {}
    let relaxng = with_rng.then(|| reader.relaxng_diagnostics().is_empty());
    let xsd = with_xsd.then(|| xsd::is_valid(reader.xsd_diagnostics()));
    let valid = relaxng.or(xsd).is_some() && relaxng != Some(false) && xsd != Some(false);
    assert_eq!(reader.is_valid(), valid);
    let state = Err(ErrorKind::InvalidState);
    assert_eq!(reader.relaxng_set_schema(None).map_err(|e| e.kind), state);
    assert_eq!(reader.xsd_set_schema(None).map_err(|e| e.kind), state);
    let tree_only = ParseOptions::new(options::RECOVER);
    let rejected = TextReader::with_options(document, tree_only).map(drop);
    assert_eq!(
        rejected.map_err(|e| e.kind),
        Err(ErrorKind::UnsupportedOption)
    );
}
