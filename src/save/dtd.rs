//! `xmlDtdDumpOutput`: the DOCTYPE with its internal subset regenerated from
//! the declarations, in libxml2's canonical spelling. The subset is parsed
//! again, expanding internal parameter entities; libxml2 keeps the first of
//! duplicate declarations, writes notations first (in its hash order, here
//! declaration order), one `<!ATTLIST>` per attribute, and drops a subset
//! that declares nothing.

use std::collections::HashSet;

use super::Out;
use crate::dtd::{
    AttributeKind, Connector, DefaultDeclarationView, DtdLimits, DtdUnitView, ElementContentView,
    EntityDefinitionView, ExternalIdView, ModelTokenView, Occurrence,
};
use crate::dtd_catalog::DtdCatalog;
use crate::dtd_controller::{DtdController, DtdEvent};
use crate::parser::Budget;
use crate::{DecodeLimits, Doctype, Limits};

pub(super) fn doctype(out: &mut Out<'_>, doctype: &Doctype<'_>) {
    out.raw("<!DOCTYPE ");
    out.raw(doctype.name);
    match (doctype.public_id, doctype.system_id) {
        (Some(public), system) => {
            out.raw(" PUBLIC ");
            out.quoted(public, false);
            out.raw(" ");
            out.quoted(system.unwrap_or_default(), false);
        }
        (None, Some(system)) => {
            out.raw(" SYSTEM ");
            out.quoted(system, false);
        }
        (None, None) => {}
    }
    let Some(subset) = doctype.internal_subset else {
        return out.raw(">");
    };
    let (mut notations, mut children) = (out.sibling(), out.sibling());
    match declarations(doctype.name, subset, &mut notations, &mut children) {
        Some(true) => {
            out.raw(" [\n");
            out.append(notations);
            out.append(children);
            out.raw("]>");
        }
        Some(false) => out.raw(">"),
        // A subset this standalone pass cannot read is kept as written.
        None => {
            out.raw(" [");
            out.raw(subset);
            out.raw("]>");
        }
    }
}

/// A mixed or children content model as `xmlBufDumpElementContent` prints it.
pub(crate) fn content_model<'a>(tokens: impl Iterator<Item = ModelTokenView<'a>>) -> String {
    let mut out = Out::new(crate::encoding::Output::Utf8, false, None);
    Model::parse(tokens).write(&mut out);
    String::from_utf8(out.bytes).unwrap_or_default()
}

/// Writes the subset's declarations; true when it declared anything.
fn declarations(
    name: &str,
    subset: &str,
    notations: &mut Out<'_>,
    out: &mut Out<'_>,
) -> Option<bool> {
    // The subset is source text; parsing normalizes its line ends.
    let subset = subset.replace("\r\n", "\n").replace('\r', "\n");
    let text = format!("<!DOCTYPE {name} [{subset}]>");
    let defaults = Limits::default();
    let limits = Limits {
        max_dtd_bytes: defaults.max_dtd_bytes.max(text.len()),
        ..defaults
    };
    let options = DtdLimits::new(limits, DecodeLimits::default());
    let mut controller = DtdController::stable_utf8(&text, 0, 0, options, false);
    // A recovered document's subset can hold what libxml2 reads on from.
    let mut budget = Budget::new(limits).recovering();
    let (mut seen, mut declared) = (HashSet::new(), false);
    while !controller.is_complete() {
        match controller.current() {
            Some((DtdEvent::Item(unit), _)) => {
                let view = unit.view();
                declared |= declaration(view, controller.catalog(), &mut seen, notations, out);
            }
            Some(_) => {}
            None => {
                controller.advance_borrowed(&mut budget).ok()?;
                continue;
            }
        }
        controller.release();
    }
    Some(declared)
}

