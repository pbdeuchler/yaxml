//! Streaming: a pull reader over a copied buffer and a push parser whose
//! callback borrows each event.

use std::ffi::c_void;
use std::io::Cursor;
use std::ops::ControlFlow;
use std::sync::Mutex;

use yaxml::{BufferedReader, Error, ErrorKind, Event, PushParser, ReadError, SourceOrigin};

use crate::doc::name_of;
use crate::error::{Fail, guard};
use crate::ffi::{EventFn, Out, Records, emit, slice};
use crate::options::parse_options;
use crate::registry::Registry;
use crate::types::*;
use crate::*;

/// The pull reader keeps a batch of events, their strings and attributes
/// until the next call: the events themselves borrow the reader and cannot
/// be kept.
pub(crate) struct PullReader {
    reader: BufferedReader<Cursor<Vec<u8>>>,
    strings: Vec<String>,
    attributes: Records<yaxml_attribute>,
    events: Records<yaxml_event>,
}

// `BufferedReader` and `PushParser` hold a `Send` (not `Sync`) resolver
// slot; the registry only ever reaches them through `&mut`.
static READERS: Registry<Mutex<PullReader>> = Registry::new();
static PUSHERS: Registry<Mutex<PushParser>> = Registry::new();

pub(crate) fn live() -> usize {
    READERS.len() + PUSHERS.len()
}

fn exclusive<T>(m: &mut Mutex<T>) -> Result<&mut T, Fail> {
    m.get_mut()
        .map_err(|_| Fail::new(ERR_POISONED, "an earlier call panicked"))
}

/// An event as its C record; strings go through `intern`, attributes are
/// appended to `attributes` (the caller points the record at them).
fn lower(
    event: &Event<'_>,
    origin: SourceOrigin,
    intern: &mut impl FnMut(&str) -> yaxml_str,
    attributes: &mut Vec<yaxml_attribute>,
) -> yaxml_event {
    let range = origin.input_range();
    let mut e = yaxml_event {
        start: range.as_ref().map_or(u64::MAX, |r| r.start as u64),
        end: range.map_or(u64::MAX, |r| r.end as u64),
        ..yaxml_event::default()
    };
    let opt = |s: Option<&str>, intern: &mut dyn FnMut(&str) -> yaxml_str| {
        s.map_or(yaxml_str::NULL, intern)
    };
    match event {
        Event::Declaration(d) => {
            (e.kind, e.value) = (1, intern(d.version));
            e.target = opt(d.encoding, intern);
            e.standalone = d.standalone.map_or(-1, i32::from);
        }
        Event::Doctype(d) => {
            (e.kind, e.value) = (2, intern(d.name));
            e.target = opt(d.public_id, intern);
            e.system_id = opt(d.system_id, intern);
            e.internal_subset = opt(d.internal_subset, intern);
        }
        Event::Start(element) | Event::Empty(element) => {
            e.kind = if matches!(event, Event::Start(_)) {
                3
            } else {
                4
            };
            e.name = name_of(&element.name, intern);
            for attribute in element.attributes.iter() {
                let name = name_of(&attribute.name, intern);
                let value = intern(&attribute.value);
                attributes.push(yaxml_attribute { name, value });
            }
            e.attribute_count = element.attributes.len();
        }
        Event::End(name) => (e.kind, e.name) = (5, name_of(name, intern)),
        Event::Text(t) => (e.kind, e.value) = (6, intern(t)),
        Event::Cdata(t) => (e.kind, e.value) = (7, intern(t)),
        Event::Comment(t) => (e.kind, e.value) = (8, intern(t)),
        Event::ProcessingInstruction { target, data } => {
            (e.kind, e.target, e.value) = (9, intern(target), intern(data));
        }
        Event::EntityStart(name) => (e.kind, e.value) = (10, intern(name)),
        Event::EntityEnd => e.kind = 11,
    }
    e
}

impl PullReader {
    /// Up to `max` events, fewer at the end or before an error. An error
    /// after some events is left for the next call: the reader repeats it.
    fn batch(&mut self, max: usize) -> Result<&[yaxml_event], Fail> {
        let Self {
            reader,
            strings,
            attributes: Records(attributes),
            events: Records(events),
        } = self;
        events.clear();
        attributes.clear();
        // Each string goes into its own reused buffer, so earlier pointers
        // survive the pool growing.
        let mut used = 0;
        let mut intern = |s: &str| {
            if used == strings.len() {
                strings.push(String::new());
            }
            let slot = &mut strings[used];
            used += 1;
            slot.clear();
            slot.push_str(s);
            yaxml_str::of(slot)
        };
        let error = loop {
            match (events.len() < max).then(|| reader.read_event()) {
                Some(Ok(Some((event, origin)))) => {
                    events.push(lower(&event, origin, &mut intern, attributes));
                }
                Some(Err(error)) if events.is_empty() => break error,
                _ => {
                    // `attributes` is complete: point each event at its own.
                    let mut rest = &attributes[..];
                    for e in events.iter_mut().filter(|e| e.attribute_count > 0) {
                        let (these, tail) = rest.split_at(e.attribute_count);
                        (e.attributes, rest) = (these.as_ptr(), tail);
                    }
                    return Ok(events);
                }
            }
        };
        Err(match error {
            ReadError::Xml(error) => Fail::xml(error, reader.get_ref().get_ref()),
            other => Fail::new(ERR_XML, other.to_string()),
        })
    }
}

