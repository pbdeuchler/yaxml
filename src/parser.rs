use std::borrow::Cow;
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::ops::Range;

use memchr::memchr;

use crate::dtd_tag::{Defaulted, TagDeclarations};
use crate::escape::{needs_normalization, normalize_text};
use crate::name::{name_char, name_start, qname, validate_name};
use crate::tokenizer::TokenKind;
use crate::{Attribute, Declaration, Element, Error, ErrorKind, Event, Limit, Limits, Name};

use crate::XML_NAMESPACE as XML;
const XMLNS: &str = "http://www.w3.org/2000/xmlns/";
/// Namespace scopes up to this many bindings are searched linearly.
const INDEXED_SCOPE: usize = 16;
/// Open elements a parser first makes room for.
const FRAMES: usize = 32;

/// Room for one more open-element entry. Doubling from a few entries would
/// reallocate on each level of a fresh parser's first descent.
pub(crate) fn reserve_frame<T>(stack: &mut Vec<T>, offset: usize) -> Result<(), Error> {
    if stack.len() == stack.capacity() {
        stack
            .try_reserve(stack.len().max(FRAMES))
            .map_err(|_| Error::new(ErrorKind::Allocation, offset))?;
    }
    Ok(())
}

/// Attribute counts up to this compare expanded names pairwise.
const SMALL_ATTRIBUTES: usize = 8;

/// libxml2's `XML_MAX_ERRORS`: recovered errors beyond it are not recorded.
const MAX_RECOVERED: usize = 100;

/// Per-parse state every layer shares: accounting for normalized strings and
/// retained structural strings, and the errors recovered from: namespace
/// errors always, every well-formedness error under `RECOVER`.
pub(crate) struct Budget {
    limits: Limits,
    owned_bytes: usize,
    /// Attributes a DOM builder may still retain (`max_total_attributes`).
    attributes_left: usize,
    recovers: bool,
    recovered: Vec<Error>,
    /// The last published start tag's kept-reference attribute forms
    /// (`dtd_tag::kept_form`) by attribute position, for the DOM builders.
    kept: Vec<(usize, String)>,
    /// The last published start tag's declared content
    /// ([`crate::valid::Dtd::has_element_content`]), for `NOBLANKS` streams.
    content: Option<bool>,
}

impl Budget {
    pub(crate) fn new(limits: Limits) -> Self {
        Self {
            limits,
            owned_bytes: 0,
            attributes_left: limits.max_total_attributes,
            recovers: false,
            recovered: Vec::new(),
            kept: Vec::new(),
            content: None,
        }
    }

    /// An error site's decision: strict parsing fails with `error`, except
    /// that a warning or a namespace error is recorded and the site continues with its
    /// repair, as libxml2's `xmlNsErr` does. Under `RECOVER` every
    /// well-formedness error is; resource limits, allocation failure, entity
    /// loops and failed loads stay fatal.
    #[cold]
    #[inline(never)]
    pub(crate) fn recover(&mut self, error: Error) -> Result<(), Error> {
        let fatal = matches!(
            error.kind,
            ErrorKind::LimitExceeded(_)
                | ErrorKind::Allocation
                | ErrorKind::IdentifierExhausted
                | ErrorKind::Aborted
                | ErrorKind::InvalidState
                | ErrorKind::UnsupportedOption
                | ErrorKind::EntityLoop
                | ErrorKind::ExternalResource
                | ErrorKind::XInclude
        );
        if fatal
            || !(self.recovers
                || matches!(error.kind, ErrorKind::Namespace | ErrorKind::Warning(_)))
        {
            return Err(error);
        }
        if self.recovered.len() < MAX_RECOVERED {
            self.recovered
                .try_reserve(1)
                .map_err(|_| Error::new(ErrorKind::Allocation, error.offset))?;
            self.recovered.push(error);
        }
        Ok(())
    }

    pub(crate) fn recovers(&self) -> bool {
        self.recovers
    }

    /// The errors recovered from so far.
    pub(crate) fn errors(&self) -> &[Error] {
        &self.recovered
    }

    /// Maps the errors recovered from since the first `from`.
    pub(crate) fn remap(&mut self, from: usize, map: impl Fn(Error) -> Error) {
        self.recovered[from..]
            .iter_mut()
            .for_each(|error| *error = map(*error));
    }

    /// This budget under `RECOVER`.
    pub(crate) fn recovering(self) -> Self {
        Self {
            recovers: true,
            ..self
        }
    }

    /// How a parse with this budget ended: the errors recovered from,
    /// under `RECOVER` including the terminal `end` error that stopped it.
    /// Strict parsing and fatal errors return `end`'s error.
    pub(crate) fn conclude(
        budget: Option<&mut Self>,
        end: Result<(), Error>,
    ) -> Result<Vec<Error>, Error> {
        let Some(budget) = budget else {
            return end.map(|()| Vec::new());
        };
        if let Err(error) = end {
            if !budget.recovers {
                return Err(error);
            }
            budget.recover(error)?;
        }
        Ok(std::mem::take(&mut budget.recovered))
    }

    /// The last start tag's declared content: element-only (true), mixed or
    /// ANY (false).
    pub(crate) fn content(&self) -> Option<bool> {
        self.content
    }

    pub(crate) fn set_content(&mut self, content: Option<bool>) {
        self.content = content;
    }

    /// Kept-reference forms of the last start tag's attributes; DOM builders drain them.
    pub(crate) fn kept(&mut self) -> &mut Vec<(usize, String)> {
        &mut self.kept
    }

    #[cfg(any(feature = "xinclude", feature = "html"))]
    pub(crate) fn max_events(&self) -> usize {
        self.limits.max_events
    }

    pub(crate) fn remaining_owned(&self) -> usize {
        self.limits.max_owned_bytes - self.owned_bytes
    }

    /// Counts the attributes a DOM builder retains for one start tag.
    pub(crate) fn charge_attributes(&mut self, count: usize, offset: usize) -> Result<(), Error> {
        self.attributes_left = self.attributes_left.checked_sub(count).ok_or(Error::new(
            ErrorKind::LimitExceeded(Limit::TotalAttributes),
            offset,
        ))?;
        Ok(())
    }

    pub(crate) fn charge_owned(&mut self, bytes: usize, offset: usize) -> Result<(), Error> {
        self.owned_bytes = self
            .owned_bytes
            .checked_add(bytes)
            .filter(|total| *total <= self.limits.max_owned_bytes)
            .ok_or(Error::new(
                ErrorKind::LimitExceeded(Limit::OwnedBytes),
                offset,
            ))?;
        Ok(())
    }

    /// Copies a string without tying its result to the temporary token lifetime.
    pub(crate) fn copy<'result>(
        &mut self,
        text: &str,
        offset: usize,
    ) -> Result<Cow<'result, str>, Error> {
        self.charge_owned(text.len(), offset)?;
        copied(text, offset).map(Cow::Owned)
    }
}

/// A copy of `text`, failing at `offset` when it cannot be allocated.
#[inline]
pub(crate) fn copied(text: &str, offset: usize) -> Result<String, Error> {
    let mut copied = String::new();
    copied
        .try_reserve(text.len())
        .map_err(|_| Error::new(ErrorKind::Allocation, offset))?;
    copied.push_str(text);
    Ok(copied)
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
enum NamespaceId {
    Xml,
    Xmlns,
    Declared(usize),
}

struct NamespaceEntry {
    id: NamespaceId,
    bindings: usize,
}

struct Binding<'source> {
    prefix: Cow<'source, str>,
    uri: Cow<'source, str>,
    namespace: Option<NamespaceId>,
    previous: Option<usize>,
}

/// An open element's QName: retained by the frontend, or copied into the
/// shared `frame_names` stack when streaming (one buffer, no per-element string).
enum FrameName<'source> {
    Retained(Cow<'source, str>),
    Pooled(Range<usize>),
}

struct Frame<'source> {
    qualified: FrameName<'source>,
    bindings_start: usize,
}

#[derive(Clone, Copy)]
enum Phase {
    Prolog,
    Content,
    Epilog,
}

/// XML semantics shared by borrowed slices and temporary streaming tokens.
/// Events borrow owned state only when lent: retained namespaces are
/// otherwise copied as needed.
pub(crate) struct ParserState<'source> {
    initial: usize,
    encoding: crate::encoding::SourceEncoding,
    switchable: bool,
    declaration_seen: bool,
    doctype_seen: bool,
    token_limit: usize,
    token_limit_kind: Limit,
    phase: Phase,
    stack: Vec<Frame<'source>>,
    frame_names: String,
    /// Empty attribute list whose capacity a callback frontend returned.
    attribute_storage: Vec<Attribute<'static>>,
    copies_names: bool,
    /// Streaming callers lend each event's retained namespace URIs (see
    /// [`ParserState::lend`]) instead of copying them per name.
    lends_namespaces: bool,
    /// Bindings of the latest closed scope, removed before the next token
    /// so a lent event can still borrow their URIs.
    pending_unwind: Option<usize>,
    /// Whether the latest event has names awaiting their lent URI.
    lent_names: bool,
    bindings: Vec<Binding<'source>>,
    /// Prefix -> active binding, maintained only above `INDEXED_SCOPE`
    /// bindings; smaller scopes search `bindings` from the innermost entry.
    namespace_index: HashMap<Cow<'source, str>, usize>,
    indexed: bool,
    namespace_ids: HashMap<Cow<'source, str>, NamespaceEntry>,
    next_namespace: usize,
    budget: Budget,
    events: usize,
    flags: crate::options::Flags,
    /// Recovery over input with characters outside `Char`, which libxml2
    /// drops from text, replaces in attribute values and ends markup at.
    repairs_characters: bool,
    /// Recovery by a frontend that rereads tokens from `rewind`.
    rereads: bool,
    /// Recovery: where libxml2 reads on, as content, inside the latest token.
    rewind: Option<usize>,
    /// Recovery: the kept reference to an undeclared entity just started
    /// ends before the next token (`take_reference_end`).
    reference_end: bool,
    /// The token being processed comes from entity replacement text: its
    /// line ends were normalized when it was declared, and libxml2 stops at
    /// the markup errors that physical input recovers from.
    replacement: bool,
    /// The latest token's normalized values while lending (see
    /// [`LentValues`]); boxed on first use, since a larger state makes every
    /// construction and move of the slice reader slower (`tiny` reader -9%).
    lent: Option<Box<[LentValues; 1]>>,
    /// `catalog` URLs of `<?oasis-xml-catalog?>` instructions (`CATALOG_PI`)
    /// not yet handed to the resolver; boxed for the same reason as `lent`.
    #[expect(clippy::box_collection, reason = "a smaller state constructs faster")]
    catalogs: Option<Box<Vec<String>>>,
    /// Content of a node (`xmlCtxtParseContent`): no prolog or epilog.
    fragment: bool,
}

