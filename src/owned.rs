use std::borrow::Cow;
use std::num::NonZeroUsize;
use std::ops::Range;
use std::sync::atomic::{AtomicUsize, Ordering};

#[cfg(test)]
use std::cell::Cell;
#[cfg(test)]
use std::marker::PhantomData;
#[cfg(test)]
use std::rc::Rc;

use crate::allocator::{self, Allocator, Global};
use crate::options::Flags;
use crate::parser::Budget;
use crate::xpath::tree::{self, NamespaceBinding};
use crate::{
    Attribute, Declaration, Doctype, Error, ErrorKind, Event, Limit, Limits, Name, NodeId, Reader,
    SourceOrigin,
};
use crate::{DecodeLimits, EditError, EditErrorKind, EditLimit, ParseOptions, PushParser};
use crate::{EditComponent, EditLocation};

const XMLNS: &str = "http://www.w3.org/2000/xmlns/";
const XML: &str = "http://www.w3.org/XML/1998/namespace";

mod construction;
mod copy;
mod edit;

static LAST_DOCUMENT_ID: AtomicUsize = AtomicUsize::new(0);

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EditReservation {
    Scratch,
    Node,
    Attribute,
    Pool,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug)]
struct EditReservationFailure {
    reservation: EditReservation,
    remaining: usize,
    attempts: usize,
    fired: bool,
}

#[cfg(test)]
thread_local! {
    static EDIT_RESERVATION_FAILURE: Cell<Option<EditReservationFailure>> = const {
        Cell::new(None)
    };
}

/// A thread-local, one-shot failure scope for private allocation-path tests.
/// The `Rc` marker keeps the guard on the thread whose TLS entry it owns.
#[cfg(test)]
struct EditReservationFailureGuard {
    _not_send_or_sync: PhantomData<Rc<()>>,
}

#[cfg(test)]
impl EditReservationFailureGuard {
    fn arm(reservation: EditReservation, skip: usize) -> Self {
        EDIT_RESERVATION_FAILURE.with(|failure| {
            assert!(
                failure.get().is_none(),
                "edit reservation failure already armed"
            );
            failure.set(Some(EditReservationFailure {
                reservation,
                remaining: skip,
                attempts: 0,
                fired: false,
            }));
        });
        Self {
            _not_send_or_sync: PhantomData,
        }
    }

    fn attempts(&self) -> usize {
        EDIT_RESERVATION_FAILURE.with(|failure| {
            failure
                .get()
                .expect("edit reservation failure guard lost its state")
                .attempts
        })
    }

    fn fired(&self) -> bool {
        EDIT_RESERVATION_FAILURE.with(|failure| {
            failure
                .get()
                .expect("edit reservation failure guard lost its state")
                .fired
        })
    }
}

#[cfg(test)]
impl Drop for EditReservationFailureGuard {
    fn drop(&mut self) {
        EDIT_RESERVATION_FAILURE.with(|failure| {
            assert!(
                failure.replace(None).is_some(),
                "edit reservation failure guard lost its state"
            );
        });
    }
}

#[cfg(test)]
fn inject_edit_reservation_failure(reservation: EditReservation) -> Result<(), EditError> {
    EDIT_RESERVATION_FAILURE.with(|failure| {
        let Some(mut state) = failure.get() else {
            return Ok(());
        };
        if state.reservation != reservation || state.fired {
            return Ok(());
        }
        state.attempts = state.attempts.saturating_add(1);
        if state.remaining == 0 {
            state.fired = true;
            failure.set(Some(state));
            Err(EditError::new(EditErrorKind::Allocation, None))
        } else {
            state.remaining -= 1;
            failure.set(Some(state));
            Ok(())
        }
    })
}

fn document_identity(counter: &AtomicUsize) -> Result<NonZeroUsize, Error> {
    let previous = counter
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |last| {
            last.checked_add(1)
        })
        .map_err(|_| Error::new(ErrorKind::IdentifierExhausted, 0))?;
    // The successful checked increment guarantees previous < usize::MAX.
    Ok(NonZeroUsize::MIN.saturating_add(previous))
}

/// A checked owned-tree handle, tied to one document's non-recycled identity.
/// Moving a document preserves its handles; separately parsed documents have
/// different identities. This is not a stable address or a serialized ID.
/// Borrowing-DOM IDs cannot be used for owned lookup:
///
/// ```compile_fail
/// use yaxml::{Document, OwnedDocument};
/// let borrowed = Document::parse("<r/>").unwrap();
/// let owned = OwnedDocument::parse("<r/>").unwrap();
/// owned.node(borrowed.root().id());
/// ```
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct OwnedNodeId {
    document: NonZeroUsize,
    slot: NodeId,
}

impl OwnedNodeId {
    pub(crate) fn slot(self) -> usize {
        self.slot.0
    }
}

/// The content category of a node in an owned document.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum OwnedNodeKind {
    Document,
    DocumentType,
    Element,
    Text,
    Cdata,
    Comment,
    ProcessingInstruction,
    /// A kept general entity reference (`XML_ENTITY_REF_NODE`); its children
    /// are the replacement text's nodes.
    EntityRef,
}

#[derive(Clone, Debug)]
enum Content {
    Document,
    DocumentType(StoredDoctype),
    Element {
        name: StoredName,
        attributes: Range<usize>,
    },
    Text(Range<usize>),
    Cdata(Range<usize>),
    Comment(Range<usize>),
    ProcessingInstruction {
        target: Range<usize>,
        data: Range<usize>,
    },
    EntityRef(Range<usize>),
    Removed,
}

// usize::MAX is never a published slot. Keeping the sentinel private saves one
// word per link compared with Option<NodeId>, without narrowing public IDs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Link(usize);

impl Link {
    const NONE: Self = Self(usize::MAX);

    /// A slot index is below `isize::MAX`, so it never spells `NONE`.
    fn new(id: NodeId) -> Self {
        Self(id.0)
    }

    fn get(self) -> Option<NodeId> {
        (self != Self::NONE).then_some(NodeId(self.0))
    }
}

#[derive(Clone, Debug)]
struct OwnedRecord {
    // The stable slot follows the record into a view without a second arena
    // lookup. Records never move between slots or documents.
    id: NodeId,
    content: Content,
    source: Option<SourceOrigin>,
    parent: Link,
    first_child: Link,
    last_child: Link,
    previous_sibling: Link,
    next_sibling: Link,
    subtree_size: usize,
}

impl OwnedRecord {
    fn root(source_length: usize) -> Self {
        Self {
            id: NodeId(0),
            content: Content::Document,
            source: Some(SourceOrigin::input(0..source_length)),
            parent: Link::NONE,
            first_child: Link::NONE,
            last_child: Link::NONE,
            previous_sibling: Link::NONE,
            next_sibling: Link::NONE,
            subtree_size: 1,
        }
    }

    /// A created node without links or source provenance.
    fn detached(id: NodeId, content: Content) -> Self {
        Self {
            id,
            content,
            source: None,
            parent: Link::NONE,
            first_child: Link::NONE,
            last_child: Link::NONE,
            previous_sibling: Link::NONE,
            next_sibling: Link::NONE,
            subtree_size: 1,
        }
    }
}

#[derive(Clone, Debug)]
struct StoredName {
    qualified: Range<usize>,
    prefix_length: Option<NonZeroUsize>,
    namespace: Option<Range<usize>>,
}

impl StoredName {
    #[inline]
    fn view<'pool>(&self, pool: &'pool str) -> Name<'pool> {
        let qualified = &pool[self.qualified.clone()];
        Name::from_validated_parts(
            Cow::Borrowed(qualified),
            self.prefix_length,
            self.namespace
                .as_ref()
                .map(|range| Cow::Borrowed(&pool[range.clone()])),
        )
    }
}

#[derive(Clone, Debug)]
struct StoredAttribute {
    name: StoredName,
    value: Range<usize>,
}

impl StoredAttribute {
    fn view<'pool>(&self, pool: &'pool str) -> Attribute<'pool> {
        Attribute {
            name: self.name.view(pool),
            value: Cow::Borrowed(&pool[self.value.clone()]),
        }
    }
}

#[derive(Clone, Debug)]
struct StoredDeclaration {
    version: Range<usize>,
    encoding: Option<Range<usize>>,
    standalone: Option<bool>,
}

impl StoredDeclaration {
    fn view<'pool>(&self, pool: &'pool str) -> Declaration<'pool> {
        Declaration {
            version: &pool[self.version.clone()],
            encoding: self.encoding.as_ref().map(|range| &pool[range.clone()]),
            standalone: self.standalone,
        }
    }
}

#[derive(Clone, Debug)]
struct StoredDoctype {
    name: Range<usize>,
    public_id: Option<Range<usize>>,
    system_id: Option<Range<usize>>,
    internal_subset: Option<Range<usize>>,
}

impl StoredDoctype {
    fn view<'pool>(&self, pool: &'pool str) -> Doctype<'pool> {
        let get = |range: &Option<Range<usize>>| range.as_ref().map(|range| &pool[range.clone()]);
        Doctype {
            name: &pool[self.name.clone()],
            public_id: get(&self.public_id),
            system_id: get(&self.system_id),
            internal_subset: get(&self.internal_subset),
        }
    }
}

#[derive(Debug, Default)]
struct Pool(String);

impl Pool {
    fn edit_capacity(
        &self,
        additional: usize,
        spent: usize,
        limit: usize,
    ) -> Result<usize, EditError> {
        let exceeded = EditError::new(EditErrorKind::LimitExceeded(EditLimit::OwnedBytes), None);
        let needed = self.0.len().saturating_add(additional);
        if needed <= self.0.capacity() {
            return Ok(self.0.capacity());
        }
        let remaining = limit.saturating_sub(spent);
        let target = self
            .0
            .capacity()
            .saturating_mul(2)
            .max(needed)
            .min(remaining);
        if target < needed {
            return Err(exceeded);
        }
        Ok(target)
    }

    fn reserve_edit(&mut self, target: usize, spent: &mut usize) -> Result<(), EditError> {
        if target > self.0.capacity() {
            // The preflight bounds the full request by the remaining budget.
            // A failed allocation remains cumulative work.
            *spent += target;
            #[cfg(test)]
            inject_edit_reservation_failure(EditReservation::Pool)?;
            self.0
                .try_reserve_exact(target - self.0.len())
                .map_err(|_| EditError::new(EditErrorKind::Allocation, None))?;
        }
        Ok(())
    }

    fn store_edit(
        &mut self,
        value: &str,
        spent: &mut usize,
        limit: usize,
    ) -> Result<Range<usize>, EditError> {
        let target = self.edit_capacity(value.len(), *spent, limit)?;
        self.reserve_edit(target, spent)?;
        let start = self.0.len();
        self.0.push_str(value);
        Ok(start..self.0.len())
    }

    fn store(
        &mut self,
        value: &str,
        offset: usize,
        budget: &mut Budget,
    ) -> Result<Range<usize>, Error> {
        let start = self.0.len();
        let needed = start + value.len();
        if needed > self.0.capacity() {
            let target = self
                .0
                .capacity()
                .saturating_mul(2)
                .max(needed)
                .min(budget.remaining_owned());
            if target < needed {
                return Err(Error::new(
                    ErrorKind::LimitExceeded(Limit::OwnedBytes),
                    offset,
                ));
            }
            // Charge the entire replacement buffer, even if the allocator can
            // grow in place. Earlier requests remain cumulative work.
            budget.charge_owned(target, offset)?;
            self.0
                .try_reserve_exact(target - start)
                .map_err(|_| Error::new(ErrorKind::Allocation, offset))?;
        }
        self.0.push_str(value);
        Ok(start..needed)
    }

