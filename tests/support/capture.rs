//! Owned snapshots for bounded ordinary test fixtures only.

use std::ops::Range;
use yaxml::{Error, Event, Limits, Name, Reader};

/// Owned callback values, compared by fields rather than debug formatting.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Captured {
    Declaration(String, Option<String>, Option<bool>),
    Doctype(String, Option<String>, Option<String>, Option<String>),
    Element(bool, CapturedName, Vec<(CapturedName, String)>),
    End(CapturedName),
    Text(String),
    Cdata(String),
    Comment(String),
    Pi(String, String),
    EntityStart(String),
    EntityEnd,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapturedName(String, Option<String>, String, Option<String>);

impl From<&Name<'_>> for CapturedName {
    fn from(name: &Name<'_>) -> Self {
        Self(
            name.qualified().into(),
            name.prefix().map(str::to_owned),
            name.local().into(),
            name.namespace().map(str::to_owned),
        )
    }
}

impl From<Event<'_>> for Captured {
    fn from(event: Event<'_>) -> Self {
        match event {
            Event::Declaration(value) => Self::Declaration(
                value.version.into(),
                value.encoding.map(str::to_owned),
                value.standalone,
            ),
            Event::Doctype(value) => Self::Doctype(
                value.name.into(),
                value.public_id.map(str::to_owned),
                value.system_id.map(str::to_owned),
                value.internal_subset.map(str::to_owned),
            ),
            Event::Start(value) => Self::Element(
                false,
                (&value.name).into(),
                value
                    .attributes
                    .iter()
                    .map(|a| ((&a.name).into(), a.value.to_string()))
                    .collect(),
            ),
            Event::Empty(value) => Self::Element(
                true,
                (&value.name).into(),
                value
                    .attributes
                    .iter()
                    .map(|a| ((&a.name).into(), a.value.to_string()))
                    .collect(),
            ),
            Event::End(value) => Self::End((&value).into()),
            Event::Text(value) => Self::Text(value.into_owned()),
            Event::Cdata(value) => Self::Cdata(value.into_owned()),
            Event::Comment(value) => Self::Comment(value.into_owned()),
            Event::ProcessingInstruction { target, data } => {
                Self::Pi(target.into(), data.into_owned())
            }
            Event::EntityStart(name) => Self::EntityStart(name.into_owned()),
            Event::EntityEnd => Self::EntityEnd,
        }
    }
}

pub type CapturedEvent = (Captured, Range<usize>);

pub fn slice_events(input: &[u8]) -> Result<Vec<CapturedEvent>, Error> {
    let mut parser = Reader::new(input)?;
    let mut events = Vec::new();
    while let Some(event) = parser.read_event()? {
        events.push((
            event.into(),
            parser.event_range().expect("direct input event"),
        ));
    }
    Ok(events)
}

pub fn push_events(
    input: &[u8],
    schedule: &[usize],
    limits: Limits,
) -> Result<Vec<CapturedEvent>, Error> {
    let mut events = Vec::new();
    super::driver::drive_push(input, schedule, limits, |event, range| {
        events.push((event.into(), range));
    })?;
    Ok(events)
}
