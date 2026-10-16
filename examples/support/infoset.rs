//! Independent libxml2 infoset projection through bounded XPath CLI queries.
//!
//! Run the returned query with `xmllint --nocdata --strict-namespace
//! --nocatalogs --xpath QUERY -`. Compare stdout directly with `expected()`;
//! oracle values never pass through yaxml's parser.
//! UTF-8 borrows Reader events; UTF-16 borrows an owned tree's logical views.
//! Supply the original encoded bytes to the external oracle in either case.

use std::borrow::Cow;
use std::fmt;

use yaxml::{Attribute, Error, Event, Name, OwnedDocument, OwnedNodeKind, Reader};

pub const LIBXML2_VERSION: &str = "2.15.4";
pub const LIBXML2_COMPILED_VERSION: &str = "21504";
// Stay below Linux's per-argument size limit as the query is one argv value.
pub const MAX_QUERY_BYTES: usize = 64 * 1024;
pub const MAX_NODES: usize = 256;
pub const MAX_ATTRIBUTES: usize = 4096;
pub const MAX_EXPECTED_BYTES: usize = 8 * 1024 * 1024;

const XMLNS_NAMESPACE: &str = "http://www.w3.org/2000/xmlns/";
// libxml2 retains some empty CDATA sections as empty text with --nocdata.
const NODES: &str = "/descendant::node()[not(self::text()[string-length(.)=0])]";

#[derive(Debug)]
pub enum ProjectionError {
    Xml(Error),
    Limit(&'static str),
    Allocation,
}

impl fmt::Display for ProjectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Xml(error) => write!(formatter, "cannot project XML: {error}"),
            Self::Limit(name) => write!(formatter, "infoset projection exceeds {name} ceiling"),
            Self::Allocation => formatter.write_str("cannot allocate infoset projection"),
        }
    }
}

impl std::error::Error for ProjectionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Xml(error) => Some(error),
            Self::Limit(_) | Self::Allocation => None,
        }
    }
}

impl From<Error> for ProjectionError {
    fn from(error: Error) -> Self {
        Self::Xml(error)
    }
}

#[derive(Debug)]
pub struct Projection {
    query: String,
    expected: String,
}

impl Projection {
    pub fn from_input(input: &[u8]) -> Result<Self, ProjectionError> {
        // The borrowing interfaces deliberately remain UTF-8-only. Own logical
        // values for the auto-decoding surface without retaining decoded input.
        if input.starts_with(&[0xff, 0xfe])
            || input.starts_with(&[0xfe, 0xff])
            || input.starts_with(&[0, b'<', 0, b'?'])
            || input.starts_with(&[b'<', 0, b'?', 0])
        {
            let document = OwnedDocument::parse_bytes(input)?;
            Self::from_nodes(owned_nodes(&document)?)
        } else {
            Self::from_nodes(nodes(input)?)
        }
    }

    fn from_nodes(nodes: Vec<ProjectedNode<'_>>) -> Result<Self, ProjectionError> {
        let mut builder = Builder {
            query: String::from("concat("),
            expected: String::new(),
            first: true,
        };
        builder.field(&format!("count({NODES})"), &nodes.len().to_string())?;
        for (index, node) in nodes.iter().enumerate() {
            // Apply the position on the forward descendant axis. Parenthesized
            // node-set filtering triggers libxml2's separate sorting path,
            // which can reorder comments around the document element.
            let path = format!("{NODES}[{}]", index + 1);
            let kind = format!(
                "count({path}/self::*)+2*count({path}/self::text())+3*count({path}/self::comment())+4*count({path}/self::processing-instruction())"
            );
            builder.field(&kind, node.kind)?;
            builder.field(
                &format!("count({path}/ancestor::node())"),
                &node.depth.to_string(),
            )?;
            builder.name(&path, &node.name)?;
            let value_expression = if node.kind == "1" {
                "''".to_owned()
            } else {
                format!("string({path})")
            };
            builder.field(&value_expression, &node.value)?;
            builder.field(
                &format!("count({path}/@*)"),
                &node.attributes.len().to_string(),
            )?;
            for (index, attribute) in node.attributes.iter().enumerate() {
                let path = format!("{path}/@*[{}]", index + 1);
                builder.name(&path, &attribute.name)?;
                builder.field(&format!("string({path})"), &attribute.value)?;
            }
        }
        append(&mut builder.query, ")", MAX_QUERY_BYTES, "query bytes")?;
        append(
            &mut builder.expected,
            "\n",
            MAX_EXPECTED_BYTES,
            "expected output bytes",
        )?;
        Ok(Self {
            query: builder.query,
            expected: builder.expected,
        })
    }

    /// Returns a bounded query containing only generated integer positions.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Expected complete stdout, including xmllint's single final LF.
    pub fn expected(&self) -> &[u8] {
        self.expected.as_bytes()
    }
}

struct ProjectedNode<'input> {
    depth: usize,
    name: Name<'input>,
    value: Cow<'input, str>,
    attributes: Vec<Attribute<'input>>,
    kind: &'static str,
}