/// Normalized values lent in order: each stands in its event as
/// [`LENT_VALUE`] until [`ParserState::lend`].
#[derive(Default)]
struct LentValues {
    values: String,
    ranges: Vec<Range<usize>>,
}

impl<'source> ParserState<'source> {
    pub(crate) fn owned_budget(&mut self) -> &mut Budget {
        &mut self.budget
    }

    pub(crate) fn element_depth(&self) -> usize {
        self.stack.len()
    }

    pub(crate) fn errors(&self) -> &[Error] {
        self.budget.errors()
    }

    pub(crate) fn drops_blanks(&self) -> bool {
        self.flags.drops_blanks()
    }

    pub(crate) fn new(limits: Limits, initial_offset: usize) -> Self {
        Self {
            initial: initial_offset,
            encoding: crate::encoding::SourceEncoding::Utf8,
            switchable: false,
            declaration_seen: false,
            doctype_seen: false,
            token_limit: limits.max_token_bytes,
            token_limit_kind: Limit::TokenBytes,
            phase: Phase::Prolog,
            stack: Vec::new(),
            frame_names: String::new(),
            attribute_storage: Vec::new(),
            copies_names: false,
            lends_namespaces: false,
            pending_unwind: None,
            lent_names: false,
            bindings: Vec::new(),
            namespace_index: HashMap::new(),
            indexed: false,
            namespace_ids: HashMap::new(),
            next_namespace: 0,
            budget: Budget::new(limits),
            events: 0,
            flags: crate::options::Flags::default(),
            repairs_characters: false,
            rereads: false,
            rewind: None,
            reference_end: false,
            replacement: false,
            lent: None,
            catalogs: None,
            fragment: false,
        }
    }

    /// Marks the following tokens as entity replacement text, or not.
    pub(crate) fn set_replacement(&mut self, replacement: bool) {
        self.replacement = replacement;
    }

    /// A markup error that physical input recovers from where libxml2
    /// does; replacement text stops at it.
    fn recover_markup(&mut self, error: Error) -> Result<(), Error> {
        match self.replacement {
            true => Err(error),
            false => self.budget.recover(error),
        }
    }

    /// The token ceiling, of decoded bytes for decoded input.
    pub(crate) fn token_limit(&self) -> usize {
        self.token_limit
    }

    /// A token longer than the token ceiling fails at its `start`.
    pub(crate) fn check_token_limit(&self, length: usize, start: usize) -> Result<(), Error> {
        match length > self.token_limit {
            true => Err(Error::new(
                ErrorKind::LimitExceeded(self.token_limit_kind),
                start,
            )),
            false => Ok(()),
        }
    }

    /// Records the catalog of an `oasis-xml-catalog` instruction when the
    /// flags ask for it (`xmlParseCatalogPI`; a syntax error is ignored).
    #[cfg(feature = "catalog")]
    fn note_catalog(&mut self, target: &str, data: &str) {
        if self.flags.catalog_pi()
            && target == "oasis-xml-catalog"
            && let Some(url) = crate::catalog::pi_catalog(data)
        {
            (self.catalogs.get_or_insert_default()).push(url.to_owned());
        }
    }

    /// The recorded catalog URLs, in document order.
    pub(crate) fn take_catalogs(&mut self) -> Vec<String> {
        self.catalogs.take().map_or_else(Vec::new, |urls| *urls)
    }

    /// A fresh state for `limits` that keeps this one's emptied allocations;
    /// every scalar, counter and budget starts over (`PushParser::reset`).
    pub(crate) fn recycled(self, limits: Limits) -> Self {
        fn emptied<T>(mut vec: Vec<T>) -> Vec<T> {
            vec.clear();
            vec
        }
        let lent = self.lent.map(|mut lent| {
            lent[0].values.clear();
            lent[0].ranges.clear();
            lent
        });
        let (mut index, mut ids, mut names) =
            (self.namespace_index, self.namespace_ids, self.frame_names);
        index.clear();
        ids.clear();
        names.clear();
        Self {
            stack: emptied(self.stack),
            frame_names: names,
            attribute_storage: emptied(self.attribute_storage),
            bindings: emptied(self.bindings),
            namespace_index: index,
            namespace_ids: ids,
            lent,
            ..Self::new(limits, 0)
        }
    }

    /// Recovery by a DTD-free token loop (`rereads`), which rereads a token
    /// from where libxml2 ends it (`take_rewind`) and, over input with
    /// `characters` outside `Char`, repairs them.
    /// Field assignments, not struct-update syntax: rustc 1.95-1.96 miscompile
    /// `Self { rereads, repairs_characters: rereads && characters, ..self }`
    /// in release builds (fixed in 1.97, now the MSRV; tooling/simplify_comparison_scan.py guards regressions).
    pub(crate) fn rereading(mut self, rereads: bool, characters: bool) -> Self {
        self.rereads = rereads && self.recovers();
        self.repairs_characters = self.rereads && characters;
        self
    }

    /// Whether this is a token loop's recovery, which rereads tokens.
    pub(crate) fn rereads(&self) -> bool {
        self.rereads
    }

    /// A streamed token held a character outside `Char`: recovery repairs
    /// from now on, as the slice reader does over such input.
    pub(crate) fn repair_characters(&mut self) {
        self.repairs_characters = true;
    }