    /// Stores `range`'s text followed by `value`, in place at the tail.
    fn extend(
        &mut self,
        range: Range<usize>,
        value: &str,
        offset: usize,
        budget: &mut Budget,
    ) -> Result<Range<usize>, Error> {
        if range.end != self.0.len() {
            return self.store(&[&self.0[range], value].concat(), offset, budget);
        }
        let added = self.store(value, offset, budget)?;
        Ok(range.start..added.end)
    }

    fn name(
        &mut self,
        name: &Name<'_>,
        offset: usize,
        budget: &mut Budget,
    ) -> Result<StoredName, Error> {
        Ok(StoredName {
            qualified: self.store(name.qualified(), offset, budget)?,
            prefix_length: name.prefix_length(),
            namespace: name
                .namespace()
                .map(|namespace| self.store(namespace, offset, budget))
                .transpose()?,
        })
    }
}

/// An XML tree that owns its logical strings as UTF-8.
///
/// Nodes and attributes contain ranges into one string pool, so moving the
/// document or growing its storage creates no self-references. Parsing,
/// traversal and destruction are iterative. Source bytes are not retained.
///
/// `A` holds node and attribute storage. It is always the global heap unless
/// the `allocator_api` feature (Rust 1.100) enables
/// `parse_in` and `create_in`.
#[derive(Debug)]
pub struct OwnedDocument<A: Allocator = Global> {
    identity: NonZeroUsize,
    nodes: allocator::Vec<OwnedRecord, A>,
    attributes: allocator::Vec<StoredAttribute, A>,
    /// `(attribute index, pool range)` of attribute values that keep general
    /// entity references (no `NOENT`), sorted by index: `save` writes the
    /// range, everything else reads the substituted value.
    kept: allocator::Vec<(usize, Range<usize>), A>,
    pool: Pool,
    root_element: NodeId,
    declaration: Option<StoredDeclaration>,
    edit_owned_bytes: usize,
    edit_created_nodes: usize,
    edit_created_attributes: usize,
    // Parsing produces contiguous preorder. A future structural edit clears
    // this permanently; value-only edits leave the fast path valid.
    slots_are_preorder: bool,
    html: Option<Box<HtmlMeta>>,
    /// `(element, attribute index)` of parsed ID-typed attributes, in document order.
    ids: Vec<(OwnedNodeId, usize)>,
    dtd: Option<Box<crate::valid::Dtd>>,
    /// Diagnostics of `DTDVALID` validation after parsing.
    validity: Vec<crate::valid::Diagnostic<OwnedNodeId>>,
    /// Well-formedness errors a `RECOVER` parse recovered from.
    errors: Vec<Error>,
    /// `ParseOptions::base` or `Editor::set_url`: libxml2's `doc->URL`.
    url: Option<String>,
}

impl OwnedDocument {
    /// Creates a complete owned document containing one constructed root element.
    ///
    /// Initial namespace declarations and attributes become part of the root's
    /// attribute range; `Editor::set_attribute`, `remove_attribute` and
    /// `declare_namespace` change it later.
    ///
    /// ```
    /// use yaxml::{EditLimits, ElementSpec, NameSpec, OwnedDocument, PrefixPolicy, Writer};
    ///
    /// let mut document = OwnedDocument::create(
    ///     ElementSpec {
    ///         name: NameSpec {
    ///             local: "catalog",
    ///             namespace: None,
    ///             prefix: PrefixPolicy::Exact(None),
    ///         },
    ///         items: &[],
    ///     },
    ///     EditLimits::default(),
    /// )?;
    /// let root = document.root_element().id();
    /// let mut edit = document.edit(EditLimits::default())?;
    /// let child = edit.append_element(
    ///     root,
    ///     ElementSpec {
    ///         name: NameSpec {
    ///             local: "item",
    ///             namespace: None,
    ///             prefix: PrefixPolicy::Exact(None),
    ///         },
    ///         items: &[],
    ///     },
    /// )?;
    /// edit.append_text(child, "literal text")?;
    /// let mut writer = Writer::new(Vec::new());
    /// writer.write_owned_document(edit.document())?;
    /// assert_eq!(writer.into_inner(), b"<catalog><item>literal text</item></catalog>");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn create(root: ElementSpec<'_>, limits: EditLimits) -> Result<Self, EditError> {
        construction::create(root, limits, Global)
    }

    pub fn parse(input: &str) -> Result<Self, Error> {
        Self::parse_bytes(input.as_bytes())
    }

    /// Parses UTF-8, UTF-16 or declared single-byte XML independently of the source's lifetime.
    pub fn parse_bytes(input: &[u8]) -> Result<Self, Error> {
        Self::with_limits(input, Limits::default())
    }

    /// Parses with aggregate string accounting for parser normalization and
    /// full new pool-capacity requests. Owned storage can exhaust this budget
    /// before the borrowing DOM; metadata and allocator overhead are separate.
    pub fn with_limits(input: &[u8], limits: Limits) -> Result<Self, Error> {
        Self::with_decode_limits(input, limits, DecodeLimits::default())
    }

    /// Parses UTF-8 or UTF-16 while bounding source, decoded and retained bytes.
    pub fn with_decode_limits(
        input: &[u8],
        limits: Limits,
        decode: DecodeLimits,
    ) -> Result<Self, Error> {
        let options = ParseOptions::default().with_limits(limits);
        Self::parse_with(input, options.with_decode_limits(decode))
    }

    /// The substituting parse (`NOENT | DTDATTR`) used for schema, catalog
    /// and expanded reader documents.
    pub(crate) fn parse_legacy(input: &[u8], limits: Limits) -> Result<Self, Error> {
        let options = ParseOptions::new(crate::options::LEGACY).with_limits(limits);
        Self::parse_with(input, options)
    }

    /// Parses with libxml2 parser options; see [`crate::options`]. With
    /// `XINCLUDE` the result is [`xinclude::process`](crate::xinclude::process)ed
    /// through the same resolver; its failures are `ErrorKind::XInclude`.
    pub fn parse_with(input: &[u8], options: ParseOptions) -> Result<Self, Error> {
        let (mut document, flags, include) = Self::parse_tree(input, options, 0, Global)?;
        if let Some((resolver, base, limits)) = include {
            let options = crate::xinclude::Options {
                base_fixup: flags.fixes_base(),
                limits,
                parse_flags: flags.0,
                ..Default::default()
            };
            let (errors, url) = (std::mem::take(&mut document.errors), document.url.take());
            document = crate::xinclude::process(&document, &base, resolver, &options).map_err(
                |error| match error.kind {
                    crate::xinclude::ErrorKind::Xml(error) => error,
                    _ => Error::new(ErrorKind::XInclude, error.offset.unwrap_or(0)),
                },
            )?;
            (document.errors, document.url) = (errors, url);
        }
        Ok(document.validated(flags))
    }
}

/// What a deferred XInclude pass needs: resolver, base URI and limits.
type Include = (Box<dyn crate::Resolver>, String, Limits);

#[cfg(feature = "allocator_api")]
impl<A: Allocator + Clone> OwnedDocument<A> {
    /// Parses like [`parse_with`](OwnedDocument::parse_with), charging node and
    /// attribute storage to `allocator`; each growth is a fallible reservation
    /// reported as [`ErrorKind::Allocation`]. The string pool and the other
    /// allocations listed in `docs/owned-dom.md` stay on the global heap.
    /// `XINCLUDE` is [`ErrorKind::UnsupportedOption`]: its result tree is
    /// built on the global heap.
    ///
    /// Each storage vector holds a clone of `allocator`; pass `&A` to share one
    /// allocator by reference, or `Arc<dyn Allocator + Send + Sync>` for one
    /// erased, owned type.
    ///
    /// ```
    /// use std::alloc::System;
    /// use yaxml::{OwnedDocument, ParseOptions, Writer};
    ///
    /// let arena = System;
    /// let document = OwnedDocument::parse_in(b"<r a='1'>text</r>", ParseOptions::default(), &arena)?;
    /// assert!(std::ptr::eq(*document.allocator(), &arena));
    /// let mut writer = Writer::new(Vec::new());
    /// writer.write_owned_document(&document)?;
    /// assert_eq!(writer.into_inner(), b"<r a=\"1\">text</r>");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn parse_in(input: &[u8], options: ParseOptions, allocator: A) -> Result<Self, Error> {
        let (document, flags, _) =
            Self::parse_tree(input, options, crate::options::XINCLUDE, allocator)?;
        Ok(document.validated(flags))
    }

    /// [`create`](OwnedDocument::create) with node and attribute storage
    /// charged to `allocator`.
    pub fn create_in(
        root: ElementSpec<'_>,
        limits: EditLimits,
        allocator: A,
    ) -> Result<Self, EditError> {
        construction::create(root, limits, allocator)
    }

    /// The allocator holding this document's nodes and attributes.
    pub fn allocator(&self) -> &A {
        self.nodes.allocator()
    }
}