/// Writes one unit unless it repeats a declaration; true for declarations.
fn declaration(
    view: DtdUnitView<'_>,
    catalog: &DtdCatalog,
    seen: &mut HashSet<String>,
    notations: &mut Out<'_>,
    out: &mut Out<'_>,
) -> bool {
    let mut first = |kind: &str, name: &str| seen.insert(format!("{kind} {name}"));
    match view {
        DtdUnitView::Element(element) => {
            if first("element", element.name()) {
                out.raw("<!ELEMENT ");
                out.raw(element.name());
                out.raw(" ");
                match element.content() {
                    ElementContentView::Empty => out.raw("EMPTY"),
                    ElementContentView::Any => out.raw("ANY"),
                    ElementContentView::Mixed(tokens) | ElementContentView::Children(tokens) => {
                        Model::parse(tokens).write(out)
                    }
                }
                out.raw(">\n");
            }
        }
        DtdUnitView::AttributeList(list) => {
            let element = list.element_name();
            for definition in list.definitions() {
                if !first("attribute", &format!("{element} {}", definition.name())) {
                    continue;
                }
                out.raw("<!ATTLIST ");
                out.raw(element);
                out.raw(" ");
                out.raw(definition.name());
                out.raw(match definition.kind() {
                    AttributeKind::Cdata => " CDATA",
                    AttributeKind::Id => " ID",
                    AttributeKind::IdRef => " IDREF",
                    AttributeKind::IdRefs => " IDREFS",
                    AttributeKind::Entity => " ENTITY",
                    AttributeKind::Entities => " ENTITIES",
                    AttributeKind::NmToken => " NMTOKEN",
                    AttributeKind::NmTokens => " NMTOKENS",
                    AttributeKind::Enumeration => " (",
                    AttributeKind::Notation => " NOTATION (",
                });
                if matches!(
                    definition.kind(),
                    AttributeKind::Enumeration | AttributeKind::Notation
                ) {
                    for (index, value) in definition.values().enumerate() {
                        if index > 0 {
                            out.raw(" | ");
                        }
                        out.raw(value);
                    }
                    out.raw(")");
                }
                let (keyword, literal) = match definition.default() {
                    DefaultDeclarationView::Required => (" #REQUIRED", None),
                    DefaultDeclarationView::Implied => (" #IMPLIED", None),
                    DefaultDeclarationView::Value { fixed, literal } => (
                        if fixed { " #FIXED" } else { "" },
                        Some(literal.content().0),
                    ),
                };
                out.raw(keyword);
                if let Some(literal) = literal {
                    // The stored default: references expanded, then normalized.
                    let value = catalog
                        .attributes(element)
                        .find(|binding| binding.name == definition.name())
                        .and_then(|binding| binding.default.value())
                        .unwrap_or(literal);
                    out.raw(" \"");
                    out.text(value, true);
                    out.raw("\"");
                }
                out.raw(">\n");
            }
            // An empty list creates no attribute table.
            return list.definitions().next().is_some();
        }
        DtdUnitView::Entity(entity) => {
            let parameter = entity.is_parameter();
            if first(if parameter { "%" } else { "&" }, entity.name()) {
                out.raw(if parameter {
                    "<!ENTITY % "
                } else {
                    "<!ENTITY "
                });
                out.raw(entity.name());
                out.raw(" ");
                match entity.definition() {
                    EntityDefinitionView::Internal(literal) => {
                        out.quoted(literal.content().0, true)
                    }
                    EntityDefinitionView::External {
                        identifier,
                        notation,
                    } => {
                        external(out, identifier);
                        if let Some(notation) = notation {
                            out.raw(" NDATA ");
                            out.raw(notation);
                        }
                    }
                }
                out.raw(">\n");
            }
        }
        DtdUnitView::Notation(notation) => {
            if first("notation", notation.name()) {
                notations.raw("<!NOTATION ");
                notations.raw(notation.name());
                notations.raw(" ");
                external(notations, notation.identifier());
                notations.raw(" >\n");
            }
        }
        DtdUnitView::Comment(text) => {
            out.raw("<!--");
            out.raw(text);
            out.raw("-->");
            return false;
        }
        DtdUnitView::ProcessingInstruction(instruction) => {
            out.raw("<?");
            out.raw(instruction.target);
            if let Some(data) = instruction.data.filter(|data| !data.is_empty()) {
                out.raw(" ");
                out.raw(data);
            }
            out.raw("?>");
            return false;
        }
        DtdUnitView::Header(_) => return false,
    }
    true
}