    /// Recovery keeps a reference to an undeclared entity in text `raw` at
    /// `start`, as libxml2 does without `NOENT`, as an empty reference: its
    /// start (its end follows, `take_reference_end`), or the text before it.
    /// The rest is reread.
    #[cold]
    fn undeclared<'event>(
        &mut self,
        raw: &'event str,
        start: usize,
    ) -> Result<Result<Event<'event>, &'event str>, Error> {
        let found = memchr::memchr_iter(b'&', raw.as_bytes()).find_map(|at| {
            let rest = &raw[at + 1..];
            let name = &rest[..name_length(rest)];
            let complete = !name.is_empty() && rest[name.len()..].starts_with(';');
            (complete && crate::escape::predefined(name).is_none()).then_some((at, name))
        });
        Ok(match found {
            Some((0, name)) => {
                self.budget
                    .recover(Error::new(ErrorKind::UnknownEntity, start))?;
                (self.rewind, self.reference_end) = (Some(start + name.len() + 2), true);
                Ok(Event::EntityStart(Cow::Borrowed(name)))
            }
            Some((at, _)) => {
                self.rewind = Some(start + at);
                Err(&raw[..at])
            }
            None => Err(raw),
        })
    }

    /// Whether a kept undeclared reference ends before the next token.
    pub(crate) fn reference_ends(&self) -> bool {
        self.reference_end
    }

    /// [`Self::reference_ends`], which the caller now publishes.
    pub(crate) fn take_reference_end(&mut self) -> bool {
        std::mem::take(&mut self.reference_end)
    }

    /// Where a rereading token loop reads the latest token's rest as content.
    pub(crate) fn take_rewind(&mut self) -> Option<usize> {
        self.rewind.take()
    }

    /// Recovery from a token that libxml2 ends at `at`, past `start`, and
    /// whose rest it reads as content.
    fn reread(&mut self, error: Error, start: usize, at: usize) -> Result<(), Error> {
        self.budget.recover(error)?;
        self.rewind = Some(at.max(start + 1));
        Ok(())
    }

    pub(crate) fn set_initial_offset(&mut self, offset: usize) {
        self.initial = offset;
    }

    pub(crate) fn set_encoding(
        &mut self,
        encoding: crate::encoding::SourceEncoding,
        initial: usize,
        decoded_token_limit: usize,
    ) {
        self.encoding = encoding;
        self.set_initial_offset(initial);
        self.token_limit = decoded_token_limit;
        self.token_limit_kind = Limit::DecodedTokenBytes;
    }

    /// Content in a node's context (`xmlCtxtParseContent`): any sequence of
    /// content, no XML declaration, and `bindings` (prefix, URI; `""` is
    /// the default namespace) in scope.
    pub(crate) fn in_context(mut self, bindings: &[(&str, &str)]) -> Result<Self, Error> {
        (self.fragment, self.phase, self.initial) = (true, Phase::Content, usize::MAX);
        for &(prefix, uri) in bindings {
            let mut copy = |raw: &str, offset, budget: &mut Budget| budget.copy(raw, offset);
            self.bind(prefix, None, &Cow::Borrowed(uri), 0, &mut copy)?;
        }
        Ok(self)
    }

    /// Applies the token-level parser options (`NOCDATA`, `NSCLEAN`, `IGNORE_ENC`).
    pub(crate) fn with_flags(mut self, flags: crate::options::Flags) -> Self {
        self.flags = flags;
        self.budget.recovers = flags.recovers();
        self
    }

    /// A CDATA section's event: text under `NOCDATA`.
    fn cdata<'event>(&self, value: Cow<'event, str>) -> Event<'event> {
        match self.flags.cdata_as_text() {
            true => Event::Text(value),
            false => Event::Cdata(value),
        }
    }

    /// Whether `prefix` is already bound to `uri` outside this element, so
    /// `NSCLEAN` drops the declaration.
    fn redundant(&self, prefix: &str, uri: &str) -> bool {
        self.flags.clean_namespaces()
            && self
                .bound(prefix)
                .is_some_and(|index| self.bindings[index].uri == uri)
    }

    /// Streaming frontends copy every retained name. Open element names then
    /// share one charged buffer, and unindexed prefixes are charged directly.
    pub(crate) fn copying_names(mut self) -> Self {
        self.copies_names = true;
        self
    }

    /// Events will be lent: namespace URIs retained by the parser are
    /// borrowed by [`ParserState::lend`] rather than copied per name.
    pub(crate) fn lending_namespaces(mut self) -> Self {
        self.lends_namespaces = true;
        self
    }

    /// Returns to copying namespace URIs, for consumers that never lend.
    pub(crate) fn stop_lending(&mut self) {
        self.settle();
        self.lends_namespaces = false;
    }

    /// Fills the namespace URIs of a name and attributes built while
    /// lending, borrowing them from retained bindings; returns the budget
    /// for delivery.
    #[inline]
    pub(crate) fn lend<'a>(
        &'a mut self,
        name: &mut Name<'a>,
        attributes: &mut [Attribute<'a>],
    ) -> &'a mut Budget {
        let Self {
            bindings,
            namespace_index,
            indexed,
            budget,
            lent_names,
            lent,
            ..
        } = self;
        if let Some([LentValues { values, ranges }]) = lent.as_deref()
            && !ranges.is_empty()
        {
            let mut ranges = ranges.iter();
            for attribute in attributes.iter_mut().filter(|a| is_lent_value(&a.value)) {
                let range = ranges.next().expect("a lent value per placeholder");
                attribute.value = Cow::Borrowed(&values[range.clone()]);
            }
        }
        if *lent_names {
            *lent_names = false;
            let names = std::iter::once(name).chain(attributes.iter_mut().map(|a| &mut a.name));
            for name in names.filter(|name| name.namespace() == Some(LENT)) {
                let prefix = name.prefix().unwrap_or("");
                let index = bound_in(bindings, namespace_index, *indexed, prefix)
                    .expect("lent namespace is in scope");
                name.set_namespace(Some(Cow::Borrowed(bindings[index].uri.as_ref())));
            }
        }
        budget
    }

    /// [`ParserState::lend`] for any event; inlined into each delivery loop.
    #[inline(always)]
    pub(crate) fn lend_event<'a>(&'a mut self, event: &mut Event<'a>) -> &'a mut Budget {
        match event {
            Event::Start(element) | Event::Empty(element) => match &mut element.attributes {
                crate::Attributes::Owned(list) => self.lend(&mut element.name, list),
                crate::Attributes::Borrowed(_) => self.lend(&mut element.name, &mut []),
            },
            Event::End(name) => self.lend(name, &mut []),
            Event::Text(value)
            | Event::Cdata(value)
            | Event::Comment(value)
            | Event::ProcessingInstruction { data: value, .. } => {
                let Self { lent, budget, .. } = self;
                if let Some([LentValues { values, ranges }]) = lent.as_deref()
                    && let Some(range) = ranges.first()
                    && is_lent_value(value)
                {
                    *value = Cow::Borrowed(&values[range.clone()]);
                }
                budget
            }
            _ => &mut self.budget,
        }
    }

    /// Removes the bindings of a scope closed by the previous token.
    /// Releases the previous token's lent values too.
    fn settle(&mut self) {
        if let Some(start) = self.pending_unwind.take() {
            self.unwind_bindings(start);
        }
        if let Some([lent]) = self.lent.as_deref_mut() {
            lent.ranges.clear();
        }
    }

    /// Closes a scope now, or after delivery when its event may be lent.
    fn close_scope(&mut self, start: usize) {
        if self.bindings.len() > start {
            if self.lends_namespaces {
                self.pending_unwind = Some(start);
            } else {
                self.unwind_bindings(start);
            }
        }
    }

    /// The active binding index for `prefix`.
    fn bound(&self, prefix: &str) -> Option<usize> {
        bound_in(&self.bindings, &self.namespace_index, self.indexed, prefix)
    }

    /// The interned identity of an active URI and its binding count (counts
    /// are kept only while indexed; linear scopes search the binding stack).
    fn interned(&self, uri: &str) -> Option<(NamespaceId, usize)> {
        if self.indexed {
            return self
                .namespace_ids
                .get(uri)
                .map(|entry| (entry.id, entry.bindings));
        }
        self.bindings
            .iter()
            .rev()
            .find_map(|binding| match binding.namespace {
                Some(id @ NamespaceId::Declared(_)) if binding.uri == uri => Some((id, 0)),
                _ => None,
            })
    }

    /// Builds the prefix index once a scope outgrows linear search. Keys
    /// were charged when their bindings were retained.
    fn index_bindings(&mut self, offset: usize) -> Result<(), Error> {
        if self.indexed || self.bindings.len() <= INDEXED_SCOPE {
            return Ok(());
        }
        let allocation = || Error::new(ErrorKind::Allocation, offset);
        let key = |value: &Cow<'source, str>| -> Result<Cow<'source, str>, Error> {
            Ok(match value {
                Cow::Borrowed(value) => Cow::Borrowed(*value),
                Cow::Owned(value) => {
                    let mut key = String::new();
                    key.try_reserve(value.len()).map_err(|_| allocation())?;
                    key.push_str(value);
                    Cow::Owned(key)
                }
            })
        };
        let mut index = HashMap::new();
        let mut ids: HashMap<Cow<'source, str>, NamespaceEntry> = HashMap::new();
        index
            .try_reserve(self.bindings.len())
            .map_err(|_| allocation())?;
        ids.try_reserve(self.bindings.len())
            .map_err(|_| allocation())?;
        for (position, binding) in self.bindings.iter().enumerate() {
            index.insert(key(&binding.prefix)?, position);
            if let Some(id @ NamespaceId::Declared(_)) = binding.namespace {
                if let Some(entry) = ids.get_mut(binding.uri.as_ref()) {
                    entry.bindings += 1;
                } else {
                    ids.insert(key(&binding.uri)?, NamespaceEntry { id, bindings: 1 });
                }
            }
        }
        self.namespace_index = index;
        self.namespace_ids = ids;
        self.indexed = true;
        Ok(())
    }

    /// Reuses returned attribute capacity.
    fn take_attributes<'event>(&mut self) -> Vec<Attribute<'event>> {
        emptied(std::mem::take(&mut self.attribute_storage))
    }

    /// Keeps the capacity of a list whose event has been delivered.
    pub(crate) fn recycle_attributes(&mut self, attributes: Vec<Attribute<'static>>) {
        if attributes.capacity() > self.attribute_storage.capacity() {
            self.attribute_storage = attributes;
        }
    }

    fn open_name(&self) -> Option<&str> {
        self.stack.last().map(|frame| match &frame.qualified {
            FrameName::Retained(name) => name.as_ref(),
            FrameName::Pooled(range) => &self.frame_names[range.clone()],
        })
    }

    fn pop_frame(&mut self) -> Frame<'source> {
        let frame = self.stack.pop().expect("matched frame");
        if let FrameName::Pooled(range) = &frame.qualified {
            self.frame_names.truncate(range.start);
        }
        frame
    }

    /// A BOM-less decoding stream can follow a declared byte charset.
    pub(crate) fn allow_switch(&mut self) {
        self.switchable = true;
    }

    pub(crate) fn encoding(&self) -> crate::encoding::SourceEncoding {
        self.encoding
    }

    pub(crate) fn recovers(&self) -> bool {
        self.budget.recovers()
    }

    pub(crate) fn in_content(&self) -> bool {
        matches!(self.phase, Phase::Content)
    }

    /// The lending driver must skip non-events before borrowing its token for
    /// the return value. These bytes are necessarily valid UTF-8/XML whitespace.
    pub(crate) fn is_non_event(&self, kind: TokenKind, bytes: &[u8]) -> bool {
        !matches!(kind, TokenKind::Dtd | TokenKind::InvalidMarkup)
            && (bytes.is_empty()
                || (matches!(kind, TokenKind::Text)
                    && !self.in_content()
                    && bytes.iter().all(|byte| space(*byte))))
    }

    /// Tokens `max_events` still allows, elements `max_depth` still allows
    /// open, and owned bytes `max_owned_bytes` still allows.
    pub(crate) fn headroom(&self) -> (usize, usize, usize) {
        let limits = &self.budget.limits;
        let depth = limits.max_depth.saturating_sub(self.stack.len());
        let owned = limits.max_owned_bytes - self.budget.owned_bytes;
        (limits.max_events - self.events, depth, owned)
    }

    /// Counts `events` tokens published without parsing them.
    pub(crate) fn skip_events(&mut self, events: usize) {
        self.events += events;
    }

    /// The default namespace's binding in scope, unless it is undeclared.
    pub(crate) fn default_namespace(&self) -> Option<&Cow<'source, str>> {
        let index = self.bound("")?;
        Some(&self.bindings[index].uri).filter(|uri| !uri.is_empty())
    }

    pub(crate) fn check_event_limit(&self, offset: usize) -> Result<(), Error> {
        if self.events == self.budget.limits.max_events {
            Err(Error::new(ErrorKind::LimitExceeded(Limit::Events), offset))
        } else {
            Ok(())
        }
    }

    pub(crate) fn check_doctype_start(&self, offset: usize) -> Result<(), Error> {
        if self.doctype_seen || !matches!(self.phase, Phase::Prolog) {
            return Err(Error::new(ErrorKind::Syntax, offset));
        }
        self.check_event_limit(offset)
    }

    pub(crate) fn commit_doctype(&mut self) {
        debug_assert!(self.events < self.budget.limits.max_events);
        self.doctype_seen = true;
        self.events += 1;
    }

    pub(crate) fn validate_finish(&self, offset: usize) -> Result<(), Error> {
        match self.phase {
            Phase::Prolog => Err(Error::new(ErrorKind::MissingRoot, offset)),
            Phase::Content if self.fragment && self.stack.is_empty() => Ok(()),
            Phase::Content => Err(Error::new(ErrorKind::UnexpectedEof, offset)),
            Phase::Epilog => Ok(()),
        }
    }

    pub(crate) fn check_token_start(&self, kind: TokenKind, offset: usize) -> Result<(), Error> {
        match kind {
            TokenKind::Start if matches!(self.phase, Phase::Epilog) => {
                Err(Error::new(ErrorKind::MultipleRoots, offset))
            }
            TokenKind::Start if self.element_depth() >= self.budget.limits.max_depth => {
                Err(Error::new(ErrorKind::LimitExceeded(Limit::Depth), offset))
            }
            TokenKind::Cdata if !self.in_content() => {
                Err(Error::new(ErrorKind::TextOutsideRoot, offset))
            }
            _ => Ok(()),
        }
    }

    /// `retain` borrows slice input or copies streaming structural strings.
    /// Token bytes are already UTF-8 and XML-character validated by the frontend.
    #[inline(always)]
    pub(crate) fn process_token<'event, F>(
        &mut self,
        kind: TokenKind,
        raw: &'event str,
        range: Range<usize>,
        retain: F,
    ) -> Result<Option<Event<'event>>, Error>
    where
        'source: 'event,
        F: FnMut(&'event str, usize, &mut Budget) -> Result<Cow<'source, str>, Error>,
    {
        self.read_token::<false, F>(kind, raw, range, retain)
    }

    /// [`ParserState::process_token`] for the slice reader, whose recovery
    /// rereads tokens when `REREADS` ([`ParserState::rereads`]); strict
    /// reading and the other frontends compile without its checks.
    pub(crate) fn read_token<'event, const REREADS: bool, F>(
        &mut self,
        kind: TokenKind,
        raw: &'event str,
        range: Range<usize>,
        mut retain: F,
    ) -> Result<Option<Event<'event>>, Error>
    where
        'source: 'event,
        F: FnMut(&'event str, usize, &mut Budget) -> Result<Cow<'source, str>, Error>,
    {
        self.settle();
        let start = range.start;
        if matches!(kind, TokenKind::Dtd | TokenKind::InvalidMarkup) {
            self.check_event_limit(start)?;
            self.markup_as_text(Error::new(ErrorKind::Syntax, start))?;
        }
        if self.is_non_event(kind, raw.as_bytes()) {
            return Ok(None);
        }
        if matches!(kind, TokenKind::Text) && !self.in_content() {
            return match raw.bytes().position(|byte| !space(byte)) {
                Some(relative) => Err(Error::new(ErrorKind::TextOutsideRoot, start + relative)),
                None => Err(Error::new(ErrorKind::TextOutsideRoot, start)),
            };
        }
        self.check_event_limit(start)?;
        self.check_token_start(kind, start)?;
        // Every arm yields the result type returned here, so a tag's event
        // passes through without being re-laid-out.
        let event = match kind {
            TokenKind::Start => {
                self.start_element::<REREADS, false, F>(raw, start, &mut retain, None)
            }
            TokenKind::End => self.end_element(raw, start),
            TokenKind::Text => 'text: {
                let raw = match REREADS && !self.flags.substitutes() {
                    true => match self.undeclared(raw, start)? {
                        Ok(reference) => break 'text Ok(Some(reference)),
                        Err(text) => text,
                    },
                    false => raw,
                };
                if let Some(relative) = crate::tokenizer::find(raw.as_bytes(), b"]]>") {
                    self.recover_markup(Error::new(ErrorKind::Syntax, start + relative))?;
                }
                let text = self.normalized::<REREADS>(raw, start, false, true, true)?;
                Ok(Some(Event::Text(text)))
            }
            // libxml2 ends a comment, PI or CDATA section at a character
            // outside `Char` and reads on from there.
            TokenKind::Comment | TokenKind::Cdata | TokenKind::Instruction
                if REREADS
                    && self.repairs_characters
                    && let Some(at) = crate::escape::invalid_char(raw) =>
            {
                let error = Error::new(ErrorKind::InvalidCharacter, start + at);
                self.reread(error, start, start + at)?;
                Ok(None)
            }
            TokenKind::Comment => {
                let body = delimited_body(raw, "<!--", "-->", start)?;
                if crate::tokenizer::find(body.as_bytes(), b"--").is_some() || body.ends_with('-') {
                    self.recover_markup(Error::new(ErrorKind::Syntax, start))?;
                }
                Ok(Some(Event::Comment(self.normalized::<REREADS>(
                    body,
                    start + 4,
                    false,
                    false,
                    true,
                )?)))
            }
            TokenKind::Cdata => {
                if !self.in_content() {
                    return Err(Error::new(ErrorKind::TextOutsideRoot, start));
                }
                let body = delimited_body(raw, "<![CDATA[", "]]>", start)?;
                let value = self.normalized::<REREADS>(body, start + 9, false, false, true)?;
                Ok(Some(self.cdata(value)))
            }
            TokenKind::Instruction => self.instruction::<REREADS>(raw, start),
            TokenKind::Dtd | TokenKind::InvalidMarkup => self.text_after::<REREADS>(raw, 1, start),
        };
        if event.is_ok() {
            self.events += 1;
        }
        event
    }

    /// Markup that starts nothing (`<!x`, a misplaced DOCTYPE, a tag without
    /// a name): strict parsing fails, as does recovery outside the root,
    /// where libxml2 stops. Inside it libxml2 reads it as text after `<`.
    fn markup_as_text(&mut self, error: Error) -> Result<(), Error> {
        match self.in_content() {
            true => self.recover_markup(error),
            false => Err(error),
        }
    }

    /// The text of `raw` after its `prefix` bytes, which a rereading
    /// frontend reads as content.
    fn text_after<'event, const REREADS: bool>(
        &mut self,
        raw: &'event str,
        prefix: usize,
        start: usize,
    ) -> Result<Option<Event<'event>>, Error> {
        if REREADS {
            self.rewind = Some(start + prefix);
            return Ok(None);
        }
        let text = raw.get(prefix..).unwrap_or_default();
        let text = self.normalized::<REREADS>(text, start + prefix, false, true, true)?;
        Ok(Some(Event::Text(text)))
    }

    /// A value after XML normalization; `REPAIRS`: the frontend may repair
    /// characters ([`ParserState::rereading`]). A changed value is `lent`
    /// from the parser's storage when events are (see `LENT_VALUE`).
    /// Inlined: most values need no normalization and cost only the scan.
    #[inline(always)]
    fn normalized<'event, const REPAIRS: bool>(
        &mut self,
        raw: &'event str,
        offset: usize,
        attribute: bool,
        references: bool,
        lent: bool,
    ) -> Result<Cow<'event, str>, Error> {
        let repairs = REPAIRS && self.repairs_characters;
        if repairs || needs_normalization(raw, attribute, references) {
            let lent = lent && self.lends_namespaces;
            return self.normalize(raw, offset, (attribute, references), repairs, lent);
        }
        Ok(Cow::Borrowed(raw))
    }

    /// [`ParserState::normalized`] of a value that changes or, when
    /// `repairs`, may have characters to repair.
    #[inline(never)]
    fn normalize<'event>(
        &mut self,
        raw: &'event str,
        offset: usize,
        (attribute, references): (bool, bool),
        repairs: bool,
        lent: bool,
    ) -> Result<Cow<'event, str>, Error> {
        if repairs {
            if let Some(repaired) = self.repaired(raw, offset, attribute, references)? {
                return Ok(Cow::Owned(repaired));
            }
            if !needs_normalization(raw, attribute, references) {
                return Ok(Cow::Borrowed(raw));
            }
        }
        self.budget.charge_owned(raw.len(), offset)?;
        let recover = |error| self.budget.recover(error);
        if lent {
            // Normalization never lengthens a value, so the storage stays
            // within the token's size.
            let lent = match &mut self.lent {
                Some(lent) => &mut lent[0],
                lent => &mut lent.insert(boxed(LentValues::default(), offset)?)[0],
            };
            if lent.ranges.is_empty() {
                lent.values.clear();
            }
            let start = lent.values.len();
            normalize_text(
                raw,
                offset,
                (attribute, references, !self.replacement),
                &mut lent.values,
                recover,
            )?;
            lent.ranges
                .try_reserve(1)
                .map_err(|_| Error::new(ErrorKind::Allocation, offset))?;
            lent.ranges.push(start..lent.values.len());
            return Ok(Cow::Borrowed(LENT_VALUE));
        }
        let mut value = String::new();
        let options = (attribute, references, !self.replacement);
        normalize_text(raw, offset, options, &mut value, recover)?;
        Ok(Cow::Owned(value))
    }

    /// `normalized` over text with characters outside `Char`, which
    /// recovery repairs first (see `repairs_characters`); `None` without any.
    #[cold]
    fn repaired(
        &mut self,
        raw: &str,
        offset: usize,
        attribute: bool,
        references: bool,
    ) -> Result<Option<String>, Error> {
        if crate::escape::invalid_char(raw).is_none() {
            return Ok(None);
        }
        self.budget.charge_owned(raw.len(), offset)?;
        let mut recover = |error| self.budget.recover(error);
        let repaired = crate::escape::repair_characters(raw, offset, attribute, &mut recover)?;
        // Offsets into the repaired text are kept within the original.
        let end = offset + raw.len();
        let within = |error: Error| Error::new(error.kind, error.offset.min(end));
        let recover = |error| recover(within(error));
        let mut value = String::new();
        let options = (attribute, references, true);
        normalize_text(&repaired, offset, options, &mut value, recover).map_err(within)?;
        Ok(Some(value))
    }

    fn instruction<'event, const REREADS: bool>(
        &mut self,
        raw: &'event str,
        start: usize,
    ) -> Result<Option<Event<'event>>, Error> {
        let body = delimited_body(raw, "<?", "?>", start)?;
        let mut name_end = body.bytes().position(space).unwrap_or(body.len());
        match validate_name(&body[..name_end], start + 2, false) {
            Ok(()) => {}
            // A colon in a valid Name is a namespace error.
            Err(error) if validate_name(&body[..name_end], start + 2, true).is_ok() => {
                let error = Error::new(ErrorKind::Namespace, error.offset);
                self.budget.recover(error)?;
            }
            // libxml2 targets the longest Name there, colons included, and
            // reads a PI without one as text after `<?`.
            Err(error) => {
                name_end = name_length(body);
                if name_end == 0 {
                    self.markup_as_text(error)?;
                    return self.text_after::<REREADS>(raw, 2, start);
                }
                self.recover_markup(error)?;
            }
        }
        let target = &body[..name_end];
        if target == "xml" && start == self.initial && !self.replacement {
            let (mut declaration, encoding_offset) = parse_declaration(
                &body[name_end..],
                start + 2 + name_end,
                false,
                &mut |error| self.budget.recover(error),
            )?;
            // An auto-detected encoding (a BOM, or the UTF-16 `<?` signature)
            // is kept, as in libxml2's `xmlSetDeclaredEncoding`: a label
            // outside its allowed set only warns and records the detected
            // name. `IGNORE_ENC` keeps any detected encoding and records no
            // label for BOM-less UTF-8. Recovery from a label it cannot
            // switch to also drops it.
            use crate::encoding::SourceEncoding::{Utf8, Utf16};
            let offset = encoding_offset.unwrap_or(start);
            let auto: Option<(&[&str], &str)> = match self.encoding {
                Utf8 if start != 0 => Some((&["UTF-8", "UTF8"], "UTF-8")),
                Utf16 {
                    little_endian: true,
                    ..
                } => Some((&["UTF-16", "UTF-16LE", "UTF16"], "UTF-16LE")),
                Utf16 { .. } => Some((&["UTF-16", "UTF-16BE", "UTF16"], "UTF-16BE")),
                _ => None,
            };
            self.encoding = match declaration.encoding {
                Some(label) if auto.is_some() || self.flags.ignores_encoding() => {
                    declaration.encoding = match auto {
                        Some((allowed, _))
                            if allowed.iter().any(|a| a.eq_ignore_ascii_case(label)) =>
                        {
                            Some(label)
                        }
                        Some((_, name)) => {
                            let warning = ErrorKind::Warning(crate::Warning::EncodingMismatch);
                            self.budget.recover(Error::new(warning, offset))?;
                            Some(name)
                        }
                        None => (self.encoding != Utf8).then_some(label),
                    };
                    self.encoding
                }
                label => match validate_encoding(self.encoding, self.switchable, label, offset) {
                    Ok(encoding) => encoding,
                    Err(error) => {
                        self.budget.recover(error)?;
                        declaration.encoding = None;
                        self.encoding
                    }
                },
            };
            self.declaration_seen = true;
            return Ok(Some(Event::Declaration(declaration)));
        }
        if target.eq_ignore_ascii_case("xml") {
            // libxml2 keeps a misplaced or miscased declaration as a PI.
            self.recover_markup(Error::new(ErrorKind::InvalidDeclaration, start))?;
        } else if target.len() > 3
            && target.as_bytes()[..3].eq_ignore_ascii_case(b"xml")
            && !matches!(target, "xml-stylesheet" | "xml-model")
        {
            let warning = ErrorKind::Warning(crate::Warning::ReservedTarget);
            self.budget.recover(Error::new(warning, start + 2))?;
        }
        if self.encoding.requires_declaration() && !self.declaration_seen && !self.replacement {
            return Err(Error::new(ErrorKind::EncodingMismatch, start));
        }
        let data = body[name_end..].trim_start_matches([' ', '\t', '\r', '\n']);
        let offset = start + raw.len() - 2 - data.len();
        #[cfg(feature = "catalog")]
        self.note_catalog(target, data);
        Ok(Some(Event::ProcessingInstruction {
            target: Cow::Borrowed(target),
            data: self.normalized::<REREADS>(data, offset, false, false, true)?,
        }))
    }

    /// A start tag; `DTD` tags also read `declarations` (attribute
    /// normalization, kept references and defaults).
    fn start_element<'event, const REREADS: bool, const DTD: bool, F>(
        &mut self,
        raw: &'event str,
        start: usize,
        retain: &mut F,
        declarations: Option<&mut TagDeclarations<'_>>,
    ) -> Result<Option<Event<'event>>, Error>
    where
        'source: 'event,
        F: FnMut(&'event str, usize, &mut Budget) -> Result<Cow<'source, str>, Error>,
    {
        let mut declarations = if DTD { declarations } else { None };
        if raw.len() < 2 || !raw.starts_with('<') {
            return Err(Error::new(ErrorKind::Syntax, start));
        }
        // A recovering slice reader passes a tag cut off by the end of input,
        // which libxml2 makes an empty element.
        let closed = raw.ends_with('>');
        if !closed {
            self.budget
                .recover(Error::new(ErrorKind::UnexpectedEof, start))?;
        }
        let mut empty = !closed || raw.as_bytes()[raw.len() - 2] == b'/';
        let content_end = raw.len() - usize::from(closed) - usize::from(closed && empty);
        let mut scan = TagScanner {
            raw: &raw[1..content_end],
            cursor: 0,
            offset: start + 1,
        };
        let (qualified, split) = match scan.scan_name() {
            Ok(name) => name,
            // libxml2 names the element by the longest Name there, and reads
            // a tag without one as text after `<`.
            Err(error) => match name_length(scan.raw) {
                0 => {
                    self.markup_as_text(error)?;
                    return self.text_after::<REREADS>(raw, 1, start);
                }
                length => {
                    self.budget.recover(error)?;
                    scan.cursor = length;
                    (&scan.raw[..length], None)
                }
            },
        };
        let (element_prefix, _) = self.qname_parts(qualified, split, start + 1)?;
        let element_prefix = element_prefix.and_then(|prefix| NonZeroUsize::new(prefix.len()));
        if let Some(declarations) = declarations.as_deref_mut() {
            declarations.element(qualified);
            self.budget.kept.clear();
        }
        let bindings_start = self.bindings.len();
        let mut attributes = self.take_attributes();
        loop {
            let attribute = match scan.attribute(&mut |error| self.budget.recover(error)) {
                Ok(Some(attribute)) => attribute,
                Ok(None) => break,
                // libxml2 ends the element at a malformed attribute and
                // reads on from there.
                Err(error) => {
                    self.reread(error, start, error.offset)?;
                    empty = true;
                    break;
                }
            };
            let name = attribute.qualified;
            let value = attribute.literal;
            let offset = attribute.name_offset;
            let value_offset = attribute.value_offset;
            if attributes.len() == self.budget.limits.max_attributes {
                return Err(Error::new(
                    ErrorKind::LimitExceeded(Limit::Attributes),
                    offset,
                ));
            }
            let (prefix, local) = self.qname_parts(name, attribute.split, offset)?;
            let declaration = if name == "xmlns" {
                Some("")
            } else if prefix == Some("xmlns") {
                Some(local)
            } else {
                None
            };
            // A declaration's value is bound, so it is never lent.
            let lent = declaration.is_none();
            let mut kept = None;
            let value = if let Some(declarations) = declarations.as_deref_mut() {
                let at = (start, offset, value_offset);
                let (value, form) =
                    declarations.value(&mut self.budget, (qualified, name), value, at)?;
                kept = form;
                value
            } else if attribute.plain && !(REREADS && self.repairs_characters) {
                Cow::Borrowed(value)
            } else {
                match self.normalized::<REREADS>(value, value_offset, true, true, lent) {
                    Ok(value) => value,
                    // Recovery from a character reference libxml2 reads as
                    // zero, which ends the element (see `normalize_into`).
                    Err(error) if error.kind == ErrorKind::InvalidReference && self.recovers() => {
                        self.rewind = Some(error.offset);
                        empty = true;
                        break;
                    }
                    Err(error) => return Err(error),
                }
            };
            if name == "xml:space" && !matches!(&*value, "default" | "preserve") {
                let warning = ErrorKind::Warning(crate::Warning::SpaceValue);
                self.budget.recover(Error::new(warning, offset))?;
            }
            let redundant = declaration.is_some_and(|prefix| self.redundant(prefix, &value));
            if let Some(bound_prefix) = declaration {
                // libxml2 ignores a declaration it cannot bind, and all but
                // the first of a prefix in one tag.
                if invalid_namespace_binding(bound_prefix, &value) {
                    self.budget
                        .recover(Error::new(ErrorKind::Namespace, offset))?;
                    continue;
                }
                // libxml2 checks a declared URI with `xmlParseURISafe`; only
                // a relative default namespace warns without `PEDANTIC`.
                let warning = match crate::uri::has_scheme(&value) {
                    _ if value.is_empty() => None,
                    None => Some(crate::Warning::NamespaceUri),
                    Some(false) if bound_prefix.is_empty() => {
                        Some(crate::Warning::RelativeNamespaceUri)
                    }
                    Some(_) => None,
                };
                if let Some(warning) = warning {
                    self.budget
                        .recover(Error::new(ErrorKind::Warning(warning), offset))?;
                }
                if self.recovers()
                    && self
                        .bound(bound_prefix)
                        .is_some_and(|at| at >= bindings_start)
                {
                    self.budget
                        .recover(Error::new(ErrorKind::DuplicateAttribute, offset))?;
                    continue;
                }
                self.bind(bound_prefix, Some(bound_prefix), &value, offset, retain)?;
            }
            if redundant {
                continue;
            }
            attributes
                .try_reserve(1)
                .map_err(|_| Error::new(ErrorKind::Allocation, offset))?;
            if let Some(kept) = kept {
                self.keep(attributes.len(), kept, offset)?;
            }
            attributes.push(Attribute {
                name: Name::from_validated_parts(
                    Cow::Borrowed(name),
                    prefix.and_then(|prefix| std::num::NonZeroUsize::new(prefix.len())),
                    None,
                ),
                value,
            });
        }
        if let Some(declarations) = declarations {
            for default in declarations.defaults(&mut self.budget, qualified, start)? {
                self.default_attribute(&mut attributes, default, start, retain)?;
            }
        }
        let name = self.resolve_name(Cow::Borrowed(qualified), element_prefix, start + 1)?;
        match &mut attributes[..] {
            [] => {}
            [attribute] => _ = self.resolve_attribute(&mut attribute.name, start)?,
            attributes => self.resolve_attributes(attributes, start)?,
        }
        // An unbound element prefix is reported after its attributes', as
        // in libxml2.
        if element_prefix.is_some() && name.namespace().is_none() {
            let error = Error::new(ErrorKind::Namespace, start + 1);
            self.budget.recover(error)?;
        }
        let element = Element {
            name,
            attributes: attributes.into(),
        };
        if empty {
            self.close_scope(bindings_start);
            if self.stack.is_empty() && !self.fragment {
                self.phase = Phase::Epilog;
            }
            Ok(Some(Event::Empty(element)))
        } else {
            reserve_frame(&mut self.stack, start)?;
            let qualified = if self.copies_names {
                self.budget.charge_owned(qualified.len(), start + 1)?;
                if self.frame_names.capacity() - self.frame_names.len() < qualified.len() {
                    self.frame_names
                        .try_reserve(qualified.len().max(self.frame_names.len()).max(8 * FRAMES))
                        .map_err(|_| Error::new(ErrorKind::Allocation, start + 1))?;
                }
                let begin = self.frame_names.len();
                self.frame_names.push_str(qualified);
                FrameName::Pooled(begin..self.frame_names.len())
            } else {
                FrameName::Retained(retain(qualified, start + 1, &mut self.budget)?)
            };
            self.stack.push(Frame {
                qualified,
                bindings_start,
            });
            self.phase = Phase::Content;
            Ok(Some(Event::Start(element)))
        }
    }

    /// A start tag that reads the declarations of `catalog`, with
    /// literals from `source`.
    pub(crate) fn process_dtd_start<'event, F>(
        &mut self,
        raw: &'event str,
        start: usize,
        (catalog, source): (
            &mut crate::dtd_catalog::DtdCatalog,
            crate::dtd::LiteralSource,
        ),
        retain: &mut F,
    ) -> Result<Option<Event<'event>>, Error>
    where
        'source: 'event,
        F: FnMut(&'event str, usize, &mut Budget) -> Result<Cow<'source, str>, Error>,
    {
        self.settle();
        self.check_event_limit(start)?;
        self.check_token_start(TokenKind::Start, start)?;
        let mut declarations = TagDeclarations::new(catalog, source);
        let event =
            self.start_element::<false, true, F>(raw, start, retain, Some(&mut declarations));
        if event.is_ok() {
            self.events += 1;
        }
        event
    }

    /// Binds `prefix` to `value` in the scope of the tag being read; the
    /// prefix's `source` text is retained, a copy otherwise.
    #[allow(
        clippy::ptr_arg,
        reason = "a borrowed value is retained, an owned one copied"
    )]
    #[inline(always)]
    fn bind<'event, F>(
        &mut self,
        prefix: &str,
        source: Option<&'event str>,
        value: &Cow<'event, str>,
        offset: usize,
        retain: &mut F,
    ) -> Result<(), Error>
    where
        F: FnMut(&'event str, usize, &mut Budget) -> Result<Cow<'source, str>, Error>,
    {
        let retained = |retain: &mut F, budget: &mut Budget| match source {
            Some(source) => retain(source, offset, budget),
            None => budget.copy(prefix, offset),
        };
        let namespace = self.intern_namespace(value, offset, retain)?;
        self.bindings
            .try_reserve(1)
            .map_err(|_| Error::new(ErrorKind::Allocation, offset))?;
        let retained_prefix = retained(retain, &mut self.budget)?;
        let uri = self.retain_value(value, offset, retain)?;
        let previous = self.bound(prefix);
        if !self.indexed {
            if previous.is_none() && self.copies_names {
                self.budget.charge_owned(prefix.len(), offset)?;
            }
        } else if let Some(slot) = self.namespace_index.get_mut(prefix) {
            *slot = self.bindings.len();
        } else {
            self.namespace_index
                .try_reserve(1)
                .map_err(|_| Error::new(ErrorKind::Allocation, offset))?;
            let key = retained(retain, &mut self.budget)?;
            self.namespace_index.insert(key, self.bindings.len());
        }
        self.bindings.push(Binding {
            prefix: retained_prefix,
            uri,
            namespace,
            previous,
        });
        self.index_bindings(offset)
    }

    /// Records the kept-reference form of the attribute at `position`.
    fn keep(&mut self, position: usize, form: String, offset: usize) -> Result<(), Error> {
        self.budget
            .kept
            .try_reserve(1)
            .map_err(|_| Error::new(ErrorKind::Allocation, offset))?;
        self.budget.kept.push((position, form));
        Ok(())
    }

    /// Adds a defaulted attribute to the tag at `start`; its failures,
    /// unlike a specified attribute's, are fatal.
    fn default_attribute<'event, F>(
        &mut self,
        attributes: &mut Vec<Attribute<'event>>,
        (name, value, kept): Defaulted,
        start: usize,
        retain: &mut F,
    ) -> Result<(), Error>
    where
        F: FnMut(&'event str, usize, &mut Budget) -> Result<Cow<'source, str>, Error>,
    {
        let fail = |kind| Error::new(kind, start);
        let (prefix, local) = qname(&name, start).map_err(|error| fail(error.kind))?;
        let prefix_length = prefix.and_then(|prefix| NonZeroUsize::new(prefix.len()));
        if attributes.len() == self.budget.limits.max_attributes {
            return Err(fail(ErrorKind::LimitExceeded(Limit::Attributes)));
        }
        let declaration = match prefix {
            None if name == "xmlns" => Some(""),
            Some("xmlns") => Some(local),
            _ => None,
        };
        let value: Cow<'event, str> = value;
        if let Some(bound_prefix) = declaration {
            // libxml2 reports no error for a defaulted declaration: it
            // binds any but `xml`, where yaxml drops any it cannot bind.
            if invalid_namespace_binding(bound_prefix, &value) {
                return Ok(());
            }
            let redundant = self.redundant(bound_prefix, &value);
            self.bind(bound_prefix, None, &value, start, retain)?;
            if redundant {
                return Ok(());
            }
        }
        attributes
            .try_reserve(1)
            .map_err(|_| fail(ErrorKind::Allocation))?;
        if let Some(kept) = kept {
            self.keep(attributes.len(), kept, start)?;
        }
        attributes.push(Attribute {
            name: Name::from_validated_parts(name, prefix_length, None),
            value,
        });
        Ok(())
    }

    #[allow(
        clippy::ptr_arg,
        reason = "borrowed input and owned normalized values require different retention"
    )]
    fn retain_value<'event, F>(
        &mut self,
        value: &Cow<'event, str>,
        offset: usize,
        retain: &mut F,
    ) -> Result<Cow<'source, str>, Error>
    where
        F: FnMut(&'event str, usize, &mut Budget) -> Result<Cow<'source, str>, Error>,
    {
        match value {
            Cow::Borrowed(raw) => retain(raw, offset, &mut self.budget),
            Cow::Owned(value) => self.budget.copy(value, offset),
        }
    }

    #[allow(
        clippy::ptr_arg,
        reason = "the interner preserves borrowed input and charges owned copies"
    )]
    fn intern_namespace<'event, F>(
        &mut self,
        uri: &Cow<'event, str>,
        offset: usize,
        retain: &mut F,
    ) -> Result<Option<NamespaceId>, Error>
    where
        F: FnMut(&'event str, usize, &mut Budget) -> Result<Cow<'source, str>, Error>,
    {
        if uri.is_empty() {
            return Ok(None);
        }
        if uri == XML {
            return Ok(Some(NamespaceId::Xml));
        }
        if !self.indexed {
            if let Some((id, _)) = self.interned(uri) {
                return Ok(Some(id));
            }
            // Charge the URI exactly as retaining it as an index key would.
            if matches!(uri, Cow::Owned(_)) || self.copies_names {
                self.budget.charge_owned(uri.len(), offset)?;
            }
            let id = NamespaceId::Declared(self.next_namespace);
            self.next_namespace = self
                .next_namespace
                .checked_add(1)
                .ok_or(Error::new(ErrorKind::Allocation, offset))?;
            return Ok(Some(id));
        }
        if let Some(entry) = self.namespace_ids.get_mut(uri.as_ref()) {
            entry.bindings = entry
                .bindings
                .checked_add(1)
                .ok_or(Error::new(ErrorKind::Allocation, offset))?;
            return Ok(Some(entry.id));
        }
        self.namespace_ids
            .try_reserve(1)
            .map_err(|_| Error::new(ErrorKind::Allocation, offset))?;
        let key = self.retain_value(uri, offset, retain)?;
        let id = NamespaceId::Declared(self.next_namespace);
        self.next_namespace = self
            .next_namespace
            .checked_add(1)
            .ok_or(Error::new(ErrorKind::Allocation, offset))?;
        self.namespace_ids
            .insert(key, NamespaceEntry { id, bindings: 1 });
        Ok(Some(id))
    }

    /// Resolves an element QName whose prefix split was already validated;
    /// an unbound prefix leaves it in no namespace, unreported.
    fn resolve_name<'event>(
        &mut self,
        qualified: Cow<'event, str>,
        prefix_length: Option<NonZeroUsize>,
        offset: usize,
    ) -> Result<Name<'event>, Error>
    where
        'source: 'event,
    {
        let mut name = Name::from_validated_parts(qualified, prefix_length, None);
        self.resolve_namespace(&mut name, false, offset)?;
        Ok(name)
    }

    /// Resolves two or more attribute names, reporting duplicates.
    #[inline(never)]
    fn resolve_attributes<'event>(
        &mut self,
        attributes: &mut [Attribute<'event>],
        start: usize,
    ) -> Result<(), Error>
    where
        'source: 'event,
    {
        // Expanded-name uniqueness: equal URIs have equal namespace IDs, so
        // short lists compare (ID, local name) pairs directly instead of
        // sorting.
        // Duplicates are kept, as libxml2 does; a QName repeated is a
        // well-formedness error, other prefixes a namespace error.
        let duplicate = |lexical| match lexical {
            true => Error::new(ErrorKind::DuplicateAttribute, start),
            false => Error::new(ErrorKind::Namespace, start),
        };
        if attributes.len() <= SMALL_ATTRIBUTES {
            let mut identities = [None; SMALL_ATTRIBUTES];
            for index in 0..attributes.len() {
                identities[index] = self.resolve_attribute(&mut attributes[index].name, start)?;
                let (prior, rest) = attributes.split_at(index);
                let name = &rest[0].name;
                if prior.iter().zip(&identities).any(|(other, &identity)| {
                    identity == identities[index] && expanded(&other.name).0 == expanded(name).0
                }) {
                    let lexical = prior
                        .iter()
                        .any(|other| other.name.qualified() == name.qualified());
                    self.budget.recover(duplicate(lexical))?;
                }
            }
        } else {
            // Resolve in order up to the first failure. The interleaved check
            // above would report any duplicate among the resolved attributes
            // before reaching that failure, so sorting finds the same result.
            let mut identities = Vec::new();
            identities
                .try_reserve(attributes.len())
                .map_err(|_| Error::new(ErrorKind::Allocation, start))?;
            let mut failure = None;
            for attribute in attributes.iter_mut() {
                match self.resolve_attribute(&mut attribute.name, start) {
                    Ok(identity) => identities.push(identity),
                    Err(error) => {
                        failure = Some(error);
                        break;
                    }
                }
            }
            let mut names = Vec::new();
            names
                .try_reserve(identities.len())
                .map_err(|_| Error::new(ErrorKind::Allocation, start))?;
            names.extend(
                attributes
                    .iter()
                    .zip(identities)
                    .map(|(attribute, identity)| {
                        let (local, _) = expanded(&attribute.name);
                        (sort_key(local), local, identity)
                    }),
            );
            names.sort_unstable();
            if names.windows(2).any(|pair| pair[0] == pair[1]) {
                self.budget
                    .recover(duplicate(repeated(attributes, start)?))?;
            }
            if let Some(error) = failure {
                return Err(error);
            }
        }
        Ok(())
    }

    /// Resolves an attribute name as `resolve_namespace` does. An unprefixed
    /// attribute other than `xmlns` has no namespace, as it was built.
    #[inline(always)]
    fn resolve_attribute<'event>(
        &mut self,
        name: &mut Name<'event>,
        offset: usize,
    ) -> Result<Option<NamespaceId>, Error>
    where
        'source: 'event,
    {
        if name.prefix().is_none() && name.qualified() != "xmlns" {
            debug_assert!(name.namespace().is_none());
            return Ok(None);
        }
        self.resolve_namespace(name, true, offset)
    }

    /// Resolves `name`; an attribute's unbound prefix is a namespace error
    /// at `offset`, its start tag.
    fn resolve_namespace<'event>(
        &mut self,
        name: &mut Name<'event>,
        attribute: bool,
        offset: usize,
    ) -> Result<Option<NamespaceId>, Error>
    where
        'source: 'event,
    {
        let prefix = name.prefix();
        let qualified = name.qualified();
        let (namespace, identity) = match prefix {
            Some("xmlns") if attribute => (Some(Cow::Borrowed(XMLNS)), Some(NamespaceId::Xmlns)),
            Some("xmlns") => (None, None),
            Some("xml") => (Some(Cow::Borrowed(XML)), Some(NamespaceId::Xml)),
            None if attribute && qualified == "xmlns" => {
                (Some(Cow::Borrowed(XMLNS)), Some(NamespaceId::Xmlns))
            }
            None if attribute || self.bindings.is_empty() => (None, None),
            _ => match self.bound(prefix.unwrap_or("")) {
                Some(index) if !self.bindings[index].uri.is_empty() => {
                    if let Cow::Owned(uri) = &self.bindings[index].uri
                        && !self.lends_namespaces
                    {
                        self.budget.charge_owned(uri.len(), offset)?;
                    }
                    (
                        Some(match &self.bindings[index].uri {
                            Cow::Borrowed(uri) => Cow::Borrowed(*uri),
                            Cow::Owned(_) if self.lends_namespaces => {
                                self.lent_names = true;
                                Cow::Borrowed(LENT)
                            }
                            Cow::Owned(uri) => {
                                let mut copy = String::new();
                                copy.try_reserve(uri.len())
                                    .map_err(|_| Error::new(ErrorKind::Allocation, offset))?;
                                copy.push_str(uri);
                                Cow::Owned(copy)
                            }
                        }),
                        self.bindings[index].namespace,
                    )
                }
                _ if prefix.is_some() && attribute => {
                    let error = Error::new(ErrorKind::Namespace, offset);
                    self.budget.recover(error)?;
                    (None, None)
                }
                _ => (None, None),
            },
        };
        name.set_namespace(namespace);
        Ok(identity)
    }

    /// A QName's prefix and local part; a malformed QName (a namespace
    /// error) reads as an unprefixed name.
    fn qname_parts<'n>(
        &mut self,
        name: &'n str,
        split: Option<Option<NonZeroUsize>>,
        offset: usize,
    ) -> Result<(Option<&'n str>, &'n str), Error> {
        split_qname(name, split, offset).or_else(|error| {
            let error = Error::new(ErrorKind::Namespace, error.offset);
            self.budget.recover(error).map(|()| (None, name))
        })
    }

    fn end_element<'event>(
        &mut self,
        raw: &'event str,
        start: usize,
    ) -> Result<Option<Event<'event>>, Error>
    where
        'source: 'event,
    {
        let body = delimited_body(raw, "</", ">", start)?;
        let mut scan = TagScanner {
            raw: body,
            cursor: 0,
            offset: start + 2,
        };
        // The open element's name is a validated QName: an exact match needs
        // no rescan, and any other spelling takes the diagnosing path.
        let open = self.open_name();
        let trimmed = body.trim_end_matches([' ', '\t', '\r', '\n']);
        let qualified = if open == Some(trimmed) {
            Cow::Borrowed(trimmed)
        } else {
            // A Name and spaces alone would equal `trimmed`: they name
            // another element.
            let error = scan.name().map_or_else(std::convert::identity, |_| {
                scan.skip_space();
                let kind = match scan.cursor == body.len() {
                    true => ErrorKind::MismatchedEndTag,
                    false => ErrorKind::Syntax,
                };
                Error::new(kind, start)
            });
            self.end_open(error)?
        };
        self.close(qualified, start + 2)
    }

    /// The open element's name: libxml2 ends it whatever a malformed or
    /// mismatched end tag names. Recovery stops with nothing open.
    fn end_open(&mut self, error: Error) -> Result<Cow<'static, str>, Error> {
        if self.stack.is_empty() {
            return Err(error);
        }
        self.recover_markup(error)?;
        self.open_copy(error.offset)
    }

    /// Under `RECOVER`, the end of the innermost open element, which a parse
    /// stopped by an unrepaired error closes at `offset` (libxml2's tree keeps
    /// it); `None` once nothing is open.
    #[cold]
    #[inline(never)]
    pub(crate) fn close_open(&mut self, offset: usize) -> Result<Option<Event<'source>>, Error> {
        if self.stack.is_empty() {
            return Ok(None);
        }
        self.settle();
        let qualified = self.open_copy(offset)?;
        self.close(qualified, offset)
    }

    /// A copy of the innermost open element's name.
    fn open_copy(&mut self, offset: usize) -> Result<Cow<'static, str>, Error> {
        let frame = self.stack.last().expect("an open element");
        let name = match &frame.qualified {
            FrameName::Retained(name) => name.as_ref(),
            FrameName::Pooled(range) => &self.frame_names[range.clone()],
        };
        self.budget.copy(name, offset)
    }

    /// Ends the innermost element, named `qualified`; libxml2 ends it
    /// whatever a malformed or mismatched end tag names.
    #[inline(always)]
    fn close<'event>(
        &mut self,
        qualified: Cow<'event, str>,
        offset: usize,
    ) -> Result<Option<Event<'event>>, Error>
    where
        'source: 'event,
    {
        // Split as the start tag did: a recovered malformed QName has no prefix.
        let prefix_length = memchr(b':', qualified.as_bytes())
            .filter(|_| qname(&qualified, offset).is_ok())
            .and_then(NonZeroUsize::new);
        let name = self.resolve_name(qualified, prefix_length, offset)?;
        let frame = self.pop_frame();
        self.close_scope(frame.bindings_start);
        if self.stack.is_empty() && !self.fragment {
            self.phase = Phase::Epilog;
        }
        Ok(Some(Event::End(name)))
    }

    fn unwind_bindings(&mut self, start: usize) {
        while self.bindings.len() > start {
            let binding = self.bindings.pop().expect("binding scope");
            if self.indexed {
                if let Some(previous) = binding.previous {
                    *self
                        .namespace_index
                        .get_mut(binding.prefix.as_ref())
                        .expect("active namespace prefix") = previous;
                } else {
                    self.namespace_index.remove(binding.prefix.as_ref());
                }
            }
            if self.indexed && matches!(binding.namespace, Some(NamespaceId::Declared(_))) {
                let entry = self
                    .namespace_ids
                    .get_mut(binding.uri.as_ref())
                    .expect("active namespace URI");
                entry.bindings -= 1;
                if entry.bindings == 0 {
                    self.namespace_ids.remove(binding.uri.as_ref());
                }
            }
        }
        if self.indexed && self.bindings.len() <= INDEXED_SCOPE / 2 {
            self.namespace_index = HashMap::new();
            self.namespace_ids = HashMap::new();
            self.indexed = false;
        }
    }
}