impl<A: Allocator + Clone> OwnedDocument<A> {
    /// Builds the tree. XInclude, whose result is a global-heap document, is
    /// left to the caller.
    fn parse_tree(
        input: &[u8],
        mut options: ParseOptions,
        unsupported: u32,
        allocator: A,
    ) -> Result<(Self, Flags, Option<Include>), Error> {
        let flags = options.check(true, unsupported)?;
        let (limits, decode) = options.effective_limits();
        if input.len() > limits.max_input_bytes {
            return Err(Error::new(ErrorKind::LimitExceeded(Limit::InputBytes), 0));
        }
        let url = options.base.clone();
        let base = url.clone().unwrap_or_default();
        let include = match (flags.includes(), options.resolver.take()) {
            (true, Some(resolver)) => {
                let share = crate::resolve::share(resolver);
                options.resolver = Some(share());
                Some(share())
            }
            (true, None) => Some(Box::new(|_: crate::Request<'_>| Ok(None)) as Box<_>),
            (false, resolver) => {
                options.resolver = resolver;
                None
            }
        };
        let mut builder = OwnedBuilder::new(input.len(), expected_nodes(input), allocator)?;
        let document = if crate::encoding::needs_decoding(input) {
            let mut parser = PushParser::configured(limits, decode, flags, options);
            let end = parser
                .feed_with(input, |event, origin, budget| {
                    builder.consume(event, origin, budget)
                })
                .and_then(|()| {
                    parser
                        .finish_with(|event, origin, budget| builder.consume(event, origin, budget))
                });
            let errors = Budget::conclude(parser.budget(), end)?;
            builder.finish(parser.position(), parser.catalog(), flags, errors)?
        } else {
            // Recovery reads invalid UTF-8 as libxml2 does; offsets then
            // count the repaired text.
            let repaired = flags
                .recovers()
                .then(|| crate::encoding::repair_utf8(input));
            let repaired = repaired.flatten().transpose()?;
            let source = repaired.as_ref().map_or(input, |(text, _)| text.as_bytes());
            let max_input_bytes = limits.max_input_bytes.max(source.len());
            let read = Limits {
                max_input_bytes,
                ..limits
            };
            let mut reader = Reader::configured(source, read, decode, flags, options)?;
            if let Some((_, error)) = repaired {
                reader.owned_budget().recover(error)?;
            }
            let end = loop {
                let event = match reader.read_event() {
                    Ok(Some(event)) => event,
                    Ok(None) => break Ok(()),
                    Err(error) => break Err(error),
                };
                let origin = reader
                    .event_origin()
                    .ok_or_else(|| Error::new(ErrorKind::InvalidState, reader.position()))?;
                builder.consume(&event, origin, reader.owned_budget())?;
            };
            let errors = Budget::conclude(reader.budget(), end)?;
            builder.finish(reader.position(), reader.catalog(), flags, errors)?
        };
        Ok((
            OwnedDocument { url, ..document },
            flags,
            include.map(|resolver| (resolver, base, limits)),
        ))
    }
}

impl<A: Allocator> OwnedDocument<A> {
    /// Unlinks the node at `index` from its parent and siblings, keeping its
    /// subtree; a detached root stays as it is.
    fn detach(&mut self, index: usize) {
        let record = &mut self.nodes[index];
        let Some(parent) = record.parent.get() else {
            return;
        };
        let (previous, next) = (record.previous_sibling, record.next_sibling);
        (record.parent, record.previous_sibling, record.next_sibling) =
            (Link::NONE, Link::NONE, Link::NONE);
        let size = record.subtree_size;
        self.slots_are_preorder = false;
        match previous.get() {
            Some(previous) => self.nodes[previous.0].next_sibling = next,
            None => self.nodes[parent.0].first_child = next,
        }
        match next.get() {
            Some(next) => self.nodes[next.0].previous_sibling = previous,
            None => self.nodes[parent.0].last_child = previous,
        }
        self.resize_ancestors(parent, size, false);
    }

    /// Links the detached node at `index` under `parent` before `next`
    /// (`Link::NONE`: as the last child).
    fn attach(&mut self, index: usize, parent: usize, next: Link) {
        let previous = match next.get() {
            Some(next) => self.nodes[next.0].previous_sibling,
            None => self.nodes[parent].last_child,
        };
        let (link, parent_link) = (Link::new(NodeId(index)), Link::new(NodeId(parent)));
        let record = &mut self.nodes[index];
        (record.parent, record.previous_sibling, record.next_sibling) =
            (parent_link, previous, next);
        let size = record.subtree_size;
        self.slots_are_preorder = false;
        match previous.get() {
            Some(previous) => self.nodes[previous.0].next_sibling = link,
            None => self.nodes[parent].first_child = link,
        }
        match next.get() {
            Some(next) => self.nodes[next.0].previous_sibling = link,
            None => self.nodes[parent].last_child = link,
        }
        self.resize_ancestors(NodeId(parent), size, true);
    }

    fn resize_ancestors(&mut self, from: NodeId, size: usize, grow: bool) {
        let mut ancestor = Some(from);
        while let Some(id) = ancestor {
            let record = &mut self.nodes[id.0];
            record.subtree_size = match grow {
                true => record.subtree_size + size,
                false => record.subtree_size - size,
            };
            ancestor = record.parent.get();
        }
    }

    /// Detaches and permanently removes the node at `index` and its descendants.
    fn remove(&mut self, index: usize) {
        self.detach(index);
        // A detached root (a fresh copy) has no parent to detach from, but
        // its tombstones still end the all-live preorder fast path.
        self.slots_are_preorder = false;
        // Strip rightmost leaves, then their parents. Each edge is visited a
        // constant number of times, without recursion or a temporary stack.
        let mut current = NodeId(index);
        loop {
            if let Some(last) = self.nodes[current.0].last_child.get() {
                current = last;
                continue;
            }
            let record = &mut self.nodes[current.0];
            let parent = record.parent;
            let previous = record.previous_sibling;
            record.content = Content::Removed;
            record.source = None;
            record.parent = Link::NONE;
            record.first_child = Link::NONE;
            record.last_child = Link::NONE;
            record.previous_sibling = Link::NONE;
            record.next_sibling = Link::NONE;
            record.subtree_size = 0;
            if current.0 == index {
                break;
            }
            let parent = parent.get().expect("removed descendant has a parent");
            self.nodes[parent.0].last_child = previous;
            if let Some(previous) = previous.get() {
                self.nodes[previous.0].next_sibling = Link::NONE;
                current = previous;
            } else {
                self.nodes[parent.0].first_child = Link::NONE;
                current = parent;
            }
        }
    }

    fn validated(mut self, flags: Flags) -> Self {
        if flags.validates() {
            self.validity = self.validate();
        }
        self
    }

    /// The element whose `xml:id` or DTD-declared `ID` attribute has this
    /// value; the first in document order wins, as in libxml2's `xmlGetID`.
    pub fn element_by_id(&self, value: &str) -> Option<OwnedNode<'_, A>> {
        crate::xpath::tree::element_by_id(self, &self.ids, value).and_then(|id| self.node(id))
    }

    /// The declarations of the internal and loaded external subsets.
    pub fn dtd(&self) -> Option<&crate::valid::Dtd> {
        self.dtd.as_deref()
    }

    /// The well-formedness errors a `RECOVER` parse recovered from, in the
    /// order found (at most 100, like libxml2); empty for a well-formed
    /// document and without `RECOVER`.
    pub fn errors(&self) -> &[Error] {
        &self.errors
    }

    /// Validity diagnostics found by `DTDVALID` after parsing; empty without it.
    pub fn validity(&self) -> &[crate::valid::Diagnostic<OwnedNodeId>] {
        &self.validity
    }

    /// Checks the current tree against its DTD, as libxml2's `xmlValidateDocument`.
    /// Never fails: an empty result means valid; see [`crate::valid`].
    pub fn validate(&self) -> Vec<crate::valid::Diagnostic<OwnedNodeId>> {
        crate::valid::validate_document(self, self.dtd())
    }

    pub(crate) fn ids(&self) -> &[(OwnedNodeId, usize)] {
        &self.ids
    }

    /// Returns the synthetic root, including surrounding comments and PIs.
    pub fn root(&self) -> OwnedNode<'_, A> {
        self.view(NodeId(0)).expect("owned document root is live")
    }

    /// The document element; the synthetic root for an HTML or `RECOVER`
    /// document without one ([`Self::document_element`] tells them apart).
    pub fn root_element(&self) -> OwnedNode<'_, A> {
        self.view(self.root_element)
            .expect("owned document element is live")
    }

    /// The document element, or `None` for an HTML or `RECOVER` document
    /// without one (libxml2's `xmlDocGetRootElement`, DOM's `documentElement`).
    pub fn document_element(&self) -> Option<OwnedNode<'_, A>> {
        (self.root_element != NodeId(0)).then(|| self.root_element())
    }

    /// Looks up a checked owned handle, rejecting foreign or out-of-range IDs.
    #[inline]
    pub fn node(&self, id: OwnedNodeId) -> Option<OwnedNode<'_, A>> {
        if id.document != self.identity {
            return None;
        }
        self.view(id.slot)
    }

    #[inline]
    fn view(&self, id: NodeId) -> Option<OwnedNode<'_, A>> {
        let record = self.nodes.get(id.0)?;
        // Contiguous parsed slots are all live. Structural edits permanently
        // disable that invariant before any slot can become Removed.
        (self.slots_are_preorder || !matches!(record.content, Content::Removed)).then_some(
            OwnedNode {
                document: self,
                record,
            },
        )
    }

    pub fn nodes(&self) -> impl ExactSizeIterator<Item = OwnedNode<'_, A>> + '_ {
        self.root().descendants()
    }

    /// Preorder rank per slot, or `None` while slots are already preorder.
    pub(crate) fn preorder_ranks(&self) -> Option<Vec<usize>> {
        (!self.slots_are_preorder).then(|| {
            let mut ranks = vec![0; self.nodes.len()];
            for (rank, node) in self.nodes().enumerate() {
                ranks[node.record.id.0] = rank;
            }
            ranks
        })
    }

    fn name_parts(&self, name: &StoredName) -> (&str, Option<&str>) {
        let pool = &self.pool.0;
        (
            &pool[name.qualified.clone()],
            name.namespace.as_ref().map(|range| &pool[range.clone()]),
        )
    }

    pub fn declaration(&self) -> Option<Declaration<'_>> {
        self.declaration
            .as_ref()
            .map(|declaration| declaration.view(&self.pool.0))
    }

    /// Returns the document type declaration while its node remains.
    pub fn doctype(&self) -> Option<Doctype<'_>> {
        self.root().children().find_map(OwnedNode::doctype)
    }

    /// For an HTML document, the first content XML cannot represent
    /// ([`xml_problem`]) or a second top-level element. XML documents always
    /// pass.
    pub(crate) fn xml_violation(&self) -> Option<String> {
        self.html.as_ref()?;
        let elements = self
            .root()
            .children()
            .filter(|node| node.kind() == OwnedNodeKind::Element);
        if elements.count() > 1 {
            return Some("a second top-level element is not XML".to_owned());
        }
        self.nodes()
            .find_map(|node| xml_problem(node).map(|(_, problem)| format!("{problem} is not XML")))
    }

    /// The document URL (`ParseOptions::base` or [`Editor::set_url`],
    /// libxml2's `doc->URL`), against which [`OwnedNode::base_uri`] resolves.
    pub fn url(&self) -> Option<&str> {
        self.url.as_deref()
    }

    /// True for documents built by the HTML parser.
    pub fn is_html(&self) -> bool {
        self.html.is_some()
    }

    /// The input encoding libxml2 records for an HTML document (`doc->encoding`).
    /// `None` means ASCII-only or BOM-detected input; serialization then uses
    /// ASCII with HTML 4 entities.
    pub fn html_encoding(&self) -> Option<&str> {
        self.html.as_ref()?.encoding.as_deref()
    }

    /// Opens a bounded tree editor. Owned-byte, created-node and created-attribute
    /// counters persist across editors; a ceiling below earlier expenditure is rejected.
    /// Parsed string capacity is the baseline and is not charged again.
    ///
    /// ```
    /// use yaxml::{EditLimits, OwnedDocument};
    /// let mut document = OwnedDocument::parse("<r>old</r>")?;
    /// let text = document.root_element().first_child().unwrap().id();
    /// document.edit(EditLimits::default())?.set_node_value(text, "new")?;
    /// assert_eq!(document.node(text).unwrap().text(), Some("new"));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// Views cannot be used across mutation:
    ///
    /// ```compile_fail
    /// use yaxml::{EditLimits, OwnedDocument};
    /// let mut document = OwnedDocument::parse("<r>old</r>").unwrap();
    /// let view = document.root_element().first_child().unwrap();
    /// let mut editor = document.edit(EditLimits::default()).unwrap();
    /// editor.set_node_value(view.id(), "new").unwrap();
    /// ```
    pub fn edit(&mut self, limits: EditLimits) -> Result<Editor<'_, A>, EditError> {
        if self.edit_created_nodes > limits.max_created_nodes {
            return Err(EditError::new(
                EditErrorKind::LimitExceeded(EditLimit::CreatedNodes),
                None,
            ));
        }
        if self.edit_owned_bytes > limits.max_owned_bytes {
            return Err(EditError::new(
                EditErrorKind::LimitExceeded(EditLimit::OwnedBytes),
                None,
            ));
        }
        if self.edit_created_attributes > limits.max_created_attributes {
            return Err(EditError::new(
                EditErrorKind::LimitExceeded(EditLimit::CreatedAttributes),
                None,
            ));
        }
        Ok(Editor {
            document: self,
            limits,
        })
    }
}

