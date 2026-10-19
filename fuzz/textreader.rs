//! `TextReader` against the slice `Reader`. A testcase is one control byte
//! followed by an XML document. The node sequence must equal the projection
//! of the reader's events under libxml2's node rules, and acceptance must
//! agree. Bit 0 trickles the input one byte per `fill_buf`; per node, the
//! rotated control byte selects content, expansion and navigation checks.
//! A second pass applies `next` at every node.

use std::io::{self, BufRead, Read};

use yaxml::{Event, NodeType, OwnedDocument, ReadError, Reader, TextReader, Writer};

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
    let (mut nodes, mut depth) = (Vec::<Node>::new(), 0);
    while let Some(event) = reader.read_event().ok()? {
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
    node
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
    if nodes[0].kind == NodeType::Element {
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
        walk(
            TextReader::new(document),
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
        None => assert!(failed || Reader::new(document).is_err()),
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
            Ok(false) => return false,
            Err(_) => return true,
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
