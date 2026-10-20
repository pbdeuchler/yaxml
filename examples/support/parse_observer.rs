//! Canonical semantic observation for matched XML parsing benchmarks.

use yaxml::{
    Attribute, Document, Element, Event, Name, Node, NodeKind, OwnedDocument, OwnedNode,
    OwnedNodeKind,
};

const FNV_OFFSET: u64 = 14_695_981_039_346_656_037;
const FNV_PRIME: u64 = 1_099_511_628_211;
const XMLNS_NAMESPACE: &str = "http://www.w3.org/2000/xmlns/";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Observation {
    pub(crate) elements: u64,
    pub(crate) attributes: u64,
    pub(crate) namespaces: u64,
    pub(crate) text_bytes: u64,
    pub(crate) comments: u64,
    pub(crate) pis: u64,
    pub(crate) hash: u64,
}

pub(crate) struct Observer {
    observation: Observation,
    pending_text: bool,
}

impl Observer {
    pub(crate) fn new() -> Self {
        Self {
            observation: Observation {
                elements: 0,
                attributes: 0,
                namespaces: 0,
                text_bytes: 0,
                comments: 0,
                pis: 0,
                hash: FNV_OFFSET,
            },
            pending_text: false,
        }
    }

    /// Observes a lent (`&Event`) or an owned event, which it then drops,
    /// as a consumer of either API would.
    pub(crate) fn event<'e>(&mut self, event: impl std::borrow::Borrow<Event<'e>>) {
        match event.borrow() {
            Event::Declaration(_)
            | Event::Doctype(_)
            | Event::EntityStart(_)
            | Event::EntityEnd => {}
            Event::Start(element) => self.event_start(element),
            Event::Empty(element) => {
                self.event_start(element);
                self.end(&element.name);
            }
            Event::End(name) => self.end(name),
            Event::Text(value) | Event::Cdata(value) => self.text(value),
            Event::Comment(value) => self.comment(value),
            Event::ProcessingInstruction { target, data } => self.pi(target, data),
        }
    }

    pub(crate) fn finish(mut self) -> Observation {
        self.flush_text();
        self.observation
    }

    fn event_start(&mut self, element: &Element<'_>) {
        self.borrowed_start(element);
    }

    fn borrowed_start(&mut self, element: &Element<'_>) {
        self.begin_start(&element.name);
        for attribute in element
            .attributes
            .iter()
            .filter(|attribute| is_namespace(attribute))
        {
            self.namespace(attribute);
        }
        for attribute in element
            .attributes
            .iter()
            .filter(|attribute| !is_namespace(attribute))
        {
            self.ordinary_attribute(attribute);
        }
        self.finish_start();
    }

    fn owned_start(&mut self, node: OwnedNode<'_>, name: &Name<'_>) {
        self.begin_start(name);
        for attribute in node.attributes().filter(is_namespace) {
            self.namespace(&attribute);
        }
        for attribute in node
            .attributes()
            .filter(|attribute| !is_namespace(attribute))
        {
            self.ordinary_attribute(&attribute);
        }
        self.finish_start();
    }

    fn begin_start(&mut self, name: &Name<'_>) {
        self.flush_text();
        self.byte(b'S');
        self.field(name.qualified());
        self.field(name.namespace().unwrap_or(""));
    }

    fn namespace(&mut self, attribute: &Attribute<'_>) {
        self.byte(b'N');
        self.field(if attribute.name.qualified() == "xmlns" {
            ""
        } else {
            attribute.name.local()
        });
        self.field(&attribute.value);
        self.observation.namespaces += 1;
    }

    fn ordinary_attribute(&mut self, attribute: &Attribute<'_>) {
        self.byte(b'A');
        self.field(attribute.name.qualified());
        self.field(attribute.name.namespace().unwrap_or(""));
        self.field(&attribute.value);
        self.observation.attributes += 1;
    }

    fn finish_start(&mut self) {
        self.byte(b'>');
        self.observation.elements += 1;
    }

    fn end(&mut self, name: &Name<'_>) {
        self.flush_text();
        self.byte(b'E');
        self.field(name.qualified());
        self.field(name.namespace().unwrap_or(""));
    }

    fn text(&mut self, value: &str) {
        if value.is_empty() {
            return;
        }
        if !self.pending_text {
            self.byte(b'T');
            self.pending_text = true;
        }
        self.bytes(value.as_bytes());
        self.observation.text_bytes += value.len() as u64;
    }

    fn flush_text(&mut self) {
        if self.pending_text {
            self.byte(0);
            self.pending_text = false;
        }
    }

    fn comment(&mut self, value: &str) {
        self.flush_text();
        self.byte(b'C');
        self.field(value);
        self.observation.comments += 1;
    }

    fn pi(&mut self, target: &str, data: &str) {
        self.flush_text();
        self.byte(b'P');
        self.field(target);
        self.field(data);
        self.observation.pis += 1;
    }

    fn field(&mut self, value: &str) {
        self.bytes(&(value.len() as u64).to_le_bytes());
        self.bytes(value.as_bytes());
    }

    fn byte(&mut self, byte: u8) {
        self.observation.hash ^= u64::from(byte);
        self.observation.hash = self.observation.hash.wrapping_mul(FNV_PRIME);
    }

    fn bytes(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.byte(byte);
        }
    }
}