/// What XML cannot represent in an HTML node, and the edit error it is: a
/// name that is not an NCName (or an `xmlns` attribute, which would rebind
/// namespaces), a character outside XML's range, or a comment XML forbids.
fn xml_problem<A: Allocator>(node: OwnedNode<'_, A>) -> Option<(EditErrorKind, String)> {
    let chars = |text: &str| text.chars().all(crate::escape::xml_char);
    let ncname = |name: &str| crate::name::validate_name(name, 0, false).is_ok();
    let text = node.text().unwrap_or_default();
    match node.kind() {
        OwnedNodeKind::Element => {
            let name = node.qualified_name().unwrap_or_default();
            if !ncname(name) {
                return Some((EditErrorKind::InvalidName, format!("element name {name:?}")));
            }
            node.attributes().find_map(|attribute| {
                let name = attribute.name.qualified();
                if !ncname(name) || name == "xmlns" {
                    Some((
                        EditErrorKind::InvalidName,
                        format!("attribute name {name:?}"),
                    ))
                } else if !chars(&attribute.value) {
                    let problem = format!("attribute {name:?} value");
                    Some((EditErrorKind::InvalidCharacter, problem))
                } else {
                    None
                }
            })
        }
        OwnedNodeKind::Comment => (!chars(text) || text.contains("--") || text.ends_with('-'))
            .then(|| (EditErrorKind::InvalidValue, format!("comment {text:?}"))),
        OwnedNodeKind::Text | OwnedNodeKind::Cdata => {
            (!chars(text)).then(|| (EditErrorKind::InvalidCharacter, "character data".to_owned()))
        }
        _ => None,
    }
}

/// Inclusive ceilings for supplied strings and cumulative editing work.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EditLimits {
    /// UTF-8 bytes per supplied value or PI target, checked before scanning.
    /// Default: 1 MiB.
    pub max_value_bytes: usize,
    /// Full new pool-capacity requests across all editors. Default: 64 MiB.
    /// Includes earlier released buffers and failed reservation charges, but
    /// excludes parsed baseline capacity, metadata and allocator overhead.
    pub max_owned_bytes: usize,
    /// Successful fresh slots across all editors, never refunded by removal.
    /// Parsed nodes are excluded. Default: 1,000,000.
    pub max_created_nodes: usize,
    /// UTF-8 bytes per supplied XML name, prefix or namespace URI. Default: 1 MiB.
    pub max_name_bytes: usize,
    /// Aggregate supplied string bytes per construction call. Default: 8 MiB.
    pub max_supplied_bytes: usize,
    /// Initial items on one constructed element, including declarations. Default: 1,024.
    pub max_attributes: usize,
    /// Prospective element depth, with the document element at depth one. Default: 256.
    pub max_depth: usize,
    /// Successful initial attributes and declarations across all editors. Default: 1,000,000.
    pub max_created_attributes: usize,
    /// Namespace lookup and comparison work per construction call. Each
    /// element ancestor, examined attribute, and prefix candidate costs one;
    /// compared prefix, URI, and expanded-name bytes cost their full operand
    /// lengths. Caller-string validation is charged to `max_supplied_bytes`.
    /// Default: 64 MiB.
    pub max_namespace_work: usize,
}

/// A lexical prefix choice for a constructed element or attribute name.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrefixPolicy<'a> {
    /// Preserve this exact prefix choice; `None` means unprefixed.
    Exact(Option<&'a str>),
    /// Reuse a suitable in-scope binding or choose a deterministic prefix.
    Automatic,
}

/// Borrowed expanded name and requested lexical spelling for construction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NameSpec<'a> {
    /// XML local name, without a colon.
    pub local: &'a str,
    /// Expanded-name namespace; `None` means no namespace.
    pub namespace: Option<&'a str>,
    /// Exact lexical spelling or deterministic automatic resolution.
    pub prefix: PrefixPolicy<'a>,
}

/// One ordinary attribute in a fresh element specification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AttributeSpec<'a> {
    /// Expanded name and requested lexical spelling.
    pub name: NameSpec<'a>,
    /// Literal attribute value, never parsed as markup.
    pub value: &'a str,
}

/// One namespace declaration in a fresh element specification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NamespaceSpec<'a> {
    /// Bound prefix; `None` declares or undeclares the default namespace.
    pub prefix: Option<&'a str>,
    /// Namespace URI; empty is permitted only for default undeclaration.
    pub uri: &'a str,
}

/// An ordered initial namespace declaration or ordinary attribute.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ElementItem<'a> {
    Namespace(NamespaceSpec<'a>),
    Attribute(AttributeSpec<'a>),
}

/// Borrowed definition of a fresh element and its initial items.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ElementSpec<'a> {
    /// Expanded name and requested lexical spelling.
    pub name: NameSpec<'a>,
    /// Initial declarations and attributes, kept in caller order.
    pub items: &'a [ElementItem<'a>],
}

impl Default for EditLimits {
    fn default() -> Self {
        Self {
            max_value_bytes: 1024 * 1024,
            max_owned_bytes: 64 * 1024 * 1024,
            max_created_nodes: 1_000_000,
            max_name_bytes: 1024 * 1024,
            max_supplied_bytes: 8 * 1024 * 1024,
            max_attributes: 1024,
            max_depth: 256,
            max_created_attributes: 1_000_000,
            max_namespace_work: 64 * 1024 * 1024,
        }
    }
}

/// Exclusive, atomic edits: values, creation, moves, unlink and replacement,
/// attribute membership, names and namespaces, removal and compaction (see
/// `docs/owned-dom.md`). Every failure leaves the tree unchanged.
/// An editor remains usable after an error. Parsed nodes keep their
/// provenance; created nodes have no original source location.
/// Dropping an editor performs no work. Equal replacements take the edit path.
pub struct Editor<'document, A: Allocator = Global> {
    document: &'document mut OwnedDocument<A>,
    limits: EditLimits,
}