/// `PUBLIC "p" "s"` or `SYSTEM "s"`; only notations may lack the system literal.
fn external(out: &mut Out<'_>, identifier: ExternalIdView<'_>) {
    match identifier.public {
        Some(public) => {
            out.raw("PUBLIC ");
            out.quoted(unquote(public), false);
            if let Some(system) = identifier.system {
                out.raw(" ");
                out.quoted(unquote(system), false);
            }
        }
        None => {
            out.raw("SYSTEM ");
            out.quoted(identifier.system.map_or("", unquote), false);
        }
    }
}

/// A literal's content without its quotes.
fn unquote(literal: &str) -> &str {
    literal
        .get(1..literal.len().saturating_sub(1))
        .unwrap_or_default()
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Kind<'a> {
    Pcdata,
    Name(&'a str),
    Group(Connector),
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Occur {
    Once,
    Optional,
    ZeroOrMore,
    OneOrMore,
}

struct Particle<'a> {
    kind: Kind<'a>,
    occur: Occur,
    children: [usize; 2],
    parent: usize,
}

/// libxml2's `xmlElementContent` tree: groups are right-leaning binary
/// operator chains, built as `xmlParseElementChildrenContentDeclPriv` does,
/// including its occurrence normalizations (`(a | b?)+` is `(a | b)*`).
struct Model<'a> {
    particles: Vec<Particle<'a>>,
    root: usize,
}

/// One open group: libxml2's `ret`, `cur` and `last`.
#[derive(Default)]
struct Frame {
    first: Option<usize>,
    current: Option<usize>,
    last: Option<usize>,
}

const NONE: usize = usize::MAX;

impl<'a> Model<'a> {
    fn parse(tokens: impl Iterator<Item = ModelTokenView<'a>>) -> Self {
        let mut model = Self {
            particles: Vec::new(),
            root: NONE,
        };
        let mut frames: Vec<Frame> = Vec::new();
        // The particle an occurrence indicator applies to, with the closed
        // group's `cur` when it is a group.
        let mut previous = (NONE, None);
        for token in tokens {
            match token {
                ModelTokenView::Open => frames.push(Frame::default()),
                ModelTokenView::Pcdata | ModelTokenView::Name(_) => {
                    let kind = match token {
                        ModelTokenView::Name(name) => Kind::Name(name),
                        _ => Kind::Pcdata,
                    };
                    let item = model.add(kind);
                    model.item(frames.last_mut(), item);
                    previous = (item, None);
                }
                ModelTokenView::Connector(connector) => {
                    let Some(frame) = frames.last_mut() else {
                        continue;
                    };
                    let operator = model.add(Kind::Group(connector));
                    match frame.last.take() {
                        None => {
                            model.link(operator, 0, frame.first.unwrap_or(NONE));
                            frame.first = Some(operator);
                        }
                        Some(last) => {
                            model.link(frame.current.unwrap_or(NONE), 1, operator);
                            model.link(operator, 0, last);
                        }
                    }
                    frame.current = Some(operator);
                }
                ModelTokenView::Close => {
                    let Some(frame) = frames.pop() else {
                        continue;
                    };
                    if let (Some(current), Some(last)) = (frame.current, frame.last) {
                        model.link(current, 1, last);
                    }
                    let group = frame.first.unwrap_or(NONE);
                    model.item(frames.last_mut(), group);
                    previous = (group, frame.current);
                }
                ModelTokenView::Occurrence(occurrence) => model.occur(previous, occurrence),
            }
        }
        model
    }

    fn add(&mut self, kind: Kind<'a>) -> usize {
        self.particles.push(Particle {
            kind,
            occur: Occur::Once,
            children: [NONE; 2],
            parent: NONE,
        });
        self.particles.len() - 1
    }

    fn link(&mut self, parent: usize, side: usize, child: usize) {
        if parent != NONE && child != NONE {
            self.particles[parent].children[side] = child;
            self.particles[child].parent = parent;
        }
    }