fn nodes(input: &[u8]) -> Result<Vec<ProjectedNode<'_>>, ProjectionError> {
    let mut reader = Reader::new(input)?;
    let mut nodes: Vec<ProjectedNode<'_>> = Vec::new();
    let mut depth = 0;
    let mut attributes = 0usize;
    while let Some(event) = reader.read_event()? {
        let starts = matches!(&event, Event::Start(_));
        let mut node = ProjectedNode {
            depth: depth + 1,
            name: Name {
                qualified: "",
                prefix: None,
                local: "",
                namespace: None,
            },
            value: Cow::Borrowed(""),
            attributes: Vec::new(),
            kind: "1",
        };
        match event {
            Event::Declaration(_) => continue,
            Event::End(_) => {
                depth -= 1;
                continue;
            }
            Event::Start(element) | Event::Empty(element) => {
                node.name = element.name;
                node.attributes = element.attributes;
                node.attributes.retain(|attribute| {
                    attribute.name.namespace.as_deref() != Some(XMLNS_NAMESPACE)
                });
                attributes = attributes
                    .checked_add(node.attributes.len())
                    .ok_or(ProjectionError::Limit("attribute count"))?;
                if attributes > MAX_ATTRIBUTES {
                    return Err(ProjectionError::Limit("attribute count"));
                }
                if starts {
                    depth += 1;
                }
            }
            Event::Text(value) | Event::Cdata(value) => {
                if value.is_empty() {
                    continue;
                }
                if let Some(previous) = nodes.last_mut()
                    && previous.kind == "2"
                    && previous.depth == node.depth
                {
                    merge_value(&mut previous.value, &value)?;
                    continue;
                }
                node.kind = "2";
                node.value = value;
            }
            Event::Comment(value) => {
                node.kind = "3";
                node.value = value;
            }
            Event::ProcessingInstruction { target, data } => {
                node.kind = "4";
                node.name.qualified = target;
                node.name.local = target;
                node.value = data;
            }
        }
        if nodes.len() == MAX_NODES {
            return Err(ProjectionError::Limit("node count"));
        }
        nodes
            .try_reserve(1)
            .map_err(|_| ProjectionError::Allocation)?;
        nodes.push(node);
    }
    Ok(nodes)
}

fn owned_nodes(document: &OwnedDocument) -> Result<Vec<ProjectedNode<'_>>, ProjectionError> {
    let mut nodes: Vec<ProjectedNode<'_>> = Vec::new();
    let mut attributes = 0usize;
    for source in document.nodes().skip(1) {
        let mut node = ProjectedNode {
            depth: std::iter::successors(source.parent(), |node| node.parent()).count(),
            name: source.name().unwrap_or(Name {
                qualified: "",
                prefix: None,
                local: "",
                namespace: None,
            }),
            value: Cow::Borrowed(source.text().unwrap_or("")),
            attributes: Vec::new(),
            kind: match source.kind() {
                OwnedNodeKind::Element => "1",
                OwnedNodeKind::Text | OwnedNodeKind::Cdata => "2",
                OwnedNodeKind::Comment => "3",
                OwnedNodeKind::ProcessingInstruction => "4",
                _ => return Err(ProjectionError::Limit("supported node kinds")),
            },
        };
        if let Some(target) = source.processing_instruction_target() {
            node.name.qualified = target;
            node.name.local = target;
        }
        for attribute in source.attributes() {
            if attribute.name.namespace.as_deref() == Some(XMLNS_NAMESPACE) {
                continue;
            }
            if attributes == MAX_ATTRIBUTES {
                return Err(ProjectionError::Limit("attribute count"));
            }
            attributes += 1;
            node.attributes
                .try_reserve(1)
                .map_err(|_| ProjectionError::Allocation)?;
            node.attributes.push(attribute);
        }
        if node.kind == "2" {
            if node.value.is_empty() {
                continue;
            }
            if let Some(previous) = nodes.last_mut()
                && previous.kind == "2"
                && previous.depth == node.depth
            {
                merge_value(&mut previous.value, &node.value)?;
                continue;
            }
        }
        if nodes.len() == MAX_NODES {
            return Err(ProjectionError::Limit("node count"));
        }
        nodes
            .try_reserve(1)
            .map_err(|_| ProjectionError::Allocation)?;
        nodes.push(node);
    }
    Ok(nodes)
}

fn merge_value(previous: &mut Cow<'_, str>, value: &str) -> Result<(), ProjectionError> {
    let mut merged = match std::mem::take(previous) {
        Cow::Borrowed(value) => {
            let mut owned = String::new();
            append(
                &mut owned,
                value,
                MAX_EXPECTED_BYTES,
                "expected output bytes",
            )?;
            owned
        }
        Cow::Owned(value) => value,
    };
    append(
        &mut merged,
        value,
        MAX_EXPECTED_BYTES,
        "expected output bytes",
    )?;
    *previous = Cow::Owned(merged);
    Ok(())
}

struct Builder {
    query: String,
    expected: String,
    first: bool,
}

impl Builder {
    fn name(&mut self, path: &str, name: &Name<'_>) -> Result<(), ProjectionError> {
        self.field(&format!("name({path})"), name.qualified)?;
        self.field(&format!("local-name({path})"), name.local)?;
        self.field(
            &format!("namespace-uri({path})"),
            name.namespace.as_deref().unwrap_or(""),
        )
    }

    fn field(&mut self, expression: &str, value: &str) -> Result<(), ProjectionError> {
        if !self.first {
            append(&mut self.query, ",", MAX_QUERY_BYTES, "query bytes")?;
        }
        self.first = false;
        for part in [
            "string-length(string(",
            expression,
            ")),':',string(",
            expression,
            ")",
        ] {
            append(&mut self.query, part, MAX_QUERY_BYTES, "query bytes")?;
        }
        for part in [value.chars().count().to_string().as_str(), ":", value] {
            append(
                &mut self.expected,
                part,
                MAX_EXPECTED_BYTES,
                "expected output bytes",
            )?;
        }
        Ok(())
    }
}

fn append(
    destination: &mut String,
    value: &str,
    maximum: usize,
    ceiling: &'static str,
) -> Result<(), ProjectionError> {
    if value.len() > maximum.saturating_sub(destination.len()) {
        return Err(ProjectionError::Limit(ceiling));
    }
    destination
        .try_reserve(value.len())
        .map_err(|_| ProjectionError::Allocation)?;
    destination.push_str(value);
    Ok(())
}