impl<A: Allocator> Editor<'_, A> {
    pub fn document(&self) -> &OwnedDocument<A> {
        self.document
    }

    /// Appends a fresh element with its initial attributes and namespace declarations.
    /// Later membership changes use `set_attribute`, `remove_attribute` and
    /// `declare_namespace`.
    pub fn append_element(
        &mut self,
        parent: OwnedNodeId,
        element: ElementSpec<'_>,
    ) -> Result<OwnedNodeId, EditError> {
        construction::append(self, parent, element)
    }

    /// Appends one fresh Text node under an element, without merging neighbors.
    pub fn append_text(
        &mut self,
        parent: OwnedNodeId,
        value: &str,
    ) -> Result<OwnedNodeId, EditError> {
        self.append_leaf(parent, ValueKind::Text, None, value)
    }

    /// Appends one fresh CDATA node under an element. Raw CR and `]]>` fail.
    pub fn append_cdata(
        &mut self,
        parent: OwnedNodeId,
        value: &str,
    ) -> Result<OwnedNodeId, EditError> {
        self.append_leaf(parent, ValueKind::Cdata, None, value)
    }

    /// Appends a comment under an element or after the document's epilog tail.
    pub fn append_comment(
        &mut self,
        parent: OwnedNodeId,
        value: &str,
    ) -> Result<OwnedNodeId, EditError> {
        self.append_leaf(parent, ValueKind::Comment, None, value)
    }

    /// Appends a PI under an element or after the document's epilog tail.
    /// The target is an XML Name without a colon and cannot be any ASCII case
    /// spelling of `xml`. Target failures index target; data failures index data.
    /// Both string lengths are checked before either string is scanned.
    /// Target and data share one pool reservation: on a parsed `<r/>` with one
    /// pool byte, appending target `p` and data `d` charges capacity 3 once.
    pub fn append_processing_instruction(
        &mut self,
        parent: OwnedNodeId,
        target: &str,
        data: &str,
    ) -> Result<OwnedNodeId, EditError> {
        self.append_leaf(parent, ValueKind::Pi, Some(target), data)
    }

    fn append_leaf(
        &mut self,
        parent: OwnedNodeId,
        kind: ValueKind,
        target: Option<&str>,
        value: &str,
    ) -> Result<OwnedNodeId, EditError> {
        let parent = self.resolve(parent)?;
        match (&self.document.nodes[parent].content, kind) {
            (Content::Element { .. }, _)
            | (Content::Document, ValueKind::Comment | ValueKind::Pi) => {}
            _ => return Err(EditError::new(EditErrorKind::WrongNodeKind, None)),
        }
        // PI argument lengths precede all target/data validation, so a large
        // data argument cannot make scanning an invalid target the first error.
        if let Some(target) = target {
            validate_value_length(target, self.limits.max_value_bytes)?;
        }
        validate_value_length(value, self.limits.max_value_bytes)?;
        if let Some(target) = target {
            validate_characters(target)?;
            crate::name::validate_name(target, 0, false)
                .map_err(|error| EditError::new(EditErrorKind::InvalidName, Some(error.offset)))?;
            if target.eq_ignore_ascii_case("xml") {
                return Err(EditError::new(EditErrorKind::InvalidName, Some(0)));
            }
        }
        validate_value(value, kind, self.limits.max_value_bytes)?;

        let exceeded = EditError::new(EditErrorKind::LimitExceeded(EditLimit::CreatedNodes), None);
        let created = self.document.edit_created_nodes + 1;
        if created > self.limits.max_created_nodes {
            return Err(exceeded);
        }
        let additional = target.unwrap_or("").len() + value.len();
        // Preflight every quota before reserving either arena. PI strings share
        // one bounded growth request, so a later string cannot fail a quota
        // after staging the first one.
        let capacity = self.document.pool.edit_capacity(
            additional,
            self.document.edit_owned_bytes,
            self.limits.max_owned_bytes,
        )?;
        let id = NodeId(self.document.nodes.len());
        let allocation = EditError::new(EditErrorKind::Allocation, None);
        #[cfg(test)]
        if self.document.nodes.len() == self.document.nodes.capacity() {
            inject_edit_reservation_failure(EditReservation::Node)?;
        }
        self.document.nodes.try_reserve(1).map_err(|_| allocation)?;
        self.document
            .pool
            .reserve_edit(capacity, &mut self.document.edit_owned_bytes)?;

        // Everything below is infallible and uses the pre-reserved capacity.
        let pool = &mut self.document.pool.0;
        let target_start = pool.len();
        pool.push_str(target.unwrap_or(""));
        let target_range = target_start..pool.len();
        let value_start = pool.len();
        pool.push_str(value);
        let value_range = value_start..pool.len();
        let content = match kind {
            ValueKind::Text => Content::Text(value_range),
            ValueKind::Cdata => Content::Cdata(value_range),
            ValueKind::Comment => Content::Comment(value_range),
            ValueKind::Pi => Content::ProcessingInstruction {
                target: target_range,
                data: value_range,
            },
        };
        self.document.nodes.push(OwnedRecord::detached(id, content));
        self.document.attach(id.0, parent, Link::NONE);
        self.document.edit_created_nodes = created;
        Ok(OwnedNodeId {
            document: self.document.identity,
            slot: id,
        })
    }

    /// Permanently removes a node and all descendants without allocating.
    /// The synthetic root and document element are protected. Removed handles
    /// never become live again; slot and pool expenditure is not refunded.
    pub fn remove_subtree(&mut self, node: OwnedNodeId) -> Result<(), EditError> {
        let index = self.resolve(node)?;
        if index == 0 || NodeId(index) == self.document.root_element {
            return Err(EditError::new(EditErrorKind::ProtectedNode, None));
        }
        self.document.remove(index);
        Ok(())
    }

    fn resolve(&self, node: OwnedNodeId) -> Result<usize, EditError> {
        if node.document != self.document.identity
            || self
                .document
                .nodes
                .get(node.slot.0)
                .is_none_or(|record| matches!(record.content, Content::Removed))
        {
            return Err(EditError::new(EditErrorKind::InvalidHandle, None));
        }
        Ok(node.slot.0)
    }

    /// Replaces Text, CDATA, Comment or PI data; PI targets remain unchanged.
    /// Non-text lexical values reject raw CR and their closing delimiters.
    /// Comments also reject a trailing dash; PI data rejects leading XML space.
    pub fn set_node_value(&mut self, node: OwnedNodeId, value: &str) -> Result<(), EditError> {
        let index = self.resolve(node)?;
        let (target, kind) = match &mut self.document.nodes[index].content {
            Content::Text(range) => (range, ValueKind::Text),
            Content::Cdata(range) => (range, ValueKind::Cdata),
            Content::Comment(range) => (range, ValueKind::Comment),
            Content::ProcessingInstruction { data, .. } => (data, ValueKind::Pi),
            Content::Removed
            | Content::Document
            | Content::DocumentType(_)
            | Content::Element { .. }
            | Content::EntityRef(_) => {
                return Err(EditError::new(EditErrorKind::WrongNodeKind, None));
            }
        };
        validate_value(value, kind, self.limits.max_value_bytes)?;
        let replacement = self.document.pool.store_edit(
            value,
            &mut self.document.edit_owned_bytes,
            self.limits.max_owned_bytes,
        )?;
        *target = replacement;
        Ok(())
    }

    /// Replaces one existing attribute by expanded name, retaining its spelling
    /// and position. Some("") differs from None; selector names are not parsed.
    /// Missing attributes and XMLNS declaration edits are rejected.
    pub fn set_attribute_value(
        &mut self,
        element: OwnedNodeId,
        namespace: Option<&str>,
        local: &str,
        value: &str,
    ) -> Result<(), EditError> {
        let index = self.resolve(element)?;
        let attributes = match &self.document.nodes[index].content {
            Content::Element { attributes, .. } => attributes,
            _ => return Err(EditError::new(EditErrorKind::WrongNodeKind, None)),
        };
        let target = attributes
            .clone()
            .find(|index| {
                let name = self.document.attributes[*index]
                    .name
                    .view(&self.document.pool.0);
                name.local() == local && name.namespace() == namespace
            })
            .ok_or(EditError::new(EditErrorKind::MissingAttribute, None))?;
        if namespace == Some(XMLNS) {
            return Err(EditError::new(EditErrorKind::NamespaceDeclaration, None));
        }
        validate_value(value, ValueKind::Text, self.limits.max_value_bytes)?;
        let replacement = self.document.pool.store_edit(
            value,
            &mut self.document.edit_owned_bytes,
            self.limits.max_owned_bytes,
        )?;
        self.document.attributes[target].value = replacement;
        // The new value replaces the attribute's kept references too.
        let kept = &mut self.document.kept;
        if let Ok(position) = kept.binary_search_by_key(&target, |(index, _)| *index) {
            kept.remove(position);
        }
        if let Some(html) = &mut self.document.html
            && let Ok(position) = html.valueless.binary_search(&target)
        {
            html.valueless.remove(position);
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum ValueKind {
    Text,
    Cdata,
    Comment,
    Pi,
}

fn validate_value_length(value: &str, limit: usize) -> Result<(), EditError> {
    if value.len() > limit {
        return Err(EditError::new(
            EditErrorKind::LimitExceeded(EditLimit::ValueBytes),
            None,
        ));
    }
    Ok(())
}

fn validate_characters(value: &str) -> Result<(), EditError> {
    if let Some(index) = crate::escape::invalid_char(value) {
        return Err(EditError::new(EditErrorKind::InvalidCharacter, Some(index)));
    }
    Ok(())
}

fn validate_value(value: &str, kind: ValueKind, limit: usize) -> Result<(), EditError> {
    validate_value_length(value, limit)?;
    validate_characters(value)?;
    let invalid = match kind {
        ValueKind::Text => None,
        ValueKind::Cdata => value.find("]]>").into_iter().chain(value.find('\r')).min(),
        ValueKind::Comment => value
            .find("--")
            .into_iter()
            .chain(value.find('\r'))
            .chain(value.ends_with('-').then(|| value.len() - 1))
            .min(),
        ValueKind::Pi => value
            .find("?>")
            .into_iter()
            .chain(value.find('\r'))
            .chain(
                value
                    .as_bytes()
                    .first()
                    .is_some_and(|byte| crate::parser::space(*byte))
                    .then_some(0),
            )
            .min(),
    };
    if let Some(index) = invalid {
        return Err(EditError::new(EditErrorKind::InvalidValue, Some(index)));
    }
    Ok(())
}

/// Nodes to reserve before parsing `input`: two per `<` (a tag and the text
/// after it) in a sampled prefix, scaled to the input and capped. Markup-heavy
/// input skips early regrowth; text-heavy input reserves only what it needs.
fn expected_nodes(input: &[u8]) -> usize {
    const SAMPLE: usize = 16 * 1024;
    const CAP: usize = 4096;
    let sample = &input[..input.len().min(SAMPLE)];
    let tags = memchr::memchr_iter(b'<', sample).count();
    let scaled = (2 * tags + 1).saturating_mul(input.len()) / sample.len().max(1);
    scaled.min(CAP)
}

/// Concrete shared sink: all strings are copied using the parser's budget.
struct OwnedBuilder<A: Allocator = Global> {
    nodes: allocator::Vec<OwnedRecord, A>,
    /// `(element slot, attribute index)` of each `xml:id`, in document order.
    xml_ids: Vec<(NodeId, usize)>,
    attributes: allocator::Vec<StoredAttribute, A>,
    kept: allocator::Vec<(usize, Range<usize>), A>,
    pool: Pool,
    root_element: NodeId,
    declaration: Option<StoredDeclaration>,
    parents: Vec<NodeId>,
}

impl<A: Allocator + Clone> OwnedBuilder<A> {
    /// A builder with room for the root and `expected` more nodes.
    fn new(source_length: usize, expected: usize, allocator: A) -> Result<Self, Error> {
        let mut nodes = allocator::Vec::new_in(allocator.clone());
        nodes
            .try_reserve(1 + expected)
            .map_err(|_| Error::new(ErrorKind::Allocation, 0))?;
        nodes.push(OwnedRecord::root(source_length));
        Ok(Self {
            nodes,
            xml_ids: Vec::new(),
            kept: allocator::Vec::new_in(allocator.clone()),
            attributes: allocator::Vec::new_in(allocator),
            pool: Pool::default(),
            root_element: NodeId(0),
            declaration: None,
            parents: Vec::new(),
        })
    }

    fn consume(
        &mut self,
        event: &Event<'_>,
        origin: SourceOrigin,
        budget: &mut Budget,
    ) -> Result<(), Error> {
        let starts = matches!(event, Event::Start(_) | Event::EntityStart(_));

        let offset = origin.error_offset();
        let parent = self.parents.last().copied().unwrap_or(NodeId(0));
        if let Event::Text(value) = event
            && (value.is_empty() || self.merge_text(parent, value, origin, budget)?)
        {
            return Ok(());
        }
        let content = match event {
            Event::Declaration(declaration) => {
                self.declaration = Some(store_declaration(
                    &mut self.pool,
                    *declaration,
                    offset,
                    budget,
                )?);
                return Ok(());
            }
            // The parser balances ends, so `parent` is what they close.
            // Parsing still appends in preorder. Closing an element gives
            // its count directly, without updating every open ancestor.
            Event::EntityEnd | Event::End(_) => {
                let end = self.nodes.len();
                let record = &mut self.nodes[parent.0];
                if let Event::End(_) = event {
                    record.source = record.source.map(|opening| opening.close_element(origin));
                }
                self.parents.pop();
                record.subtree_size = end - parent.0;
                return Ok(());
            }
            event => {
                if let Event::Start(element) | Event::Empty(element) = event {
                    self.note_xml_ids(element, offset)?;
                    self.store_kept(budget, offset)?;
                }
                store_content(&mut self.pool, &mut self.attributes, event, offset, budget)?
            }
        };
        let id = append_record(&mut self.nodes, parent, content, Some(origin), offset)?;
        let element = matches!(self.nodes[id.0].content, Content::Element { .. });
        if element && parent == NodeId(0) {
            self.root_element = id;
        }
        if starts {
            crate::parser::reserve_frame(&mut self.parents, offset)?;
            self.parents.push(id);
        }
        Ok(())
    }

    fn merge_text(
        &mut self,
        parent: NodeId,
        value: &str,
        origin: SourceOrigin,
        budget: &mut Budget,
    ) -> Result<bool, Error> {
        let Some(last) = self.nodes[parent.0].last_child.get() else {
            return Ok(false);
        };
        let record = &self.nodes[last.0];
        let (Content::Text(range), Some(source)) = (&record.content, record.source) else {
            return Ok(false);
        };
        let joined = source.join_text(origin);
        let extended = self
            .pool
            .extend(range.clone(), value, origin.error_offset(), budget)?;
        let record = &mut self.nodes[last.0];
        record.content = Content::Text(extended);
        record.source = Some(joined);
        Ok(true)
    }

    /// Stores the kept-reference forms of the element about to be appended.
    fn store_kept(&mut self, budget: &mut Budget, offset: usize) -> Result<(), Error> {
        let mut kept = std::mem::take(budget.kept());
        if kept.is_empty() {
            return Ok(());
        }
        self.kept
            .try_reserve(kept.len())
            .map_err(|_| Error::new(ErrorKind::Allocation, offset))?;
        let start = self.attributes.len();
        for (index, form) in kept.drain(..) {
            let range = self.pool.store(&form, offset, budget)?;
            self.kept.push((start + index, range));
        }
        // Return the drained buffer for the next tag.
        *budget.kept() = kept;
        Ok(())
    }

    /// Notes the `xml:id` attributes of the element about to be appended.
    fn note_xml_ids(&mut self, element: &crate::Element<'_>, offset: usize) -> Result<(), Error> {
        let slot = NodeId(self.nodes.len());
        for (index, attribute) in element.attributes.iter().enumerate() {
            if attribute.name.qualified() == "xml:id" {
                self.xml_ids
                    .try_reserve(1)
                    .map_err(|_| Error::new(ErrorKind::Allocation, offset))?;
                self.xml_ids.push((slot, index));
            }
        }
        Ok(())
    }

    fn finish(
        mut self,
        position: usize,
        catalog: Option<&crate::dtd_catalog::DtdCatalog>,
        flags: Flags,
        errors: Vec<Error>,
    ) -> Result<OwnedDocument<A>, Error> {
        let xml_ids = std::mem::take(&mut self.xml_ids);
        // A recovered parse can stop with elements open: close them where
        // reading stopped (one opened by entity replacement text keeps its span).
        let (stop, count) = (SourceOrigin::input(position..position), self.nodes.len());
        for id in std::mem::take(&mut self.parents) {
            let record = &mut self.nodes[id.0];
            record.subtree_size = count - id.0;
            if let (Content::Element { .. }, Some(source)) = (&record.content, record.source) {
                record.source = Some(source.close_element(stop));
            }
        }
        // Recovery keeps a document without an element unless the input is
        // empty, as libxml2 does (otherwise the reader failed already).
        let input = self.nodes[0].source.and_then(SourceOrigin::input_range);
        let rootless = input.is_some_and(|input| !input.is_empty());
        let document = self
            .finish_with_counter(position, &LAST_DOCUMENT_ID, rootless)
            .map_err(|error| match error.kind {
                ErrorKind::MissingRoot => errors.first().copied().unwrap_or(error),
                _ => error,
            })?;
        let dtd = catalog.map(|catalog| Box::new(catalog.dtd().clone()));
        let mut document = OwnedDocument {
            dtd,
            errors,
            ..document
        };
        // Without DTD-declared ID attributes, the `xml:id`s noted while
        // building are exactly what a full traversal would find.
        document.ids = if !flags.records_ids() {
            Vec::new()
        } else if crate::xpath::tree::declares_ids(document.dtd()) {
            crate::xpath::tree::find_ids(&document, document.dtd())
        } else {
            let document = document.identity;
            xml_ids
                .into_iter()
                .map(|(slot, index)| (OwnedNodeId { document, slot }, index))
                .collect()
        };
        if flags.drops_blanks() {
            for blank in crate::xpath::tree::blanks(&document) {
                document.remove(blank.slot.0);
            }
        }
        Ok(document)
    }

    fn finish_with_counter(
        mut self,
        position: usize,
        counter: &AtomicUsize,
        rootless: bool,
    ) -> Result<OwnedDocument<A>, Error> {
        if self.root_element == NodeId(0) && !rootless {
            return Err(Error::new(ErrorKind::MissingRoot, position));
        }
        self.nodes[0].subtree_size = self.nodes.len();
        Ok(OwnedDocument {
            identity: document_identity(counter)?,
            nodes: self.nodes,
            attributes: self.attributes,
            kept: self.kept,
            pool: self.pool,
            root_element: self.root_element,
            declaration: self.declaration,
            edit_owned_bytes: 0,
            edit_created_nodes: 0,
            edit_created_attributes: 0,
            slots_are_preorder: true,
            html: None,
            ids: Vec::new(),
            dtd: None,
            validity: Vec::new(),
            errors: Vec::new(),
            url: None,
        })
    }
}

fn store_declaration(
    pool: &mut Pool,
    declaration: Declaration<'_>,
    offset: usize,
    budget: &mut Budget,
) -> Result<StoredDeclaration, Error> {
    Ok(StoredDeclaration {
        version: pool.store(declaration.version, offset, budget)?,
        encoding: declaration
            .encoding
            .map(|encoding| pool.store(encoding, offset, budget))
            .transpose()?,
        standalone: declaration.standalone,
    })
}

/// Stores a node event's strings; declarations and end tags are not nodes.
fn store_content<A: Allocator>(
    pool: &mut Pool,
    attributes: &mut allocator::Vec<StoredAttribute, A>,
    event: &Event<'_>,
    offset: usize,
    budget: &mut Budget,
) -> Result<Content, Error> {
    Ok(match event {
        Event::Doctype(doctype) => {
            let mut store = |value: Option<&str>| {
                value
                    .map(|value| pool.store(value, offset, budget))
                    .transpose()
            };
            Content::DocumentType(StoredDoctype {
                name: store(Some(doctype.name))?.expect("present name"),
                public_id: store(doctype.public_id)?,
                system_id: store(doctype.system_id)?,
                internal_subset: store(doctype.internal_subset)?,
            })
        }
        Event::Start(element) | Event::Empty(element) => {
            let name = pool.name(&element.name, offset, budget)?;
            let start = attributes.len();
            attributes
                .try_reserve(element.attributes.len())
                .map_err(|_| Error::new(ErrorKind::Allocation, offset))?;
            for attribute in element.attributes.iter() {
                attributes.push(StoredAttribute {
                    name: pool.name(&attribute.name, offset, budget)?,
                    value: pool.store(&attribute.value, offset, budget)?,
                });
            }
            Content::Element {
                name,
                attributes: start..attributes.len(),
            }
        }
        Event::Text(value) => Content::Text(pool.store(value, offset, budget)?),
        Event::Cdata(value) => Content::Cdata(pool.store(value, offset, budget)?),
        Event::Comment(value) => Content::Comment(pool.store(value, offset, budget)?),
        Event::ProcessingInstruction { target, data } => Content::ProcessingInstruction {
            target: pool.store(target, offset, budget)?,
            data: pool.store(data, offset, budget)?,
        },
        Event::EntityStart(name) => Content::EntityRef(pool.store(name, offset, budget)?),
        Event::Declaration(_) | Event::End(_) | Event::EntityEnd => {
            return Err(Error::new(ErrorKind::InvalidState, offset));
        }
    })
}

/// Builds a document from copied node events, without source origins and
/// without merging Text: the XInclude result tree. Appends follow preorder and
/// each element is closed after its subtree.
pub(crate) struct TreeBuilder {
    nodes: allocator::Vec<OwnedRecord, Global>,
    attributes: allocator::Vec<StoredAttribute, Global>,
    pool: Pool,
    root_element: NodeId,
    declaration: Option<StoredDeclaration>,
    ids: Vec<(NodeId, usize)>,
    budget: Budget,
}

impl TreeBuilder {
    pub(crate) const ROOT: NodeId = NodeId(0);

    pub(crate) fn new(limits: Limits, declaration: Option<Declaration<'_>>) -> Result<Self, Error> {
        let mut pool = Pool::default();
        let mut budget = Budget::new(limits);
        let declaration = declaration
            .map(|declaration| store_declaration(&mut pool, declaration, 0, &mut budget))
            .transpose()?;
        Ok(Self {
            nodes: OwnedBuilder::new(0, 0, Global)?.nodes,
            attributes: allocator::Vec::new_in(Global),
            pool,
            root_element: NodeId(0),
            declaration,
            ids: Vec::new(),
            budget,
        })
    }

    /// Appends a node event as `parent`'s last child; `ids` are the positions
    /// of an element's ID-typed attributes.
    pub(crate) fn append(
        &mut self,
        parent: NodeId,
        event: Event<'_>,
        ids: &[usize],
    ) -> Result<NodeId, Error> {
        if self.nodes.len() > self.budget.max_events() {
            return Err(Error::new(ErrorKind::LimitExceeded(Limit::Events), 0));
        }
        let content = store_content(
            &mut self.pool,
            &mut self.attributes,
            &event,
            0,
            &mut self.budget,
        )?;
        let element = matches!(content, Content::Element { .. });
        let id = append_record(&mut self.nodes, parent, content, None, 0)?;
        if element && parent == Self::ROOT {
            self.root_element = id;
        }
        self.ids
            .try_reserve(ids.len())
            .map_err(|_| Error::new(ErrorKind::Allocation, 0))?;
        self.ids.extend(ids.iter().map(|&index| (id, index)));
        Ok(id)
    }

    /// Ends an element: everything appended since it is its subtree.
    pub(crate) fn close(&mut self, id: NodeId) {
        self.nodes[id.0].subtree_size = self.nodes.len() - id.0;
    }

    pub(crate) fn finish(self) -> Result<OwnedDocument, Error> {
        let document = OwnedBuilder {
            nodes: self.nodes,
            xml_ids: Vec::new(),
            attributes: self.attributes,
            kept: allocator::Vec::new_in(Global),
            pool: self.pool,
            root_element: self.root_element,
            declaration: self.declaration,
            parents: Vec::new(),
        }
        .finish_with_counter(0, &LAST_DOCUMENT_ID, false)?;
        let document_id = document.identity;
        let ids = self
            .ids
            .into_iter()
            .map(|(slot, index)| {
                let element = OwnedNodeId {
                    document: document_id,
                    slot,
                };
                (element, index)
            })
            .collect();
        Ok(OwnedDocument { ids, ..document })
    }
}

/// Appends a record as `parent`'s last child; parsing appends in preorder.
fn append_record<A: Allocator>(
    nodes: &mut allocator::Vec<OwnedRecord, A>,
    parent: NodeId,
    content: Content,
    source: Option<SourceOrigin>,
    offset: usize,
) -> Result<NodeId, Error> {
    let allocation = Error::new(ErrorKind::Allocation, offset);
    let id = NodeId(nodes.len());
    let (link, parent_link) = (Link::new(id), Link::new(parent));
    nodes.try_reserve(1).map_err(|_| allocation)?;
    let previous_sibling = nodes[parent.0].last_child;
    nodes.push(OwnedRecord {
        id,
        content,
        source,
        parent: parent_link,
        first_child: Link::NONE,
        last_child: Link::NONE,
        previous_sibling,
        next_sibling: Link::NONE,
        subtree_size: 1,
    });
    if let Some(previous) = previous_sibling.get() {
        nodes[previous.0].next_sibling = link;
    } else {
        nodes[parent.0].first_child = link;
    }
    nodes[parent.0].last_child = link;
    Ok(id)
}

/// State that only HTML documents carry.
#[derive(Clone, Debug)]
struct HtmlMeta {
    encoding: Option<String>,
    /// Sorted attribute-arena indices of attributes written without a value.
    valueless: Vec<usize>,
}

/// An element's last child other than comments, as `areBlanks` inspects it.
pub(crate) enum LastChild<'a> {
    None,
    Text,
    Element(&'a str),
    Other,
}

/// Builds an HTML document from tree-construction calls, in libxml2's SAX2
/// shape: names are stored verbatim (HTML allows names XML rejects), adjacent
/// Text or CDATA appends merge, and nodes carry no source origins. Errors are
/// limit or allocation failures with offset 0; the caller supplies offsets.
pub(crate) struct HtmlBuilder {
    nodes: allocator::Vec<OwnedRecord, Global>,
    attributes: allocator::Vec<StoredAttribute, Global>,
    pool: Pool,
    root_element: NodeId,
    valueless: Vec<usize>,
    budget: Budget,
}

impl HtmlBuilder {
    pub(crate) fn new(source_length: usize, limits: Limits) -> Result<Self, Error> {
        let OwnedBuilder { nodes, .. } = OwnedBuilder::new(source_length, 0, Global)?;
        Ok(Self {
            nodes,
            attributes: allocator::Vec::new_in(Global),
            pool: Pool::default(),
            root_element: NodeId(0),
            valueless: Vec::new(),
            budget: Budget::new(limits),
        })
    }

    fn append(&mut self, parent: NodeId, content: Content) -> Result<NodeId, Error> {
        if self.nodes.len() > self.budget.max_events() {
            return Err(Error::new(ErrorKind::LimitExceeded(Limit::Events), 0));
        }
        append_record(&mut self.nodes, parent, content, None, 0)
    }

    fn name(&mut self, name: &str) -> Result<StoredName, Error> {
        Ok(StoredName {
            qualified: self.pool.store(name, 0, &mut self.budget)?,
            prefix_length: None,
            namespace: None,
        })
    }

    pub(crate) fn element(
        &mut self,
        parent: NodeId,
        name: &str,
        attributes: &[(String, Option<String>)],
    ) -> Result<NodeId, Error> {
        let allocation = Error::new(ErrorKind::Allocation, 0);
        let name = self.name(name)?;
        let start = self.attributes.len();
        self.attributes
            .try_reserve(attributes.len())
            .map_err(|_| allocation)?;
        for (attribute, value) in attributes {
            if value.is_none() {
                self.valueless.try_reserve(1).map_err(|_| allocation)?;
                self.valueless.push(self.attributes.len());
            }
            let name = self.name(attribute)?;
            let value = self
                .pool
                .store(value.as_deref().unwrap_or(""), 0, &mut self.budget)?;
            self.attributes.push(StoredAttribute { name, value });
        }
        let attributes = start..self.attributes.len();
        let id = self.append(parent, Content::Element { name, attributes })?;
        if parent == NodeId(0) && self.root_element == NodeId(0) {
            self.root_element = id;
        }
        Ok(id)
    }

    /// Appends Text or CDATA, merging into a last child of the same kind.
    /// Returns false, changing nothing, if a merge would exceed `limit` bytes.
    pub(crate) fn text(
        &mut self,
        parent: NodeId,
        value: &str,
        cdata: bool,
        limit: usize,
    ) -> Result<bool, Error> {
        let last = self.nodes[parent.0].last_child.get();
        let merge = last.and_then(|last| match &self.nodes[last.0].content {
            Content::Text(range) if !cdata => Some((last, range.clone())),
            Content::Cdata(range) if cdata => Some((last, range.clone())),
            _ => None,
        });
        let Some((last, range)) = merge else {
            let range = self.pool.store(value, 0, &mut self.budget)?;
            let content = if cdata {
                Content::Cdata(range)
            } else {
                Content::Text(range)
            };
            self.append(parent, content)?;
            return Ok(true);
        };
        if value.len() > limit || range.len() > limit - value.len() {
            return Ok(false);
        }
        let range = self.pool.extend(range, value, 0, &mut self.budget)?;
        self.nodes[last.0].content = if cdata {
            Content::Cdata(range)
        } else {
            Content::Text(range)
        };
        Ok(true)
    }

    pub(crate) fn comment(&mut self, parent: NodeId, value: &str) -> Result<(), Error> {
        let range = self.pool.store(value, 0, &mut self.budget)?;
        self.append(parent, Content::Comment(range)).map(drop)
    }

    /// Ends an element: everything appended since it is its subtree.
    pub(crate) fn close(&mut self, id: NodeId) {
        self.nodes[id.0].subtree_size = self.nodes.len() - id.0;
    }

    pub(crate) fn last_child(&self, parent: NodeId) -> LastChild<'_> {
        let mut child = self.nodes[parent.0].last_child.get();
        while let Some(id) = child {
            let record = &self.nodes[id.0];
            match &record.content {
                Content::Comment(_) => child = record.previous_sibling.get(),
                Content::Text(_) => return LastChild::Text,
                Content::Element { name, .. } => {
                    return LastChild::Element(&self.pool.0[name.qualified.clone()]);
                }
                _ => return LastChild::Other,
            }
        }
        LastChild::None
    }

    /// Completes the document. libxml2 makes the DOCTYPE the document's first
    /// child wherever it appeared (or was implied); an empty name stands for
    /// a DOCTYPE without one.
    pub(crate) fn finish(
        mut self,
        doctype: Option<Doctype<'_>>,
        encoding: Option<String>,
    ) -> Result<OwnedDocument, Error> {
        let mut preorder = true;
        if let Some(doctype) = doctype {
            let mut store = |value: Option<&str>| {
                value
                    .map(|value| self.pool.store(value, 0, &mut self.budget))
                    .transpose()
            };
            let content = Content::DocumentType(StoredDoctype {
                name: store(Some(doctype.name))?.unwrap_or_default(),
                public_id: store(doctype.public_id)?,
                system_id: store(doctype.system_id)?,
                internal_subset: None,
            });
            let first = self.nodes[0].first_child;
            let id = self.append(NodeId(0), content)?;
            if let Some(first) = first.get() {
                // Move the new last child to the front.
                let link = Link(id.0);
                let previous = self.nodes[id.0].previous_sibling;
                self.nodes[previous.0].next_sibling = Link::NONE;
                self.nodes[0].last_child = previous;
                self.nodes[id.0].previous_sibling = Link::NONE;
                self.nodes[id.0].next_sibling = Link(first.0);
                self.nodes[first.0].previous_sibling = link;
                self.nodes[0].first_child = link;
                preorder = false;
            }
        }
        self.nodes[0].subtree_size = self.nodes.len();
        let mut document = OwnedDocument {
            identity: document_identity(&LAST_DOCUMENT_ID)?,
            nodes: self.nodes,
            attributes: self.attributes,
            kept: allocator::Vec::new_in(Global),
            pool: self.pool,
            root_element: self.root_element,
            declaration: None,
            edit_owned_bytes: 0,
            edit_created_nodes: 0,
            edit_created_attributes: 0,
            slots_are_preorder: preorder,
            html: Some(Box::new(HtmlMeta {
                encoding,
                valueless: self.valueless,
            })),
            ids: Vec::new(),
            dtd: None,
            validity: Vec::new(),
            errors: Vec::new(),
            url: None,
        };
        // xmlIsID for HTML: any valued `id` attribute, and `name` on `a`.
        document.ids = document
            .nodes()
            .filter(|node| node.kind() == OwnedNodeKind::Element)
            .flat_map(|node| {
                let anchor = node.qualified_name() == Some("a");
                node.attributes()
                    .enumerate()
                    .filter(move |(index, attribute)| {
                        let name = attribute.name.qualified();
                        node.attribute_has_value(*index)
                            && (name == "id" || anchor && name == "name")
                    })
                    .map(move |(index, _)| (node.id(), index))
            })
            .collect();
        document.ids.sort_unstable();
        Ok(document)
    }
}

/// A read-only view bounded by its owned document's lifetime.
#[derive(Debug)]
pub struct OwnedNode<'document, A: Allocator = Global> {
    document: &'document OwnedDocument<A>,
    record: &'document OwnedRecord,
}