/// Placeholder namespace of a name whose URI [`ParserState::lend`] borrows;
/// a real namespace is never empty.
const LENT: &str = "";
/// A normalized value lent from `ParserState::values`: NUL is never a value.
const LENT_VALUE: &str = "\0";

fn is_lent_value(value: &str) -> bool {
    value.len() == 1 && value.as_bytes()[0] == 0
}

/// The active binding index for `prefix`: indexed, or the innermost match.
fn bound_in<'source>(
    bindings: &[Binding<'source>],
    index: &HashMap<Cow<'source, str>, usize>,
    indexed: bool,
    prefix: &str,
) -> Option<usize> {
    if indexed {
        index.get(prefix).copied()
    } else {
        bindings
            .iter()
            .rposition(|binding| binding.prefix == prefix)
    }
}

/// The length of the longest Name, colons included, that `text` starts with.
pub(crate) fn name_length(text: &str) -> usize {
    text.char_indices()
        .find(|&(i, c)| !(name_char(c, true) && (i > 0 || name_start(c, true))))
        .map_or(text.len(), |(i, _)| i)
}

/// A QName's prefix and local part, from a scanner-proven split when present.
fn split_qname(
    name: &str,
    split: Option<Option<NonZeroUsize>>,
    offset: usize,
) -> Result<(Option<&str>, &str), Error> {
    match split {
        Some(None) => Ok((None, name)),
        Some(Some(prefix)) => Ok((Some(&name[..prefix.get()]), &name[prefix.get() + 1..])),
        None => qname(name, offset),
    }
}

