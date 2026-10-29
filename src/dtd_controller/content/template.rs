//! Recorded expansions. An internal entity referenced from stable input
//! publishes the same events at every reference, except for their origins
//! and the ledgers they charge, once its expansion resolves nothing by name
//! and nothing in it depends on the context: no prefixed names or namespace
//! declarations, no instructions, no recovered errors. Its second such
//! reference records the events and charges; later ones replay them when
//! every ledger has room for the whole expansion, so no limit can fail
//! inside it, and otherwise expand as before.

use std::borrow::Cow;
use std::collections::TryReserveError;
use std::num::NonZeroU64;

use super::*;
use crate::dtd_catalog::CatalogUsage;
use crate::dtd_controller::ENTITY_FIXED_COST;
use crate::dtd_frames::{EntityInvocationId, FrameCounters};
use crate::fallible::cow;
use crate::{Attribute, Attributes, Element};

/// Bytes one recording may hold, and all of them together.
const RECORDING_BYTES: usize = 16 * 1024;
const TEMPLATE_BYTES: usize = 1024 * 1024;

/// What became of an entity's references from stable input.
#[derive(Default)]
pub(super) enum Slot<'input> {
    #[default]
    Unseen,
    Seen,
    Recorded(Template<'input>),
    /// Its expansion depends on more than the entity, or is too large.
    Expanded,
}

pub(super) struct Template<'input> {
    entries: Box<[Entry<'input>]>,
    cost: Cost,
    /// The recorded reference's invocation, from which its entities' invocations count.
    first: u64,
    /// Entities entered, the reference's own included.
    invocations: u64,
    /// Elements open inside the reference, at most.
    depth: usize,
    /// The default namespace in scope, which unprefixed names took.
    namespace: Option<Cow<'input, str>>,
}

/// A published event, with the owned bytes its publication charged.
struct Entry<'input> {
    event: Event<'input>,
    origin: FragmentOrigin,
    owned: usize,
}

/// What an expansion charges to the ledgers that never fail between events.
#[derive(Clone, Copy, Default)]
struct Cost {
    work: usize,
    frames: FrameCounters,
    catalog: CatalogUsage,
    events: usize,
    owned: usize,
}

/// The ledgers when a recording began.
#[derive(Clone, Copy)]
struct Ledgers {
    work: usize,
    frames: FrameCounters,
    invocation: u64,
    catalog: CatalogUsage,
    events: usize,
    errors: usize,
}

pub(super) struct Recording<'input> {
    entity: EntityId,
    before: Ledgers,
    entries: Vec<Entry<'input>>,
    bytes: usize,
    depth: usize,
    entry_depth: usize,
    namespace: Option<Cow<'input, str>>,
}

/// The template being replayed.
pub(super) struct Replay {
    entity: EntityId,
    next: usize,
    invocation: u64,
    reference: ByteSpan,
}

impl<'input> ContentControllerCore<'input> {
    /// At a reference from stable input to `id`, which shared `left` of the
    /// expansion allowance: replays its template and returns `true` when it
    /// has room, or begins recording at its second reference.
    pub(super) fn reuse(&mut self, id: EntityId, reference: Range<usize>, left: usize) -> bool {
        let reusable = !self.markers
            && matches!(self.source, Source::Stable(..))
            && self.body.frames.stack().is_empty();
        if !reusable {
            return false;
        }
        #[cfg(test)]
        if !tests::TEMPLATES.get().0 {
            return false;
        }
        if self.templates.len() <= id.0 {
            let grown = self.templates.try_reserve(id.0 + 1 - self.templates.len());
            if grown.is_err() {
                return false;
            }
            self.templates.resize_with(id.0 + 1, Slot::default);
        }
        match &self.templates[id.0] {
            Slot::Unseen => self.templates[id.0] = Slot::Seen,
            Slot::Seen => self.recording = self.begin(id),
            Slot::Recorded(template) if self.fits(template, left) => {
                let (cost, invocations) = (template.cost, template.invocations);
                self.work.spend(cost.work);
                let invocation = self.body.frames.replayed(cost.frames, invocations);
                self.body.catalog.replayed(cost.catalog);
                self.parser.skip_events(cost.events);
                #[cfg(test)]
                tests::TEMPLATES.set((true, tests::TEMPLATES.get().1 + 1));
                let reference = ByteSpan::new(reference).expect("ordered");
                self.replaying = Some(Replay {
                    entity: id,
                    next: 0,
                    invocation,
                    reference,
                });
                return true;
            }
            Slot::Recorded(_) | Slot::Expanded => {}
        }
        false
    }