// Derives would require `A: Copy`; a view only copies two references.
impl<A: Allocator> Clone for OwnedNode<'_, A> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<A: Allocator> Copy for OwnedNode<'_, A> {}

impl<'document, A: Allocator> OwnedNode<'document, A> {
    pub(crate) fn document(self) -> &'document OwnedDocument<A> {
        self.document
    }

    #[inline]
    fn record(self) -> &'document OwnedRecord {
        self.record
    }

    pub fn id(self) -> OwnedNodeId {
        OwnedNodeId {
            document: self.document.identity,
            slot: self.record.id,
        }
    }

    #[inline]
    pub fn kind(self) -> OwnedNodeKind {
        match &self.record().content {
            Content::Removed => unreachable!("owned node is live"),
            Content::Document => OwnedNodeKind::Document,
            Content::DocumentType(_) => OwnedNodeKind::DocumentType,
            Content::Element { .. } => OwnedNodeKind::Element,
            Content::Text(_) => OwnedNodeKind::Text,
            Content::Cdata(_) => OwnedNodeKind::Cdata,
            Content::Comment(_) => OwnedNodeKind::Comment,
            Content::ProcessingInstruction { .. } => OwnedNodeKind::ProcessingInstruction,
            Content::EntityRef(_) => OwnedNodeKind::EntityRef,
        }
    }

    #[inline]
    pub fn name(self) -> Option<Name<'document>> {
        match &self.record().content {
            Content::Element { name, .. } => Some(name.view(&self.document.pool.0)),
            _ => None,
        }
    }

    /// The element's lexical name, borrowed for the document's lifetime.
    pub(crate) fn qualified_name(self) -> Option<&'document str> {
        match &self.record().content {
            Content::Element { name, .. } => Some(&self.document.pool.0[name.qualified.clone()]),
            _ => None,
        }
    }

    /// Returns the direct Text, CDATA, comment or PI payload, without joining
    /// descendant text. Elements and the document root return None.
    #[inline]
    pub fn text(self) -> Option<&'document str> {
        let range = match &self.record().content {
            Content::Text(range) | Content::Cdata(range) | Content::Comment(range) => range,
            Content::ProcessingInstruction { data, .. } => data,
            _ => return None,
        };
        Some(&self.document.pool.0[range.clone()])
    }

    /// Visits the node's string value as borrowed chunks without allocating.
    ///
    /// Documents and elements yield descendant Text/CDATA payloads in document
    /// order, ignoring comments and PIs. Leaves yield their own payload once,
    /// including an empty payload. Adjacent and empty Text/CDATA nodes remain
    /// distinct chunks; `text()` still returns only a direct leaf payload.
    /// An iterator's shared borrow prevents editing while it remains in use:
    ///
    /// ```compile_fail
    /// use yaxml::{EditLimits, OwnedDocument};
    /// let mut document = OwnedDocument::parse("<r>old</r>").unwrap();
    /// let root = document.root_element().id();
    /// let mut chunks = document.root_element().text_chunks();
    /// document.edit(EditLimits::default()).unwrap().append_text(root, "new").unwrap();
    /// assert_eq!(chunks.next(), Some("old"));
    /// ```
    pub fn text_chunks(self) -> impl Iterator<Item = &'document str> + 'document {
        let descendants = self
            .descendants()
            .skip(1)
            .filter_map(|node| match node.kind() {
                OwnedNodeKind::Text | OwnedNodeKind::Cdata => node.text(),
                _ => None,
            });
        self.text().into_iter().chain(descendants)
    }

    /// The entity an entity-reference node names, or `None` for other kinds.
    pub fn entity_name(self) -> Option<&'document str> {
        match &self.record().content {
            Content::EntityRef(name) => Some(&self.document.pool.0[name.clone()]),
            _ => None,
        }
    }

    /// Returns a document type node's declaration.
    pub fn doctype(self) -> Option<Doctype<'document>> {
        match &self.record().content {
            Content::DocumentType(doctype) => Some(doctype.view(&self.document.pool.0)),
            _ => None,
        }
    }

    /// The element's QName and namespace, borrowed from the pool.
    pub(crate) fn name_parts(self) -> Option<(&'document str, Option<&'document str>)> {
        match &self.record().content {
            Content::Element { name, .. } => Some(self.document.name_parts(name)),
            _ => None,
        }
    }

    /// One attribute's QName, namespace and value by source index.
    pub(crate) fn attribute_parts(
        self,
        index: usize,
    ) -> Option<((&'document str, Option<&'document str>), &'document str)> {
        let Content::Element { attributes, .. } = &self.record().content else {
            return None;
        };
        let attribute = self.document.attributes[attributes.clone()].get(index)?;
        Some((
            self.document.name_parts(&attribute.name),
            &self.document.pool.0[attribute.value.clone()],
        ))
    }

    pub fn processing_instruction_target(self) -> Option<&'document str> {
        match &self.record().content {
            Content::ProcessingInstruction { target, .. } => {
                Some(&self.document.pool.0[target.clone()])
            }
            _ => None,
        }
    }

    /// Visits attributes in source order, including namespace declarations.
    /// Each view borrows the pool without allocating.
    #[inline]
    pub fn attributes(self) -> impl ExactSizeIterator<Item = Attribute<'document>> + 'document {
        let range = match &self.record().content {
            Content::Element { attributes, .. } => attributes.clone(),
            _ => 0..0,
        };
        range.map(move |index| self.document.attributes[index].view(&self.document.pool.0))
    }

    /// False for an HTML attribute written without `=value` (libxml2 stores no
    /// value; its view reads as ""), and for an out-of-range position.
    pub fn attribute_has_value(self, position: usize) -> bool {
        let Content::Element { attributes, .. } = &self.record().content else {
            return false;
        };
        let index = attributes.start.saturating_add(position);
        index < attributes.end
            && self
                .document
                .html
                .as_ref()
                .is_none_or(|html| html.valueless.binary_search(&index).is_err())
    }

    /// The attribute at `position` as libxml2 keeps it when parsing without
    /// `NOENT` (the attribute's text and entity-reference children, which
    /// `save` writes): `&name;` for each kept general entity reference, `&#38;`
    /// for a literal `&`, other references resolved. `None` when it kept no
    /// reference, the usual case; [`attributes`](Self::attributes) always has
    /// the substituted value, which XPath, validation and C14N use.
    ///
    /// ```
    /// use yaxml::{OwnedDocument, ParseOptions, options::NOENT};
    /// let xml = b"<!DOCTYPE r [<!ENTITY e 'x'>]><r a='1&e;&amp;' b='2'/>";
    /// let document = OwnedDocument::parse_with(xml, ParseOptions::default())?;
    /// let root = document.root_element();
    /// assert_eq!(root.attributes().next().unwrap().value, "1x&");
    /// assert_eq!(root.unexpanded_attribute(0), Some("1&e;&#38;"));
    /// assert_eq!(root.unexpanded_attribute(1), None);
    /// let substituted = OwnedDocument::parse_with(xml, ParseOptions::new(NOENT))?;
    /// assert_eq!(substituted.root_element().unexpanded_attribute(0), None);
    /// # Ok::<(), yaxml::Error>(())
    /// ```
    pub fn unexpanded_attribute(self, position: usize) -> Option<&'document str> {
        let Content::Element { attributes, .. } = &self.record().content else {
            return None;
        };
        let kept = &self.document.kept;
        let index = attributes.clone().nth(position)?;
        let found = kept
            .binary_search_by_key(&index, |(index, _)| *index)
            .ok()?;
        Some(&self.document.pool.0[kept[found].1.clone()])
    }

    /// Original source location, independent of current logical content and
    /// pool storage. Parsed nodes retain their origins; constructed nodes have
    /// no source provenance. This cannot index the owned string pool.
    #[inline]
    pub fn range(self) -> Option<Range<usize>> {
        self.origin().and_then(SourceOrigin::input_range)
    }

    /// Parsed origin; constructed nodes have no parsed provenance.
    #[inline]
    pub fn origin(self) -> Option<SourceOrigin> {
        self.record().source
    }

    pub fn parent(self) -> Option<Self> {
        self.record()
            .parent
            .get()
            .and_then(|id| self.document.view(id))
    }

    pub fn first_child(self) -> Option<Self> {
        self.record()
            .first_child
            .get()
            .and_then(|id| self.document.view(id))
    }

    pub fn last_child(self) -> Option<Self> {
        self.record()
            .last_child
            .get()
            .and_then(|id| self.document.view(id))
    }

    pub fn next_sibling(self) -> Option<Self> {
        self.record()
            .next_sibling
            .get()
            .and_then(|id| self.document.view(id))
    }

    pub fn previous_sibling(self) -> Option<Self> {
        self.record()
            .previous_sibling
            .get()
            .and_then(|id| self.document.view(id))
    }

    pub fn children(self) -> impl Iterator<Item = Self> + 'document {
        std::iter::successors(self.first_child(), |node| node.next_sibling())
    }

    /// `xmlGetProp`: the value of the first attribute whose name (local name
    /// when namespaced) is `name`, in any namespace, else the DTD's default
    /// for it; namespace declarations are not attributes.
    pub fn attribute(self, name: &str) -> Option<&'document str> {
        tree::property(self.document, self.id(), name, None)
    }

    /// `xmlGetNsProp`: the attribute `local` in `namespace` (`None`: no
    /// namespace), else the DTD's default for it.
    pub fn attribute_ns(self, local: &str, namespace: Option<&str>) -> Option<&'document str> {
        tree::property(self.document, self.id(), local, Some(namespace))
    }

    /// `xmlNodeGetContent`: the payload of a leaf, or the concatenated
    /// Text/CDATA of a document, element or entity reference; `None` for the
    /// DOCTYPE. [`text_chunks`](Self::text_chunks) visits it without allocating.
    pub fn content(self) -> Option<String> {
        let doctype = self.kind() == OwnedNodeKind::DocumentType;
        (!doctype).then(|| self.text_chunks().collect())
    }

    /// `xmlNodeListGetString`: the Text and CDATA of this node and its
    /// following siblings, other nodes skipped. With `substitute` an entity
    /// reference adds its content; without, it is `&name;` and the text is
    /// escaped as `xmlEscapeText` does (non-ASCII as `&#xH;` unless the
    /// declaration names an encoding; only `&<>` in HTML).
    pub fn list_string(self, substitute: bool) -> String {
        let document = self.document;
        let ascii = document.declaration().is_none_or(|d| d.encoding.is_none());
        let escape = (!substitute).then_some((document.is_html(), ascii));
        tree::list_string(document, self.id(), escape)
    }

    /// `xmlGetNodePath`: a location path such as `/r/p:a[2]/text()`; `None`
    /// for the DOCTYPE, entity references and their content. A detached
    /// subtree's path starts at its root.
    pub fn path(self) -> Option<String> {
        tree::path(self.document, self.id())
    }

    /// `xmlNodeGetBase`: `xml:base` values from this node outwards resolved
    /// against each other and the document [`url`](OwnedDocument::url); in an
    /// HTML document, the `href` of the first `<base>` in `<html>`/`<head>`.
    pub fn base_uri(self) -> Option<String> {
        if !self.document.is_html() {
            return tree::base(self.document, self.id(), self.document.url());
        }
        let mut node = self.document.root().first_child();
        while let Some(current) = node {
            node = match current.qualified_name() {
                Some(name) if name.eq_ignore_ascii_case("base") => {
                    return current.attribute_ns("href", None).map(str::to_owned);
                }
                Some(name)
                    if ["html", "head"]
                        .iter()
                        .any(|n| name.eq_ignore_ascii_case(n)) =>
                {
                    current.first_child()
                }
                _ => current.next_sibling(),
            };
        }
        None
    }

    /// `xmlNodeGetLang`: the nearest `xml:lang`.
    pub fn lang(self) -> Option<&'document str> {
        tree::lang(self.document, self.id())
    }

    /// `xmlNodeGetSpacePreserve`: the nearest `xml:space` of an element,
    /// true for `preserve`, false for `default`; other values are skipped.
    pub fn space_preserve(self) -> Option<bool> {
        tree::space_preserve(self.document, self.id())
    }

    /// `xmlSearchNs`: the URI `prefix` (`None`: the default namespace) is
    /// bound to here; `Some("")` under `xmlns=""`. `xml` is always bound.
    pub fn search_ns(self, prefix: Option<&str>) -> Option<&'document str> {
        tree::search_ns(self.document, self.id(), prefix)
    }

    /// `xmlSearchNsByHref`: the innermost in-scope binding of `uri`.
    pub fn search_ns_by_href(self, uri: &str) -> Option<NamespaceBinding<'document>> {
        tree::search_ns_by_href(self.document, self.id(), uri)
    }

    /// `xmlGetNsList`: every in-scope declaration, innermost first.
    pub fn namespaces(self) -> Vec<NamespaceBinding<'document>> {
        tree::ns_list(self.document, self.id())
    }

    /// Visits this node and its descendants in preorder without recursion.
    #[inline]
    pub fn descendants(self) -> impl ExactSizeIterator<Item = Self> + 'document {
        let count = self.record().subtree_size;
        if self.document.slots_are_preorder {
            Descendants::Contiguous {
                document: self.document,
                slots: self.document.nodes[self.record.id.0..self.record.id.0 + count].iter(),
            }
        } else {
            Descendants::Linked(LinkedDescendants {
                next: Some(self),
                remaining: count,
            })
        }
    }
}