/// Whether a QName repeats among `attributes` (sorting a copy of the names).
#[cold]
fn repeated(attributes: &[Attribute<'_>], offset: usize) -> Result<bool, Error> {
    let mut names = Vec::new();
    names
        .try_reserve(attributes.len())
        .map_err(|_| Error::new(ErrorKind::Allocation, offset))?;
    names.extend(
        attributes
            .iter()
            .map(|attribute| attribute.name.qualified()),
    );
    names.sort_unstable();
    Ok(names.windows(2).any(|pair| pair[0] == pair[1]))
}

/// An attribute's identity for uniqueness: its expanded name, or its QName
/// in no namespace. Only a recovered unbound prefix makes the two differ.
fn expanded<'n>(name: &'n Name<'_>) -> (&'n str, Option<&'n str>) {
    match name.namespace() {
        None => (name.qualified(), None),
        namespace => (name.local(), namespace),
    }
}

fn invalid_namespace_binding(prefix: &str, uri: &str) -> bool {
    prefix == "xmlns"
        || uri == XMLNS
        || (prefix == "xml" && uri != XML)
        || (prefix != "xml" && uri == XML)
        || (!prefix.is_empty() && uri.is_empty())
}

/// `value` on the heap, failing as `Allocation` at `offset` instead of aborting.
fn boxed<T>(value: T, offset: usize) -> Result<Box<[T; 1]>, Error> {
    let mut slot = Vec::new();
    slot.try_reserve_exact(1)
        .map_err(|_| Error::new(ErrorKind::Allocation, offset))?;
    slot.push(value);
    Ok(slot
        .into_boxed_slice()
        .try_into()
        .unwrap_or_else(|_| unreachable!("one element")))
}