/// # Safety
/// See include/yaxml.h.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yaxml_reader_new(
    bytes: *const u8,
    len: usize,
    options: *const yaxml_parse_options,
    out: *mut *mut yaxml_reader,
    err: *mut yaxml_error,
) -> i32 {
    guard(err, || {
        let out = Out::new(out)?;
        // SAFETY: yaxml.h: `bytes` holds `len` bytes for the call.
        let input = unsafe { slice(bytes, len) }?;
        // SAFETY: yaxml.h: `options` is NULL or a valid options struct.
        let options = unsafe { parse_options(options) }?;
        let reader = BufferedReader::with_options(Cursor::new(input.to_vec()), options)
            .map_err(Fail::kind)?;
        let reader = READERS.insert(Mutex::new(PullReader {
            reader,
            strings: Vec::new(),
            attributes: Records(Vec::new()),
            events: Records(Vec::new()),
        }));
        // SAFETY: yaxml.h: `out` is valid for writing a handle.
        unsafe { out.put(reader) };
        Ok(OK)
    })
}

/// # Safety
/// See include/yaxml.h.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yaxml_reader_next(
    reader: *mut yaxml_reader,
    out: *mut yaxml_event,
    err: *mut yaxml_error,
) -> i32 {
    guard(err, || {
        let out = Out::new(out)?;
        let event = READERS
            .get(reader)?
            .write(|r| Ok(exclusive(r)?.batch(1)?.first().copied()))?;
        // SAFETY: yaxml.h: `out` is valid for writing an event.
        Ok(unsafe { out.reply(event) })
    })
}

/// # Safety
/// See include/yaxml.h.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yaxml_reader_next_batch(
    reader: *mut yaxml_reader,
    max: usize,
    events: *mut *const yaxml_event,
    count: *mut usize,
    err: *mut yaxml_error,
) -> i32 {
    guard(err, || {
        let (events, count) = (Out::new(events)?, Out::new(count)?);
        if max == 0 {
            return Err(Fail::argument("max must be at least 1"));
        }
        let len = READERS.get(reader)?.write(|r| {
            let batch = exclusive(r)?.batch(max)?;
            let ptr = if batch.is_empty() {
                std::ptr::null()
            } else {
                batch.as_ptr()
            };
            // SAFETY: yaxml.h: `events` is valid for writing a pointer.
            unsafe { events.put(ptr) };
            Ok(batch.len())
        })?;
        // SAFETY: yaxml.h: `count` is valid for writing a size_t.
        unsafe { count.put(len) };
        Ok(if len == 0 { NONE } else { OK })
    })
}

/// # Safety
/// See include/yaxml.h.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yaxml_reader_free(reader: *mut yaxml_reader) -> i32 {
    guard(std::ptr::null_mut(), || READERS.remove(reader).map(|()| OK))
}

/// # Safety
/// See include/yaxml.h.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yaxml_push_new(
    options: *const yaxml_parse_options,
    out: *mut *mut yaxml_push,
    err: *mut yaxml_error,
) -> i32 {
    guard(err, || {
        let out = Out::new(out)?;
        // SAFETY: yaxml.h: `options` is NULL or a valid options struct.
        let options = unsafe { parse_options(options) }?;
        let parser = PushParser::with_options(options).map_err(Fail::kind)?;
        // SAFETY: yaxml.h: `out` is valid for writing a handle.
        unsafe { out.put(PUSHERS.insert(Mutex::new(parser))) };
        Ok(OK)
    })
}

/// Lends one event to the callback, if any.
fn deliver(
    event: &Event<'_>,
    origin: SourceOrigin,
    callback: Option<EventFn>,
    user: *mut c_void,
    attributes: &mut Vec<yaxml_attribute>,
) -> ControlFlow<()> {
    let Some(callback) = callback else {
        return ControlFlow::Continue(());
    };
    attributes.clear();
    let mut event = lower(event, origin, &mut yaxml_str::of, attributes);
    event.attributes = attributes.as_ptr();
    // SAFETY: yaxml.h: `callback` is a yaxml_event_fn accepting `user`; the
    // event's strings and attributes live until it returns.
    match unsafe { emit(callback, user, &event) } {
        0 => ControlFlow::Continue(()),
        _ => ControlFlow::Break(()),
    }
}

fn pushed(result: Result<(), Error>) -> Result<i32, Fail> {
    result.map(|()| OK).map_err(|error| match error.kind {
        ErrorKind::Aborted => {
            let mut fail = Fail::new(ERR_CALLBACK, "the event callback stopped parsing");
            fail.offset = error.offset as u64;
            fail
        }
        _ => Fail::kind(error),
    })
}

/// # Safety
/// See include/yaxml.h.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yaxml_push_feed(
    push: *mut yaxml_push,
    bytes: *const u8,
    len: usize,
    callback: Option<EventFn>,
    user: *mut c_void,
    err: *mut yaxml_error,
) -> i32 {
    guard(err, || {
        // SAFETY: yaxml.h: `bytes` holds `len` bytes for the call.
        let input = unsafe { slice(bytes, len) }?;
        PUSHERS.get(push)?.write(|p| {
            let mut attributes = Vec::new();
            let result = exclusive(p)?.feed(input, |event, origin| {
                deliver(event, origin, callback, user, &mut attributes)
            });
            pushed(result)
        })
    })
}

/// # Safety
/// See include/yaxml.h.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yaxml_push_finish(
    push: *mut yaxml_push,
    callback: Option<EventFn>,
    user: *mut c_void,
    err: *mut yaxml_error,
) -> i32 {
    guard(err, || {
        PUSHERS.get(push)?.write(|p| {
            let mut attributes = Vec::new();
            let result = exclusive(p)?
                .finish(|event, origin| deliver(event, origin, callback, user, &mut attributes));
            pushed(result)
        })
    })
}

/// # Safety
/// See include/yaxml.h.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yaxml_push_free(push: *mut yaxml_push) -> i32 {
    guard(std::ptr::null_mut(), || PUSHERS.remove(push).map(|()| OK))
}