#[cfg(test)]
#[path = "../tests/internal/owned_text_join_author.rs"]
mod text_join_author_tests;

#[cfg(test)]
#[path = "../tests/internal/owned_text_join_spec.rs"]
mod text_join_spec_tests;

enum Descendants<'document, A: Allocator> {
    Contiguous {
        document: &'document OwnedDocument<A>,
        slots: std::slice::Iter<'document, OwnedRecord>,
    },
    Linked(LinkedDescendants<'document, A>),
}

impl<'document, A: Allocator> Iterator for Descendants<'document, A> {
    type Item = OwnedNode<'document, A>;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Contiguous { document, slots } => {
                slots.next().map(|record| OwnedNode { document, record })
            }
            Self::Linked(iterator) => iterator.next(),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        match self {
            Self::Contiguous { slots, .. } => slots.size_hint(),
            Self::Linked(iterator) => iterator.size_hint(),
        }
    }

    fn count(self) -> usize {
        self.len()
    }
}

impl<A: Allocator> ExactSizeIterator for Descendants<'_, A> {}
impl<A: Allocator> std::iter::FusedIterator for Descendants<'_, A> {}

struct LinkedDescendants<'document, A: Allocator> {
    next: Option<OwnedNode<'document, A>>,
    remaining: usize,
}

impl<'document, A: Allocator> Iterator for LinkedDescendants<'document, A> {
    type Item = OwnedNode<'document, A>;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        let node = self.next?;
        self.remaining -= 1;
        self.next = if self.remaining == 0 {
            None
        } else {
            // Nodes remain, so they follow within the subtree: never past its root.
            node.first_child().or_else(|| {
                std::iter::successors(Some(node), |node| node.parent())
                    .find_map(OwnedNode::next_sibling)
            })
        };
        Some(node)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl<A: Allocator> ExactSizeIterator for LinkedDescendants<'_, A> {}
impl<A: Allocator> std::iter::FusedIterator for LinkedDescendants<'_, A> {}

#[cfg(test)]
#[path = "../tests/internal/owned.rs"]
mod tests;

#[cfg(test)]
#[path = "../tests/internal/editor.rs"]
mod edit_tests;

#[cfg(test)]
#[path = "../tests/internal/owned_structure.rs"]
mod structure_tests;