    /// Whether every ledger has room for `template` here. Entity depth
    /// needs no check: its ceiling is fixed and no frame is open here. The
    /// amplification ceiling is the used amplification plus `left`.
    fn fits(&self, template: &Template<'input>, left: usize) -> bool {
        let (cost, namespace) = (template.cost, self.parser.default_namespace());
        let frames = self.body.frames.headroom();
        let catalog = self.body.catalog.headroom();
        let (events, depth, owned) = self.parser.headroom();
        let expanded = (cost.frames.reference_attempts)
            .saturating_mul(ENTITY_FIXED_COST)
            .saturating_add(cost.frames.expanded_bytes)
            .saturating_add(cost.catalog.amplification);
        let same_owner = |a: &Cow<'_, str>, b: &Cow<'_, str>| {
            matches!(a, Cow::Owned(_)) == matches!(b, Cow::Owned(_)) && a == b
        };
        cost.work <= self.work.remaining()
            && cost.frames.reference_attempts <= frames.reference_attempts
            && cost.frames.work <= frames.work
            && cost.frames.expanded_bytes <= frames.expanded_bytes
            && cost.catalog.expanded_bytes <= catalog.expanded_bytes
            && cost.catalog.work <= catalog.work
            && cost.catalog.references <= catalog.references
            && expanded <= left
            && cost.events <= events
            && template.depth <= depth
            && cost.owned <= owned
            && match (namespace, &template.namespace) {
                (None, None) => true,
                (Some(a), Some(b)) => same_owner(a, b),
                _ => false,
            }
    }

    fn begin(&mut self, entity: EntityId) -> Option<Box<Recording<'input>>> {
        let namespace = self.parser.default_namespace().map(cow).transpose();
        let Ok(namespace) = namespace else {
            self.templates[entity.0] = Slot::Expanded;
            return None;
        };
        let before = Ledgers {
            work: self.work.remaining(),
            frames: self.body.frames.counters(),
            invocation: self.body.frames.next_invocation(),
            catalog: self.body.catalog.usage(),
            events: self.parser.headroom().0,
            errors: self.parser.errors().len(),
        };
        Some(Box::new(Recording {
            entity,
            before,
            entries: Vec::new(),
            bytes: 0,
            depth: 0,
            entry_depth: self.parser.element_depth(),
            namespace,
        }))
    }

    /// Notes a reference entered while recording, to the entity `known`
    /// when that was resolved before: one resolved by name could resolve
    /// differently elsewhere.
    pub(super) fn record_reference(&mut self, known: Option<EntityId>) {
        if let (Some(recording), None) = (&self.recording, known) {
            self.abandon(recording.entity);
        }
    }

    /// Publishes a replacement token while recording, and records it.
    #[inline(never)]
    pub(super) fn record(
        &mut self,
        kind: TokenKind,
        range: Range<usize>,
    ) -> Result<Option<Event<'input>>, Error> {
        let owned_left = self.parser.headroom().2;
        let event = self.take_replacement(kind, range);
        self.note(&event, owned_left);
        event
    }

    /// Records the outcome of publishing a replacement token, which
    /// charged the owned bytes beyond `owned_left`.
    fn note(&mut self, event: &Result<Option<Event<'input>>, Error>, owned_left: usize) {
        let owned = owned_left - self.parser.headroom().2;
        let kept = !self.parser.owned_budget().kept().is_empty();
        let (depth, origin, total) = (
            self.parser.element_depth(),
            self.origin,
            self.template_bytes,
        );
        let Some(entity) = self.recording.as_ref().map(|recording| recording.entity) else {
            return;
        };
        let event = match event {
            Ok(Some(event)) if !kept && plain(event) => event,
            Ok(None) if owned == 0 => return,
            // A failure ends the parse.
            Err(_) => return,
            _ => return self.abandon(entity),
        };
        let mut bytes = size_of::<Entry<'_>>();
        let event = duplicate(event, &mut bytes);
        let recording = self.recording.as_mut().expect("recording");
        recording.bytes += bytes;
        match event {
            Ok(event)
                if recording.bytes <= RECORDING_BYTES
                    && total + recording.bytes <= TEMPLATE_BYTES
                    && recording.entries.try_reserve(1).is_ok() =>
            {
                recording.depth = recording.depth.max(depth - recording.entry_depth);
                let entry = Entry {
                    event,
                    origin,
                    owned,
                };
                recording.entries.push(entry);
            }
            _ => self.abandon(entity),
        }
    }

    /// Ends the recording when its reference is left: keeps the template
    /// unless the expansion reported an error or warning.
    pub(super) fn conclude(&mut self) {
        let Some(recording) = self.recording.take() else {
            return;
        };
        let before = recording.before;
        let (frames, catalog) = (self.body.frames.counters(), self.body.catalog.usage());
        if self.parser.errors().len() != before.errors {
            return self.abandon(recording.entity);
        }
        let cost = Cost {
            work: before.work - self.work.remaining(),
            frames: FrameCounters {
                reference_attempts: frames.reference_attempts - before.frames.reference_attempts,
                work: frames.work - before.frames.work,
                expanded_bytes: frames.expanded_bytes - before.frames.expanded_bytes,
            },
            catalog: CatalogUsage {
                expanded_bytes: catalog.expanded_bytes - before.catalog.expanded_bytes,
                work: catalog.work - before.catalog.work,
                references: catalog.references - before.catalog.references,
                amplification: catalog.amplification - before.catalog.amplification,
                ..CatalogUsage::default()
            },
            events: before.events - self.parser.headroom().0,
            owned: recording.entries.iter().map(|entry| entry.owned).sum(),
        };
        self.template_bytes += recording.bytes;
        self.templates[recording.entity.0] = Slot::Recorded(Template {
            entries: recording.entries.into_boxed_slice(),
            cost,
            first: before.invocation,
            invocations: self.body.frames.next_invocation() - before.invocation,
            depth: recording.depth,
            namespace: recording.namespace,
        });
    }

    fn abandon(&mut self, entity: EntityId) {
        self.recording = None;
        self.templates[entity.0] = Slot::Expanded;
    }

    /// The replayed template's next event, or `None` after its last.
    #[inline(never)]
    pub(super) fn replay(&mut self) -> Result<Option<Event<'input>>, Error> {
        let replay = self.replaying.as_mut().expect("replaying");
        let Slot::Recorded(template) = &self.templates[replay.entity.0] else {
            unreachable!("replayed templates are recorded")
        };
        let Some(entry) = template.entries.get(replay.next) else {
            self.replaying = None;
            return Ok(None);
        };
        replay.next += 1;
        let origin = match entry.origin {
            FragmentOrigin::Replacement {
                declaration,
                invocation,
                local,
                ..
            } => FragmentOrigin::Replacement {
                declaration,
                invocation: EntityInvocationId(
                    NonZeroU64::new(invocation.0.get() - template.first + replay.invocation)
                        .expect("from 1"),
                ),
                local,
                physical_reference: replay.reference,
            },
            origin => origin,
        };
        let offset = origin.error_offset();
        self.parser
            .owned_budget()
            .charge_owned(entry.owned, offset)?;
        let event = duplicate(&entry.event, &mut 0)
            .map_err(|_| Error::new(ErrorKind::Allocation, offset))?;
        self.origin = origin;
        Ok(Some(event))
    }
}