pub(crate) fn borrowed_document(document: &Document<'_>) -> Result<Observation, String> {
    let mut observer = Observer::new();
    let root = document.root();
    let mut next = root.first_child();
    let mut open = Vec::new();
    while let Some(node) = next {
        if let NodeKind::Element(element) = node.kind() {
            observer.borrowed_start(element);
            if let Some(child) = node.first_child() {
                open.push(node);
                next = Some(child);
                continue;
            }
            observer.end(&element.name);
        } else {
            borrowed_leaf(&mut observer, node)?;
        }
        next = node.next_sibling();
        while next.is_none() {
            let Some(parent) = open.pop() else {
                break;
            };
            let NodeKind::Element(element) = parent.kind() else {
                return Err("non-element appears in the DOM traversal stack".into());
            };
            observer.end(&element.name);
            next = parent.next_sibling();
        }
    }
    Ok(observer.finish())
}

fn borrowed_leaf(observer: &mut Observer, node: Node<'_, '_>) -> Result<(), String> {
    match node.kind() {
        NodeKind::Text(value) | NodeKind::Cdata(value) => observer.text(value),
        NodeKind::Comment(value) => observer.comment(value),
        NodeKind::ProcessingInstruction { target, data } => observer.pi(target, data),
        NodeKind::Doctype(_) | NodeKind::EntityRef(_) => {}
        NodeKind::Document => return Err("document node appears below the synthetic root".into()),
        NodeKind::Element(_) => return Err("element reached the DOM leaf observer".into()),
    }
    Ok(())
}

pub(crate) fn owned_document(document: &OwnedDocument) -> Result<Observation, String> {
    let mut observer = Observer::new();
    let root = document.root();
    let mut next = root.first_child();
    let mut open = Vec::new();
    while let Some(node) = next {
        if node.kind() == OwnedNodeKind::Element {
            let name = node
                .name()
                .ok_or("owned element is missing its semantic name")?;
            observer.owned_start(node, &name);
            if let Some(child) = node.first_child() {
                open.push(node);
                next = Some(child);
                continue;
            }
            observer.end(&name);
        } else {
            owned_leaf(&mut observer, node)?;
        }
        next = node.next_sibling();
        while next.is_none() {
            let Some(parent) = open.pop() else {
                break;
            };
            if parent.kind() != OwnedNodeKind::Element {
                return Err("non-element appears in the owned traversal stack".into());
            }
            let name = parent
                .name()
                .ok_or("owned element is missing its semantic name")?;
            observer.end(&name);
            next = parent.next_sibling();
        }
    }
    Ok(observer.finish())
}