/// A content event with every borrowed string copied, and charged to `budget`.
pub(crate) fn owned_event(
    event: Event<'_>,
    mut budget: Option<&mut Budget>,
    offset: usize,
) -> Result<Event<'static>, Error> {
    let mut copy = |text: &str| match budget.as_deref_mut() {
        Some(budget) => budget.copy(text, offset),
        None => copied(text, offset).map(Cow::Owned),
    };
    let mut owned = |text: Cow<'_, str>| match text {
        Cow::Owned(text) => Ok(Cow::Owned(text)),
        Cow::Borrowed(text) => copy(text),
    };
    let mut element = |element: Element<'_>| -> Result<Element<'static>, Error> {
        let mut attributes = Vec::new();
        attributes
            .try_reserve_exact(element.attributes.len())
            .map_err(|_| Error::new(ErrorKind::Allocation, offset))?;
        for attribute in element.attributes.into_owned() {
            attributes.push(Attribute {
                name: attribute
                    .name
                    .into_static(&mut |text| owned(Cow::Borrowed(text)))?,
                value: owned(attribute.value)?,
            });
        }
        Ok(Element {
            name: element
                .name
                .into_static(&mut |text| owned(Cow::Borrowed(text)))?,
            attributes: attributes.into(),
        })
    };
    Ok(match event {
        Event::Start(start) => Event::Start(element(start)?),
        Event::Empty(empty) => Event::Empty(element(empty)?),
        Event::End(name) => Event::End(name.into_static(&mut |text| owned(Cow::Borrowed(text)))?),
        Event::Text(text) => Event::Text(owned(text)?),
        Event::Cdata(text) => Event::Cdata(owned(text)?),
        Event::Comment(text) => Event::Comment(owned(text)?),
        Event::ProcessingInstruction { target, data } => Event::ProcessingInstruction {
            target: owned(target)?,
            data: owned(data)?,
        },
        Event::EntityStart(name) => Event::EntityStart(owned(name)?),
        Event::EntityEnd => Event::EntityEnd,
        Event::Declaration(_) | Event::Doctype(_) => {
            return Err(Error::new(ErrorKind::InvalidState, offset));
        }
    })
}