    /// Adds a name or closed group to the enclosing group, or makes it the root.
    fn item(&mut self, frame: Option<&mut Frame>, item: usize) {
        match frame {
            Some(frame) if frame.first.is_none() => {
                frame.first = Some(item);
                frame.current = Some(item);
            }
            Some(frame) => frame.last = Some(item),
            None => self.root = item,
        }
    }

    fn occur(&mut self, (item, current): (usize, Option<usize>), occurrence: Occurrence) {
        if item == NONE {
            return;
        }
        let repeated = matches!(
            self.particles[item].occur,
            Occur::Optional | Occur::ZeroOrMore
        );
        let Some(current) = current else {
            // A name takes its indicator as written.
            self.particles[item].occur = match occurrence {
                Occurrence::Optional => Occur::Optional,
                Occurrence::ZeroOrMore => Occur::ZeroOrMore,
                Occurrence::OneOrMore => Occur::OneOrMore,
            };
            return;
        };
        match occurrence {
            Occurrence::Optional => {
                let plural = matches!(
                    self.particles[item].occur,
                    Occur::OneOrMore | Occur::ZeroOrMore
                );
                self.particles[item].occur = if plural {
                    Occur::ZeroOrMore
                } else {
                    Occur::Optional
                };
            }
            Occurrence::ZeroOrMore => {
                self.particles[item].occur = Occur::ZeroOrMore;
                self.clear_choices(item);
            }
            Occurrence::OneOrMore => {
                // libxml2 starts this walk at the group's last operator.
                let found = self.clear_choices(current);
                self.particles[item].occur = if repeated || found {
                    Occur::ZeroOrMore
                } else {
                    Occur::OneOrMore
                };
            }
        }
    }

    /// Makes optional or repeated members of a choice chain occur once;
    /// true when one did.
    fn clear_choices(&mut self, mut node: usize) -> bool {
        let mut found = false;
        while node != NONE && self.particles[node].kind == Kind::Group(Connector::Choice) {
            for child in self.particles[node].children {
                if child != NONE
                    && matches!(
                        self.particles[child].occur,
                        Occur::Optional | Occur::ZeroOrMore
                    )
                {
                    self.particles[child].occur = Occur::Once;
                    found = true;
                }
            }
            node = self.particles[node].children[1];
        }
        found
    }

    /// `xmlBufDumpElementContent`, iterative like libxml2's.
    fn write(&self, out: &mut Out<'_>) {
        let occur = |out: &mut Out<'_>, particle: &Particle<'_>| {
            out.raw(match particle.occur {
                Occur::Once => "",
                Occur::Optional => "?",
                Occur::ZeroOrMore => "*",
                Occur::OneOrMore => "+",
            })
        };
        // A group needs parentheses unless it continues its parent's chain.
        let nested = |node: usize| {
            let particle = &self.particles[node];
            let parent = &self.particles[particle.parent];
            matches!(particle.kind, Kind::Group(_))
                && (particle.kind != parent.kind || particle.occur != Occur::Once)
        };
        out.raw("(");
        let root = self.root;
        if root == NONE {
            return out.raw(")");
        }
        let mut node = root;
        loop {
            let particle = &self.particles[node];
            match particle.kind {
                Kind::Pcdata => out.raw("#PCDATA"),
                Kind::Name(name) => out.raw(name),
                Kind::Group(_) => {
                    if node != root && particle.parent != NONE && nested(node) {
                        out.raw("(");
                    }
                    node = particle.children[0];
                    if node == NONE {
                        break;
                    }
                    continue;
                }
            }
            while node != root {
                let particle = &self.particles[node];
                let parent = particle.parent;
                if parent == NONE {
                    return;
                }
                if nested(node) {
                    out.raw(")");
                }
                occur(out, particle);
                if node == self.particles[parent].children[0] {
                    out.raw(match self.particles[parent].kind {
                        Kind::Group(Connector::Sequence) => " , ",
                        _ => " | ",
                    });
                    node = self.particles[parent].children[1];
                    break;
                }
                node = parent;
            }
            if node == root || node == NONE {
                break;
            }
        }
        out.raw(")");
        occur(out, &self.particles[root]);
    }
}