/// Whether `event` reads the same in any context: names without a prefix
/// other than `xml`, no namespace declarations, no instructions.
fn plain(event: &Event<'_>) -> bool {
    let plain = |name: &crate::Name<'_>| name.prefix().is_none_or(|prefix| prefix == "xml");
    match event {
        Event::Start(element) | Event::Empty(element) => {
            plain(&element.name)
                && (element.attributes.iter()).all(|attribute| {
                    plain(&attribute.name) && attribute.name.qualified() != "xmlns"
                })
        }
        Event::End(name) => plain(name),
        Event::Text(_) | Event::Cdata(_) | Event::Comment(_) => true,
        _ => false,
    }
}

/// A copy of a content event that borrows what it borrows, adding the
/// bytes it owns (names counted whole) to `total`.
fn duplicate<'a>(event: &Event<'a>, total: &mut usize) -> Result<Event<'a>, TryReserveError> {
    let bytes = std::cell::Cell::new(0);
    let text = |text: &Cow<'a, str>| {
        if let Cow::Owned(text) = text {
            bytes.set(bytes.get() + text.len());
        }
        cow(text)
    };
    let name = |name: &crate::Name<'a>| {
        let length = name.qualified().len() + name.namespace().map_or(0, str::len);
        bytes.set(bytes.get() + length);
        name.try_clone()
    };
    let element = |element: &Element<'a>| -> Result<Element<'a>, TryReserveError> {
        let attributes = match &element.attributes {
            Attributes::Borrowed(attributes) => Attributes::Borrowed(attributes),
            Attributes::Owned(attributes) => {
                let mut copy = Vec::new();
                copy.try_reserve_exact(attributes.len())?;
                bytes.set(bytes.get() + attributes.len() * size_of::<Attribute<'_>>());
                for attribute in attributes {
                    let name = name(&attribute.name)?;
                    let value = text(&attribute.value)?;
                    copy.push(Attribute { name, value });
                }
                Attributes::Owned(copy)
            }
        };
        let name = name(&element.name)?;
        Ok(Element { name, attributes })
    };
    let event = match event {
        Event::Start(start) => Event::Start(element(start)?),
        Event::Empty(empty) => Event::Empty(element(empty)?),
        Event::End(end) => Event::End(name(end)?),
        Event::Text(value) => Event::Text(text(value)?),
        Event::Cdata(value) => Event::Cdata(text(value)?),
        Event::Comment(value) => Event::Comment(text(value)?),
        _ => unreachable!("templates hold plain content events"),
    };
    *total += bytes.get();
    Ok(event)
}

#[cfg(test)]
#[path = "../../../tests/internal/templates.rs"]
mod tests;