/// An attribute list's allocation, emptied and so free of its lifetime. The
/// conversion collects in place: only the allocation changes lifetime.
#[inline(always)]
pub(crate) fn emptied<'to>(mut attributes: Vec<Attribute<'_>>) -> Vec<Attribute<'to>> {
    attributes.clear();
    attributes
        .into_iter()
        .map(|_| unreachable!("cleared"))
        .collect()
}

/// Always inlined: with constant delimiters, the checks need no `memcmp`.
#[inline(always)]
fn delimited_body<'a>(
    raw: &'a str,
    prefix: &str,
    suffix: &str,
    offset: usize,
) -> Result<&'a str, Error> {
    raw.strip_prefix(prefix)
        .and_then(|body| body.strip_suffix(suffix))
        .ok_or(Error::new(ErrorKind::Syntax, offset))
}

pub(crate) fn space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\r' | b'\n')
}
/// ASCII classes for one-pass Name scanning; non-ASCII bytes are class 0.
/// A colon may start and continue a Name, so validity is one AND of classes.
const START: u8 = 1;
const CONTINUE: u8 = 2;
const COLON: u8 = 4;
const DELIMITER: u8 = 8;
const NAME_BYTES: [u8; 256] = {
    let mut table = [0; 256];
    let mut byte = 0;
    while byte < 128 {
        let c = byte as u8;
        table[byte] = if c.is_ascii_alphabetic() || c == b'_' {
            START | CONTINUE
        } else if c.is_ascii_digit() || c == b'-' || c == b'.' {
            CONTINUE
        } else if c == b':' {
            START | CONTINUE | COLON
        } else if matches!(
            c,
            b' ' | b'\t' | b'\r' | b'\n' | b'=' | b'/' | b'>' | b'\'' | b'"'
        ) {
            DELIMITER
        } else {
            0
        };
        byte += 1;
    }
    table
};

