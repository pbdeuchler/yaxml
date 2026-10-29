//! libxml2's `areBlanks` (`XML_PARSE_NOBLANKS`) as the parse front sees it:
//! text of spaces is ignorable unless `xml:space` preserves it, the element
//! without `xml:space` already holds text the document spells, its declared
//! content is mixed or ANY, or (undeclared) its other children make it
//! significant. [`Blanks`] decides; [`Filter`] applies it
//! to the streams' events, and the text reader to its nodes.

use crate::{Element, Error, ErrorKind, Event, SourceOrigin};

/// What `areBlanks` asks about the kept children of an element so far.
#[derive(Clone, Copy, Default)]
struct Kids {
    kept: bool,
    /// The first and the last are text.
    first_text: bool,
    last_text: bool,
    /// Some text came from the document itself, which libxml2 notes as
    /// `*ctxt->space = -2`: later blanks in an element without `xml:space`
    /// are kept.
    spoken: bool,
}

impl Kids {
    /// Notes a kept child: `text`, from the document unless `replaced`.
    fn push(&mut self, text: bool, replaced: bool) {
        (self.first_text, self.kept) = (self.first_text || (!self.kept && text), true);
        (self.last_text, self.spoken) = (text, self.spoken || (text && !replaced));
    }
}

/// An element open at the parse front.
struct Open {
    /// The `xml:space` in effect: `preserve` (true), `default` (false) or none.
    space: Option<bool>,
    /// Declared element-only content (true), mixed or ANY (false).
    declared: Option<bool>,
    kids: Kids,
}

/// The elements open at the parse front, which can be ahead of a reader's
/// cursor.
#[derive(Default)]
pub(crate) struct Blanks {
    open: Vec<Open>,
}

impl Blanks {
    /// Opens an element with `xml:space` `space` (else its parent's) and
    /// `declared` content ([`crate::valid::Dtd::has_element_content`]).
    pub(crate) fn open(&mut self, space: Option<bool>, declared: Option<bool>) {
        let space = space.or_else(|| self.open.last().and_then(|open| open.space));
        let kids = Kids::default();
        self.open.push(Open {
            space,
            declared,
            kids,
        });
    }

    pub(crate) fn close(&mut self) {
        self.open.pop();
    }

    /// Notes a child that is not text.
    pub(crate) fn child(&mut self) {
        if let Some(open) = self.open.last_mut() {
            open.kids.push(false, false);
        }
    }

    /// Whether text before a node that `ends` its element or is a
    /// `reference` is dropped: only `blank` text (spaces the document spells)
    /// can be. Kept text, `replaced` or not, is noted as a child.
    pub(crate) fn drops(
        &mut self,
        blank: bool,
        replaced: bool,
        ends: bool,
        reference: bool,
    ) -> bool {
        let Some(open) = self.open.last_mut() else {
            return false;
        };
        let kids = open.kids;
        // Outside declared element content, libxml2 sees the `&` of a
        // following reference, and keeps a blank that is the only child or
        // follows or precedes other text.
        let drop = blank
            && open.space != Some(true)
            && !(open.space.is_none() && kids.spoken)
            && open.declared.unwrap_or(
                (kids.kept || !ends)
                    && (!kids.kept || !(kids.first_text || kids.last_text))
                    && !reference,
            );
        if !drop {
            open.kids.push(true, replaced);
        }
        drop
    }
}

/// An `xml:space` value: `preserve` (true), `default` (false), else none.
pub(crate) fn space(value: &str) -> Option<bool> {
    match value {
        "preserve" => Some(true),
        "default" => Some(false),
        _ => None,
    }
}

/// Whether `text` is all spaces.
pub(crate) fn is_blank(text: &str) -> bool {
    text.bytes().all(|byte| b" \t\r\n".contains(&byte))
}

/// `NOBLANKS` over events, as the text reader drops nodes: a run of blank
/// text is held until the event after it decides it. Entity references'
/// content, which the text reader does not visit, passes as it is.
#[derive(Default)]
pub(crate) struct Filter {
    blanks: Blanks,
    /// The text run before the next event: blank so far (held) or not.
    run: Option<Run>,
    /// Entity references open.
    hidden: usize,
}

struct Run {
    held: Option<(String, SourceOrigin)>,
    replaced: bool,
}

impl Filter {
    /// `event` under `NOBLANKS`: the held text that it decides is kept, to
    /// deliver before it, and whether it passes itself (text can be held).
    /// `declared` is a start tag's declared content (`Budget::content`).
    pub(crate) fn event(
        &mut self,
        event: &Event<'_>,
        origin: SourceOrigin,
        declared: Option<bool>,
    ) -> Result<(Option<(String, SourceOrigin)>, bool), Error> {
        if self.hidden > 0 {
            match event {
                Event::EntityStart(_) => self.hidden += 1,
                Event::EntityEnd => self.hidden -= 1,
                _ => {}
            }
            return Ok((None, true));
        }
        let replaced = origin.replacement_range().is_some();
        if let Event::Text(text) = event {
            let blank = !replaced && is_blank(text);
            let copy = |held: &mut String| {
                held.try_reserve(text.len())
                    .map_err(|_| Error::new(ErrorKind::Allocation, origin.error_offset()))?;
                held.push_str(text);
                Ok::<_, Error>(())
            };
            return Ok(match &mut self.run {
                Some(Run {
                    held: Some((held, _)),
                    ..
                }) if blank => {
                    copy(held)?;
                    (None, false)
                }
                // Other text keeps the run, after what was held.
                Some(run) => {
                    run.replaced |= replaced;
                    (run.held.take(), true)
                }
                None if blank => {
                    let mut held = String::new();
                    copy(&mut held)?;
                    let held = Some((held, origin));
                    self.run = Some(Run { held, replaced });
                    (None, false)
                }
                None => {
                    self.run = Some(Run {
                        held: None,
                        replaced,
                    });
                    (None, true)
                }
            });
        }
        // Another node decides the text before it.
        let kept = self.run.take().and_then(|run| {
            let (ends, reference) = (
                matches!(event, Event::End(_)),
                replaced || matches!(event, Event::EntityStart(_)),
            );
            let drop = (self.blanks).drops(run.held.is_some(), run.replaced, ends, reference);
            run.held.filter(|_| !drop)
        });
        match event {
            Event::End(_) => self.blanks.close(),
            Event::Declaration(_) | Event::Doctype(_) => {}
            Event::Start(element) => {
                self.blanks.child();
                self.blanks.open(element_space(element), declared);
            }
            Event::EntityStart(_) => {
                self.blanks.child();
                self.hidden = 1;
            }
            _ => self.blanks.child(),
        }
        Ok((kept, true))
    }
}

/// The `xml:space` an element specifies.
fn element_space(element: &Element<'_>) -> Option<bool> {
    (element.attributes.iter().rev()).find_map(|attribute| {
        let name = &attribute.name;
        (name.prefix() == Some("xml") && name.local() == "space")
            .then(|| space(&attribute.value))
            .flatten()
    })
}