fn owned_leaf(observer: &mut Observer, node: OwnedNode<'_>) -> Result<(), String> {
    match node.kind() {
        OwnedNodeKind::Text | OwnedNodeKind::Cdata => {
            observer.text(node.text().ok_or("owned text node is missing its value")?);
        }
        OwnedNodeKind::Comment => {
            observer.comment(node.text().ok_or("owned comment is missing its value")?);
        }
        OwnedNodeKind::ProcessingInstruction => observer.pi(
            node.processing_instruction_target()
                .ok_or("owned processing instruction is missing its target")?,
            node.text()
                .ok_or("owned processing instruction is missing its data")?,
        ),
        OwnedNodeKind::DocumentType => {}
        OwnedNodeKind::Document => {
            return Err("document node appears below the owned synthetic root".into());
        }
        OwnedNodeKind::Element => return Err("element reached the owned leaf observer".into()),
        _ => return Err("unsupported owned node kind reached the observer".into()),
    }
    Ok(())
}

fn is_namespace(attribute: &Attribute<'_>) -> bool {
    attribute.name.namespace() == Some(XMLNS_NAMESPACE)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reference_hash(bytes: &[u8]) -> u64 {
        let mut hash = FNV_OFFSET;
        for &byte in bytes {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(FNV_PRIME);
        }
        hash
    }

    fn literal_field(transcript: &mut Vec<u8>, value: &str) {
        transcript.extend_from_slice(&(value.len() as u64).to_le_bytes());
        transcript.extend_from_slice(value.as_bytes());
    }

    #[test]
    fn canonical_transcript_has_independent_counts_and_hash() {
        let input = "<!--pre--><?top d?><p:r a='x&#x9;y' xmlns='urn:d' p:b='&amp;' xmlns:p='urn:p'>x&amp;<![CDATA[y]]>&#x7a;<p:e/><!--c--><?pi data?></p:r><!--post-->";
        let document = Document::parse(input).unwrap();
        let observed = borrowed_document(&document).unwrap();

        let mut transcript = Vec::new();
        transcript.push(b'C');
        literal_field(&mut transcript, "pre");
        transcript.push(b'P');
        literal_field(&mut transcript, "top");
        literal_field(&mut transcript, "d");
        transcript.push(b'S');
        literal_field(&mut transcript, "p:r");
        literal_field(&mut transcript, "urn:p");
        transcript.push(b'N');
        literal_field(&mut transcript, "");
        literal_field(&mut transcript, "urn:d");
        transcript.push(b'N');
        literal_field(&mut transcript, "p");
        literal_field(&mut transcript, "urn:p");
        transcript.push(b'A');
        literal_field(&mut transcript, "a");
        literal_field(&mut transcript, "");
        literal_field(&mut transcript, "x\ty");
        transcript.push(b'A');
        literal_field(&mut transcript, "p:b");
        literal_field(&mut transcript, "urn:p");
        literal_field(&mut transcript, "&");
        transcript.push(b'>');
        transcript.extend_from_slice(b"Tx&yz\0");
        transcript.push(b'S');
        literal_field(&mut transcript, "p:e");
        literal_field(&mut transcript, "urn:p");
        transcript.push(b'>');
        transcript.push(b'E');
        literal_field(&mut transcript, "p:e");
        literal_field(&mut transcript, "urn:p");
        transcript.push(b'C');
        literal_field(&mut transcript, "c");
        transcript.push(b'P');
        literal_field(&mut transcript, "pi");
        literal_field(&mut transcript, "data");
        transcript.push(b'E');
        literal_field(&mut transcript, "p:r");
        literal_field(&mut transcript, "urn:p");
        transcript.push(b'C');
        literal_field(&mut transcript, "post");

        assert_eq!(
            observed,
            Observation {
                elements: 2,
                attributes: 2,
                namespaces: 2,
                text_bytes: 4,
                comments: 3,
                pis: 2,
                hash: reference_hash(&transcript),
            }
        );
    }
}
