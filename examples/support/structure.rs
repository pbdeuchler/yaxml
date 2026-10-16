//! Bounded, one-operation structural comparison with an external libxml2 tool.

use std::io;

use yaxml::{EditErrorKind, EditLimits, OwnedDocument, OwnedNodeKind, Writer};

pub const PROTOCOL: &str = "YAXML-STRUCTURE-1";
pub const VERSION: &str = "21504";
pub const MAX_OUTPUT: usize = 8 * 1024 * 1024;
const MAX_NODES: usize = 256;
const MAX_ATTRIBUTES: usize = 4096;
const XMLNS: &str = "http://www.w3.org/2000/xmlns/";

#[derive(Clone, Debug, Eq, PartialEq)]
struct Attribute {
    name: Vec<u8>,
    namespace: Vec<u8>,
    value: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Node {
    kind: u8,
    parent: Option<usize>,
    name: Vec<u8>,
    namespace: Vec<u8>,
    value: Vec<u8>,
    attributes: Vec<Attribute>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Snapshot(Vec<Node>);

impl Snapshot {
    fn owned(document: &OwnedDocument) -> io::Result<Self> {
        let nodes: Vec<_> = document.nodes().collect();
        if nodes.len() > MAX_NODES {
            return Err(invalid("structural node ceiling"));
        }
        let mut attributes = 0;
        let values = nodes
            .iter()
            .map(|node| {
                let name = node.name();
                let attrs = node
                    .attributes()
                    .filter(|attribute| attribute.name.namespace() != Some(XMLNS))
                    .map(|attribute| {
                        attributes += 1;
                        Attribute {
                            name: attribute.name.qualified().as_bytes().to_vec(),
                            namespace: attribute.name.namespace().unwrap_or("").as_bytes().to_vec(),
                            value: attribute.value.as_bytes().to_vec(),
                        }
                    })
                    .collect();
                Node {
                    kind: match node.kind() {
                        OwnedNodeKind::Document => b'D',
                        OwnedNodeKind::Element => b'E',
                        OwnedNodeKind::Text => b'T',
                        OwnedNodeKind::Cdata => b'C',
                        OwnedNodeKind::Comment => b'M',
                        OwnedNodeKind::ProcessingInstruction => b'P',
                        _ => unreachable!("unknown owned kind"),
                    },
                    parent: node.parent().map(|parent| {
                        nodes
                            .iter()
                            .position(|node| node.id() == parent.id())
                            .unwrap()
                    }),
                    name: name
                        .as_ref()
                        .map(|name| name.qualified())
                        .or_else(|| node.processing_instruction_target())
                        .unwrap_or("")
                        .as_bytes()
                        .to_vec(),
                    namespace: name
                        .as_ref()
                        .and_then(|name| name.namespace())
                        .unwrap_or("")
                        .as_bytes()
                        .to_vec(),
                    value: node.text().unwrap_or("").as_bytes().to_vec(),
                    attributes: attrs,
                }
            })
            .collect();
        if attributes > MAX_ATTRIBUTES {
            return Err(invalid("structural attribute ceiling"));
        }
        Ok(Self(values))
    }

    fn descendant(&self, mut node: usize, ancestor: usize) -> bool {
        loop {
            if node == ancestor {
                return true;
            }
            let Some(parent) = self.0[node].parent else {
                return false;
            };
            node = parent;
        }
    }

    fn append(&mut self, parent: usize, mut leaf: Node) {
        let mut position = parent + 1;
        while position < self.0.len() && self.descendant(position, parent) {
            position += 1;
        }
        for node in &mut self.0 {
            if let Some(parent) = &mut node.parent
                && *parent >= position
            {
                *parent += 1;
            }
        }
        leaf.parent = Some(parent);
        self.0.insert(position, leaf);
    }

    fn remove(&mut self, root: usize) {
        let keep: Vec<_> = (0..self.0.len())
            .filter(|index| !self.descendant(*index, root))
            .collect();
        self.0 = keep
            .iter()
            .map(|index| {
                let mut node = self.0[*index].clone();
                node.parent = node
                    .parent
                    .map(|parent| keep.iter().position(|index| *index == parent).unwrap());
                node
            })
            .collect();
    }

    fn text_runs(&self) -> Self {
        let mut nodes: Vec<Node> = Vec::new();
        let mut positions = Vec::new();
        for node in &self.0 {
            let parent = node.parent.map(|parent| positions[parent]);
            if node.kind == b'T'
                && nodes
                    .last()
                    .is_some_and(|previous| previous.kind == b'T' && previous.parent == parent)
            {
                let index = nodes.len() - 1;
                nodes[index].value.extend_from_slice(&node.value);
                positions.push(index);
            } else {
                let mut node = node.clone();
                node.parent = parent;
                positions.push(nodes.len());
                nodes.push(node);
            }
        }
        Self(nodes)
    }
}

#[derive(Clone, Copy)]
enum Action {
    Text(&'static str),
    Cdata(&'static str),
    Comment(&'static str),
    Pi(&'static str, &'static str),
    Remove,
}

impl Action {
    fn fields(self) -> (usize, &'static str, &'static str) {
        match self {
            Self::Text(value) => (1, "", value),
            Self::Cdata(value) => (2, "", value),
            Self::Comment(value) => (3, "", value),
            Self::Pi(target, value) => (4, target, value),
            Self::Remove => (5, "", ""),
        }
    }

    fn leaf(self) -> Node {
        let (kind, target, value) = self.fields();
        Node {
            kind: [b'?', b'T', b'C', b'M', b'P'][kind],
            parent: None,
            name: target.as_bytes().to_vec(),
            namespace: Vec::new(),
            value: value.as_bytes().to_vec(),
            attributes: Vec::new(),
        }
    }
}

#[derive(Clone, Copy)]
enum Policy {
    Exact,
    TextRuns,
    Semantic(&'static str, &'static [u8]),
    Rejected(EditErrorKind, Option<usize>, &'static str),
    Skipped(&'static str),
}

pub struct Case {
    pub name: &'static str,
    input: &'static str,
    target: usize,
    action: Action,
    policy: Policy,
    encoding: u8,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Outcome {
    Exact,
    Projected(&'static str),
    Difference(&'static str),
    Skipped(&'static str),
}

#[derive(Debug, Eq, PartialEq)]
pub enum CheckError {
    Protocol,
    Mismatch(&'static str),
}

pub struct Expected {
    before: Snapshot,
    after: Snapshot,
    bytes_before: Vec<u8>,
    bytes_after: Option<Vec<u8>>,
    returned: usize,
    outcome: Outcome,
}

impl Case {
    pub fn skipped(&self) -> Option<Outcome> {
        match self.policy {
            Policy::Skipped(reason) => Some(Outcome::Skipped(reason)),
            _ => None,
        }
    }

    fn input(&self) -> Vec<u8> {
        if self.encoding == 0 {
            return self.input.as_bytes().to_vec();
        }
        std::iter::once(0xfeff)
            .chain(self.input.encode_utf16())
            .flat_map(|unit| {
                if self.encoding == 1 {
                    unit.to_le_bytes()
                } else {
                    unit.to_be_bytes()
                }
            })
            .collect()
    }

    pub fn request(&self) -> Vec<u8> {
        let mut output = format!("{PROTOCOL}\n").into_bytes();
        let (action, target, value) = self.action.fields();
        number(&mut output, action);
        number(&mut output, self.target);
        frame(&mut output, &self.input());
        frame(&mut output, target.as_bytes());
        frame(&mut output, value.as_bytes());
        output
    }

    pub fn expected(&self) -> io::Result<Expected> {
        let mut document = OwnedDocument::parse_bytes(&self.input()).map_err(io::Error::other)?;
        let before = Snapshot::owned(&document)?;
        let bytes_before = serialized(&document)?;
        let handles: Vec<_> = document.nodes().map(|node| node.id()).collect();
        let operand = document
            .nodes()
            .nth(self.target)
            .ok_or_else(|| invalid("case target out of range"))?;
        let id = operand.id();
        let removed: Vec<_> = if matches!(self.action, Action::Remove) {
            operand.descendants().map(|node| node.id()).collect()
        } else {
            Vec::new()
        };
        let previous = operand.last_child().map(|node| (node.id(), node.kind()));
        let returned = if matches!(self.action, Action::Remove) {
            3
        } else if operand.kind() == OwnedNodeKind::Text {
            2
        } else if matches!(self.action, Action::Text(_))
            && previous.is_some_and(|(_, kind)| kind == OwnedNodeKind::Text)
        {
            1
        } else {
            0
        };
        let mut editor = document
            .edit(EditLimits::default())
            .map_err(io::Error::other)?;
        let result = match self.action {
            Action::Text(value) => editor.append_text(id, value).map(Some),
            Action::Cdata(value) => editor.append_cdata(id, value).map(Some),
            Action::Comment(value) => editor.append_comment(id, value).map(Some),
            Action::Pi(target, value) => editor
                .append_processing_instruction(id, target, value)
                .map(Some),
            Action::Remove => editor.remove_subtree(id).map(|()| None),
        };
        let mut after;
        let mut bytes_after;
        let outcome = match self.policy {
            Policy::Rejected(kind, offset, reason) => {
                let error = result
                    .err()
                    .ok_or_else(|| invalid("Rust accepted an intentionally rejected operation"))?;
                if error.kind != kind || error.value_offset != offset {
                    return Err(invalid("Rust rejection category or index changed"));
                }
                if Snapshot::owned(editor.document())? != before
                    || serialized(editor.document())? != bytes_before
                {
                    return Err(invalid("Rust rejection changed the document"));
                }
                after = before.clone();
                match self.action {
                    Action::Remove => after.remove(self.target),
                    Action::Text(value) if returned == 2 => after.0[self.target]
                        .value
                        .extend_from_slice(value.as_bytes()),
                    action => after.append(self.target, action.leaf()),
                }
                bytes_after = None;
                Outcome::Difference(reason)
            }
            policy => {
                let created = result.map_err(io::Error::other)?;
                if let Some(created) = created
                    && (handles.contains(&created)
                        || editor.document().node(created).is_none()
                        || editor.document().node(created).unwrap().range().is_some())
                {
                    return Err(invalid("Rust insertion did not create a fresh live handle"));
                }
                for old in &handles {
                    if editor.document().node(*old).is_some() == removed.contains(old) {
                        return Err(invalid("Rust survivor or stale-handle contract changed"));
                    }
                }
                after = Snapshot::owned(editor.document())?;
                bytes_after = Some(serialized(editor.document())?);
                match policy {
                    Policy::Exact => Outcome::Exact,
                    Policy::TextRuns => {
                        after = after.text_runs();
                        Outcome::Projected("Text coalescing and returned sibling")
                    }
                    Policy::Semantic(reason, bytes) => {
                        bytes_after = Some(bytes.to_vec());
                        Outcome::Projected(reason)
                    }
                    _ => return Err(invalid("skipped case cannot be executed")),
                }
            }
        };
        Ok(Expected {
            before,
            after,
            bytes_before,
            bytes_after,
            returned,
            outcome,
        })
    }
}

impl Expected {
    pub fn verify(&self, bytes: &[u8]) -> Result<Outcome, CheckError> {
        if bytes.len() > MAX_OUTPUT {
            return Err(CheckError::Protocol);
        }
        let mut input = Input(bytes);
        if input.line().map_err(|()| CheckError::Protocol)? != PROTOCOL.as_bytes() {
            return Err(CheckError::Protocol);
        }
        let (before, bytes_before) = input.snapshot().map_err(|()| CheckError::Protocol)?;
        let returned = input.number(b'\n', 3).map_err(|()| CheckError::Protocol)?;
        let (after, bytes_after) = input.snapshot().map_err(|()| CheckError::Protocol)?;
        if !input.0.is_empty() {
            return Err(CheckError::Protocol);
        }
        for (equal, reason) in [
            (
                before == self.before,
                "initial kinds, order, names or values",
            ),
            (
                bytes_before == self.bytes_before,
                "initial serialized bytes",
            ),
            (
                returned == self.returned,
                "returned survivor classification",
            ),
            (after == self.after, "result kinds, order, names or values"),
            (
                self.bytes_after
                    .as_ref()
                    .is_none_or(|expected| *expected == bytes_after),
                "result serialized bytes",
            ),
        ] {
            if !equal {
                return Err(CheckError::Mismatch(reason));
            }
        }
        Ok(self.outcome.clone())
    }
}

fn serialized(document: &OwnedDocument) -> io::Result<Vec<u8>> {
    let mut writer = Writer::new(Vec::new());
    writer
        .write_owned_document(document)
        .map_err(io::Error::other)?;
    Ok(writer.into_inner())
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn number(output: &mut Vec<u8>, value: usize) {
    output.extend_from_slice(format!("{value}\n").as_bytes());
}
fn frame(output: &mut Vec<u8>, value: &[u8]) {
    output.extend_from_slice(format!("{}:", value.len()).as_bytes());
    output.extend_from_slice(value);
    output.push(b'\n');
}

struct Input<'a>(&'a [u8]);

impl<'a> Input<'a> {
    fn line(&mut self) -> Result<&'a [u8], ()> {
        let end = self.0.iter().position(|byte| *byte == b'\n').ok_or(())?;
        let value = &self.0[..end];
        self.0 = &self.0[end + 1..];
        Ok(value)
    }

    fn number(&mut self, delimiter: u8, maximum: usize) -> Result<usize, ()> {
        let end = self
            .0
            .iter()
            .position(|byte| *byte == delimiter)
            .ok_or(())?;
        if end == 0 || end > 20 {
            return Err(());
        }
        let mut value: usize = 0;
        for digit in &self.0[..end] {
            if !digit.is_ascii_digit() {
                return Err(());
            }
            value = value
                .checked_mul(10)
                .and_then(|value| value.checked_add(usize::from(digit - b'0')))
                .filter(|value| *value <= maximum)
                .ok_or(())?;
        }
        self.0 = &self.0[end + 1..];
        Ok(value)
    }

    fn frame(&mut self) -> Result<Vec<u8>, ()> {
        let length = self.number(b':', MAX_OUTPUT)?;
        if self.0.len() <= length || self.0[length] != b'\n' {
            return Err(());
        }
        let value = self.0[..length].to_vec();
        self.0 = &self.0[length + 1..];
        Ok(value)
    }

    fn snapshot(&mut self) -> Result<(Snapshot, Vec<u8>), ()> {
        let count = self.number(b'\n', MAX_NODES)?;
        if count == 0 {
            return Err(());
        }
        let mut nodes = Vec::new();
        let mut total_attributes = 0;
        for index in 0..count {
            let kind = match self.line()? {
                [kind @ (b'D' | b'E' | b'T' | b'C' | b'M' | b'P')] => *kind,
                _ => return Err(()),
            };
            if (index == 0) != (kind == b'D') {
                return Err(());
            }
            let parent = self.number(b'\n', index)?;
            if (index == 0) != (parent == 0) {
                return Err(());
            }
            let name = self.frame()?;
            let namespace = self.frame()?;
            let value = self.frame()?;
            let attributes = self.number(b'\n', MAX_ATTRIBUTES - total_attributes)?;
            total_attributes += attributes;
            let mut attrs = Vec::new();
            for _ in 0..attributes {
                attrs.push(Attribute {
                    name: self.frame()?,
                    namespace: self.frame()?,
                    value: self.frame()?,
                });
            }
            nodes.push(Node {
                kind,
                parent: parent.checked_sub(1),
                name,
                namespace,
                value,
                attributes: attrs,
            });
        }
        Ok((Snapshot(nodes), self.frame()?))
    }
}

pub fn validate_version(report: &[u8], exploratory: bool) -> io::Result<()> {
    let report = std::str::from_utf8(report).map_err(io::Error::other)?;
    let fields: Vec<_> = report.split_whitespace().collect();
    if fields.len() != 5
        || fields[0] != PROTOCOL
        || fields[3] != "tree=1"
        || fields[4] != "output=1"
    {
        return Err(invalid("invalid structural oracle version/features"));
    }
    for (field, prefix) in [(fields[1], "compiled="), (fields[2], "runtime=")] {
        let version = field
            .strip_prefix(prefix)
            .filter(|version| {
                !version.is_empty() && version.bytes().all(|byte| byte.is_ascii_digit())
            })
            .ok_or_else(|| invalid("invalid structural oracle version"))?;
        if !exploratory && version != VERSION {
            return Err(invalid(
                "structural oracle requires libxml2 2.15.4 (21504); --allow-other-libxml2 enables exploratory comparison",
            ));
        }
    }
    Ok(())
}

pub fn cases() -> Vec<Case> {
    use Action::{Cdata, Comment, Pi, Remove, Text};
    use Policy::{Exact, Rejected, Semantic, Skipped, TextRuns};
    let mut cases = Vec::new();
    let mut add = |name, input, target, action, policy| {
        cases.push(Case {
            name,
            input,
            target,
            action,
            policy,
            encoding: 0,
        })
    };
    add("append-text", "<r/>", 1, Text("x"), Exact);
    add("append-cdata", "<r/>", 1, Cdata("é\n\t&<>"), Exact);
    add("append-comment", "<r/>", 1, Comment("é\n\t&<>"), Exact);
    add("append-pi", "<r/>", 1, Pi("空", "data"), Exact);
    add(
        "append-document-comment",
        "<!--pre--><r/><!--post-->",
        0,
        Comment("tail"),
        Exact,
    );
    add(
        "append-document-pi",
        "<r/><!--post-->",
        0,
        Pi("p", "tail"),
        Exact,
    );
    add("empty-text", "<r/>", 1, Text(""), Exact);
    add("empty-cdata", "<r/>", 1, Cdata(""), Exact);
    add("empty-comment", "<r/>", 1, Comment(""), Exact);
    add(
        "empty-pi",
        "<r/>",
        1,
        Pi("p", ""),
        Semantic("empty PI whitespace spelling", b"<r><?p ?></r>"),
    );
    add("text-coalescing", "<r>a</r>", 1, Text("b"), TextRuns);
    add("empty-text-coalescing", "<r>a</r>", 1, Text(""), TextRuns);
    add(
        "text-cdata-boundary",
        "<r><![CDATA[a]]></r>",
        1,
        Text("b"),
        Exact,
    );
    add(
        "cdata-cdata-boundary",
        "<r><![CDATA[a]]></r>",
        1,
        Cdata("b"),
        Exact,
    );
    add(
        "append-before-epilog",
        "<r><c/></r><!--post-->",
        1,
        Comment("inside"),
        Exact,
    );
    add("text-escaping", "<r/>", 1, Text("\r&<>🙂"), Exact);
    add("remove-first", "<r><a/><b/><c/></r>", 2, Remove, Exact);
    add("remove-middle", "<r><a/><b/><c/></r>", 3, Remove, Exact);
    add("remove-last", "<r><a/><b/><c/></r>", 4, Remove, Exact);
    add("remove-sole", "<r><a/></r>", 2, Remove, Exact);
    add("remove-text-separator", "<r>a<c/>b</r>", 3, Remove, Exact);
    add(
        "remove-namespace-subtree",
        "<r xmlns:p='urn:p'><p:a xmlns:q='urn:q' q:a='v'>x<q:c/></p:a><p:b a=''/></r>",
        2,
        Remove,
        Exact,
    );
    add(
        "remove-prolog",
        "<!--pre--><r/><!--post-->",
        1,
        Remove,
        Exact,
    );
    add("remove-epilog", "<r/><?post x?>", 2, Remove, Exact);
    add(
        "root-unlink",
        "<!--pre--><r><c/></r><!--post-->",
        2,
        Remove,
        Rejected(EditErrorKind::ProtectedNode, None, "root-element unlink"),
    );
    add(
        "text-parent",
        "<r>a</r>",
        2,
        Text("b"),
        Rejected(
            EditErrorKind::WrongNodeKind,
            None,
            "Text parent consumes the fresh node",
        ),
    );
    add(
        "document-text",
        "<r/>",
        0,
        Text("x"),
        Rejected(
            EditErrorKind::WrongNodeKind,
            None,
            "Text outside the document element",
        ),
    );
    add(
        "invalid-comment",
        "<r/>",
        1,
        Comment("a--b"),
        Rejected(
            EditErrorKind::InvalidValue,
            Some(1),
            "permissive comment construction",
        ),
    );
    add(
        "reserved-pi",
        "<r/>",
        1,
        Pi("xml", "data"),
        Rejected(
            EditErrorKind::InvalidName,
            Some(0),
            "permissive PI target construction",
        ),
    );
    add(
        "pi-leading-space",
        "<r/>",
        1,
        Pi("p", " data"),
        Rejected(
            EditErrorKind::InvalidValue,
            Some(0),
            "raw PI data normalization profile",
        ),
    );
    add(
        "ownership-oom",
        "<r/>",
        1,
        Text(""),
        Skipped("pointer ownership, foreign/stale handles, quotas and OOM partial updates"),
    );
    add(
        "namespace-moves",
        "<r/>",
        1,
        Text(""),
        Skipped("namespace adoption, moves and repair"),
    );
    for (name, encoding) in [("utf16le-append", 1), ("utf16be-append", 2)] {
        cases.push(Case {
            name,
            input: "<r>é</r>",
            target: 1,
            action: Cdata("🙂"),
            policy: Exact,
            encoding,
        });
    }
    cases
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPLY: &[u8] = b"YAXML-STRUCTURE-1\n2\nD\n0\n0:\n0:\n0:\n0\nE\n1\n1:r\n0:\n0:\n0\n4:<r/>\n0\n3\nD\n0\n0:\n0:\n0:\n0\nE\n1\n1:r\n0:\n0:\n0\nT\n2\n0:\n0:\n1:x\n0\n8:<r>x</r>\n";

    fn case() -> Case {
        cases()
            .into_iter()
            .find(|case| case.name == "append-text")
            .unwrap()
    }

    #[test]
    fn literal_request_and_response_anchor_the_wire_protocol() {
        let case = case();
        assert_eq!(
            case.request(),
            b"YAXML-STRUCTURE-1\n1\n1\n4:<r/>\n0:\n1:x\n"
        );
        assert_eq!(case.expected().unwrap().verify(REPLY), Ok(Outcome::Exact));
        for length in 0..REPLY.len() {
            assert_eq!(
                case.expected().unwrap().verify(&REPLY[..length]),
                Err(CheckError::Protocol)
            );
        }
    }

    #[test]
    fn mismatches_and_malformed_or_excess_output_never_agree() {
        let expected = case().expected().unwrap();
        let text = std::str::from_utf8(REPLY).unwrap();
        for (from, to, result) in [
            ("YAXML-STRUCTURE-1", "OTHER", CheckError::Protocol),
            ("2\nD\n", "257\nD\n", CheckError::Protocol),
            ("E\n1\n", "E\n2\n", CheckError::Protocol),
            ("T\n2\n", "X\n2\n", CheckError::Protocol),
            (
                "1:r",
                "1:s",
                CheckError::Mismatch("initial kinds, order, names or values"),
            ),
            (
                "4:<r/>",
                "4:<s/>",
                CheckError::Mismatch("initial serialized bytes"),
            ),
            (
                "1:x",
                "1:y",
                CheckError::Mismatch("result kinds, order, names or values"),
            ),
            (
                "8:<r>x</r>",
                "8:<r>y</r>",
                CheckError::Mismatch("result serialized bytes"),
            ),
            (
                "\n0\n3\nD",
                "\n1\n3\nD",
                CheckError::Mismatch("returned survivor classification"),
            ),
        ] {
            assert_eq!(
                expected.verify(text.replacen(from, to, 1).as_bytes()),
                Err(result)
            );
        }
        let mut trailing = REPLY.to_vec();
        trailing.push(b'x');
        assert_eq!(expected.verify(&trailing), Err(CheckError::Protocol));
        assert_eq!(
            expected.verify(&vec![0; MAX_OUTPUT + 1]),
            Err(CheckError::Protocol)
        );
    }

    #[test]
    fn all_catalog_cases_have_explicit_rust_contracts_and_bounded_requests() {
        let mut names = std::collections::HashSet::new();
        let mut outcomes = [0; 4];
        for case in cases() {
            assert!(names.insert(case.name));
            assert!(case.request().len() < 72 * 1024);
            if let Some(Outcome::Skipped(_)) = case.skipped() {
                outcomes[3] += 1;
                continue;
            }
            let expected = case
                .expected()
                .unwrap_or_else(|error| panic!("{}: {error}", case.name));
            assert!(!expected.before.0.is_empty());
            assert!(!expected.after.0.is_empty());
            match expected.outcome {
                Outcome::Exact => outcomes[0] += 1,
                Outcome::Projected(_) => outcomes[1] += 1,
                Outcome::Difference(_) => outcomes[2] += 1,
                Outcome::Skipped(_) => unreachable!(),
            }
        }
        assert_eq!(outcomes, [23, 3, 6, 2]);
    }

    #[test]
    fn version_requires_both_compile_and_runtime_identity_and_features() {
        for (report, exploratory, accepted) in [
            (
                "YAXML-STRUCTURE-1 compiled=21504 runtime=21504 tree=1 output=1\n",
                false,
                true,
            ),
            (
                "YAXML-STRUCTURE-1 compiled=21503 runtime=21503 tree=1 output=1\n",
                false,
                false,
            ),
            (
                "YAXML-STRUCTURE-1 compiled=21503 runtime=21503 tree=1 output=1\n",
                true,
                true,
            ),
            (
                "YAXML-STRUCTURE-1 compiled=21504 runtime=21503 tree=1 output=1\n",
                false,
                false,
            ),
            (
                "YAXML-STRUCTURE-1 compiled=21503 runtime=21504 tree=1 output=1\n",
                false,
                false,
            ),
            (
                "YAXML-STRUCTURE-1 compiled=21504 runtime=21504 tree=0 output=1\n",
                true,
                false,
            ),
            (
                "YAXML-STRUCTURE-1 compiled=21504 runtime=21504 tree=1 output=0\n",
                true,
                false,
            ),
            (
                "YAXML-STRUCTURE-1 compiled=x runtime=21504 tree=1 output=1\n",
                true,
                false,
            ),
            (
                "YAXML-STRUCTURE-2 compiled=21504 runtime=21504 tree=1 output=1\n",
                true,
                false,
            ),
            ("YAXML-STRUCTURE-1", true, false),
        ] {
            assert_eq!(
                validate_version(report.as_bytes(), exploratory).is_ok(),
                accepted,
                "{report}"
            );
        }
        assert!(validate_version(b"\xff", true).is_err());
    }

    #[test]
    fn framing_preserves_utf8_newlines_cr_tabs_colons_and_raw_bytes() {
        let value = "é\n\r\t:🙂";
        let mut bytes = Vec::new();
        frame(&mut bytes, value.as_bytes());
        assert_eq!(bytes, "10:é\n\r\t:🙂\n".as_bytes());
        let mut input = Input(&bytes);
        assert_eq!(input.frame().unwrap(), value.as_bytes());
        assert!(input.0.is_empty());
        assert_eq!(Input(b"2:\0\xff\n").frame().unwrap(), b"\0\xff");
        for bytes in [
            b":\n".as_slice(),
            b"x:a\n",
            b"0:x",
            b"2:x\n",
            b"999999999999999999999:\n",
        ] {
            assert!(Input(bytes).frame().is_err());
        }
    }

    fn snapshot_bytes(output: &mut Vec<u8>, snapshot: &Snapshot, bytes: &[u8]) {
        number(output, snapshot.0.len());
        for node in &snapshot.0 {
            output.extend_from_slice(&[node.kind, b'\n']);
            number(output, node.parent.map_or(0, |parent| parent + 1));
            for value in [&node.name, &node.namespace, &node.value] {
                frame(output, value);
            }
            number(output, node.attributes.len());
            for attribute in &node.attributes {
                for value in [&attribute.name, &attribute.namespace, &attribute.value] {
                    frame(output, value);
                }
            }
        }
        frame(output, bytes);
    }

    #[test]
    fn ordered_attributes_namespaces_and_normalized_values_are_compared() {
        let case = Case {
            name: "attributes",
            input: "<r xmlns:p='urn:p' a='x&#9;&#13;' p:a='é'/>",
            target: 1,
            action: Action::Comment("x"),
            policy: Policy::Exact,
            encoding: 0,
        };
        let expected = case.expected().unwrap();
        let plain = |kind, parent, name: &[u8], value: &[u8]| Node {
            kind,
            parent,
            name: name.to_vec(),
            namespace: Vec::new(),
            value: value.to_vec(),
            attributes: Vec::new(),
        };
        let mut before = Snapshot(vec![
            plain(b'D', None, b"", b""),
            plain(b'E', Some(0), b"r", b""),
        ]);
        before.0[1].attributes = vec![
            Attribute {
                name: b"a".to_vec(),
                namespace: Vec::new(),
                value: b"x\t\r".to_vec(),
            },
            Attribute {
                name: b"p:a".to_vec(),
                namespace: b"urn:p".to_vec(),
                value: "é".as_bytes().to_vec(),
            },
        ];
        let mut after = before.clone();
        after.0.push(plain(b'M', Some(1), b"", b"x"));
        let response = |before: &Snapshot| {
            let mut output = format!("{PROTOCOL}\n").into_bytes();
            snapshot_bytes(
                &mut output,
                before,
                "<r xmlns:p=\"urn:p\" a=\"x&#9;&#13;\" p:a=\"é\"/>".as_bytes(),
            );
            number(&mut output, 0);
            snapshot_bytes(
                &mut output,
                &after,
                "<r xmlns:p=\"urn:p\" a=\"x&#9;&#13;\" p:a=\"é\"><!--x--></r>".as_bytes(),
            );
            output
        };
        assert_eq!(expected.verify(&response(&before)), Ok(Outcome::Exact));
        for variant in 0..4 {
            let mut wrong = before.clone();
            match variant {
                0 => wrong.0[1].attributes.swap(0, 1),
                1 => wrong.0[1].attributes[1].namespace = b"urn:other".to_vec(),
                2 => wrong.0[1].attributes[1].name = b"q:a".to_vec(),
                _ => wrong.0[1].attributes[0].value = b"x\t".to_vec(),
            }
            assert_eq!(
                expected.verify(&response(&wrong)),
                Err(CheckError::Mismatch(
                    "initial kinds, order, names or values"
                ))
            );
        }
    }
}