/// An integer prefix of a name that orders most distinct names without
/// comparing strings: its length and first eight bytes.
fn sort_key(name: &str) -> u64 {
    let mut head = [0; 8];
    let bytes = name.as_bytes();
    let length = bytes.len().min(8);
    head[..length].copy_from_slice(&bytes[..length]);
    u64::from_be_bytes(head) ^ (bytes.len() as u64).rotate_right(8)
}

/// For a value closed within its first 32 bytes, its length and whether it
/// is free of `<`, references and whitespace that attribute normalization
/// rewrites. Longer values are left to the vectorized searches.
fn short_value(bytes: &[u8], quote: u8) -> Option<(usize, bool)> {
    let mut plain = true;
    for (index, &byte) in bytes.iter().take(32).enumerate() {
        if byte == quote {
            return Some((index, plain));
        }
        plain &= !matches!(byte, b'<' | b'&' | b'\r' | b'\n' | b'\t');
    }
    None
}

struct TagScanner<'a> {
    raw: &'a str,
    cursor: usize,
    offset: usize,
}

struct ScannedAttribute<'a> {
    qualified: &'a str,
    /// The value is known to need no normalization.
    plain: bool,
    /// The proven QName split, when the scanner established it.
    split: Option<Option<NonZeroUsize>>,
    literal: &'a str,
    name_offset: usize,
    value_offset: usize,
}

impl<'a> TagScanner<'a> {
    fn skip_space(&mut self) -> bool {
        let start = self.cursor;
        while self
            .raw
            .as_bytes()
            .get(self.cursor)
            .is_some_and(|b| space(*b))
        {
            self.cursor += 1;
        }
        start != self.cursor
    }

    fn name(&mut self) -> Result<&'a str, Error> {
        self.scan_name().map(|(name, _)| name)
    }

    /// Scans and validates a Name in one pass. For an ASCII name it also
    /// proves the QName split, returned as `Some(prefix length)`; `None`
    /// leaves `qname` to establish it (and its exact error).
    fn scan_name(&mut self) -> Result<(&'a str, Option<Option<NonZeroUsize>>), Error> {
        let start = self.cursor;
        let rest = &self.raw.as_bytes()[start..];
        let (mut length, mut all, mut any, mut colon) = (0, CONTINUE, 0, 0);
        while let Some(&byte) = rest.get(length) {
            let class = NAME_BYTES[usize::from(byte)];
            if class & DELIMITER != 0 {
                break;
            }
            colon = if class & COLON != 0 { length } else { colon };
            (all, any, length) = (all & class, any | class, length + 1);
        }
        self.cursor += length;
        let name = &self.raw[start..self.cursor];
        let first = rest
            .first()
            .map_or(0, |&byte| NAME_BYTES[usize::from(byte)]);
        if all & CONTINUE == 0 || first & START == 0 || length == 0 {
            validate_name(name, self.offset + start, true)?;
        }
        // A valid Name is a QName when its only colon is followed by a
        // name-start character; prefix characters are already valid.
        // `colon` is the last one: the split needs it to be the only one.
        let split = match any & COLON {
            0 => Some(None),
            _ => (colon > 0
                && !name.as_bytes()[..colon].contains(&b':')
                && (name[colon + 1..].chars().next())
                    .is_some_and(|c| crate::name::name_start(c, false)))
            .then_some(NonZeroUsize::new(colon)),
        };
        Ok((name, split))
    }

    // Returns the name's source offset; values are still unnormalized input.
    // A `<` in a value is `recover`ed from by keeping it, as libxml2 does.
    fn attribute(
        &mut self,
        recover: &mut impl FnMut(Error) -> Result<(), Error>,
    ) -> Result<Option<ScannedAttribute<'a>>, Error> {
        let spaced = self.skip_space();
        if self.cursor == self.raw.len() {
            return Ok(None);
        }
        if !spaced {
            return Err(Error::new(ErrorKind::Syntax, self.offset + self.cursor));
        }
        let start = self.cursor;
        let (name, split) = self.scan_name()?;
        self.skip_space();
        if self.raw.as_bytes().get(self.cursor) != Some(&b'=') {
            return Err(Error::new(ErrorKind::Syntax, self.offset + self.cursor));
        }
        self.cursor += 1;
        self.skip_space();
        let quote = *self
            .raw
            .as_bytes()
            .get(self.cursor)
            .filter(|b| matches!(b, b'\'' | b'"'))
            .ok_or(Error::new(ErrorKind::Syntax, self.offset + self.cursor))?;
        self.cursor += 1;
        let value_start = self.cursor;
        let (length, plain) = match short_value(&self.raw.as_bytes()[value_start..], quote) {
            Some(scanned) => scanned,
            None => (
                memchr(quote, &self.raw.as_bytes()[value_start..]).ok_or(Error::new(
                    ErrorKind::UnexpectedEof,
                    self.offset + value_start,
                ))?,
                false,
            ),
        };
        self.cursor += length;
        let value = &self.raw[value_start..self.cursor];
        if let Some(i) = memchr(b'<', value.as_bytes()).filter(|_| !plain) {
            recover(Error::new(ErrorKind::Syntax, self.offset + value_start + i))?;
        }
        self.cursor += 1;
        Ok(Some(ScannedAttribute {
            qualified: name,
            plain,
            split,
            literal: value,
            name_offset: self.offset + start,
            value_offset: self.offset + value_start,
        }))
    }
}

/// Parses an XML declaration, or with `text` an external entity's text
/// declaration (optional version, required encoding, no standalone). Like
/// libxml2, recovery skips a pseudo-attribute it cannot take and the rest of
/// a malformed declaration.
pub(crate) fn parse_declaration<'a>(
    raw: &'a str,
    offset: usize,
    text: bool,
    recover: &mut impl FnMut(Error) -> Result<(), Error>,
) -> Result<(Declaration<'a>, Option<usize>), Error> {
    let mut scan = TagScanner {
        raw,
        cursor: 0,
        offset,
    };
    let mut declaration = Declaration {
        version: "1.0",
        encoding: None,
        standalone: None,
    };
    let mut count = 0;
    let mut encoding_offset = None;
    loop {
        let attribute = match scan.attribute(&mut Err) {
            Ok(Some(attribute)) => attribute,
            Ok(None) => break,
            Err(error) => {
                recover(Error::new(ErrorKind::InvalidDeclaration, error.offset))?;
                break;
            }
        };
        let value = attribute.literal;
        let position = attribute.name_offset;
        let accepted = match attribute.qualified {
            "version"
                if count == 0
                    && value.strip_prefix("1.").is_some_and(|minor| {
                        !minor.is_empty() && minor.bytes().all(|b| b.is_ascii_digit())
                    }) =>
            {
                if !text && value != "1.0" {
                    let warning = ErrorKind::Warning(crate::Warning::UnknownVersion);
                    recover(Error::new(warning, position))?;
                }
                declaration.version = value;
                true
            }
            "encoding"
                if count == usize::from(count > 0 || !text)
                    && encoding_offset.is_none()
                    && value
                        .as_bytes()
                        .first()
                        .is_some_and(u8::is_ascii_alphabetic)
                    && value
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-')) =>
            {
                encoding_offset = Some(position);
                declaration.encoding = Some(value);
                true
            }
            "standalone"
                if !text
                    && count > 0
                    && declaration.standalone.is_none()
                    && matches!(value, "yes" | "no") =>
            {
                declaration.standalone = Some(value == "yes");
                true
            }
            _ => false,
        };
        match accepted {
            true => count += 1,
            false => recover(Error::new(ErrorKind::InvalidDeclaration, position))?,
        }
    }
    if count == 0 || (text && encoding_offset.is_none()) {
        recover(Error::new(ErrorKind::InvalidDeclaration, offset))?;
    }
    Ok((declaration, encoding_offset))
}

/// Returns the encoding for the input after the declaration. Only a BOM-less
/// decoding stream (`switchable`) can continue in a declared byte charset.
fn validate_encoding(
    source: crate::encoding::SourceEncoding,
    switchable: bool,
    label: Option<&str>,
    offset: usize,
) -> Result<crate::encoding::SourceEncoding, Error> {
    use crate::encoding::{Charset, SourceEncoding};
    let Some(label) = label else {
        return if source.requires_declaration() {
            Err(Error::new(ErrorKind::EncodingMismatch, offset))
        } else {
            Ok(source)
        };
    };
    let kind = match (source, crate::encoding::charset(label)) {
        // libxml2 records a UCS-4 document's label without checking it.
        (SourceEncoding::Ucs4 { .. }, _) => return Ok(source),
        (_, None) => ErrorKind::UnsupportedEncoding,
        (SourceEncoding::Utf8, Some(Charset::Utf8)) => return Ok(source),
        (SourceEncoding::Utf8, Some(Charset::Byte(table))) if switchable => {
            return Ok(SourceEncoding::Byte(table));
        }
        // Transcoded input, which the label switched already.
        (SourceEncoding::Byte(_), Some(Charset::Byte(_))) => return Ok(source),
        // An EBCDIC declaration, read in IBM037, names the code page that follows.
        (SourceEncoding::Ebcdic(_), Some(Charset::Ebcdic(table))) => {
            return Ok(SourceEncoding::Ebcdic(table));
        }
        _ => ErrorKind::UnsupportedEncoding,
    };
    Err(Error::new(kind, offset))
}

#[cfg(test)]
#[path = "../tests/internal/parser.rs"]
mod tests;
