//! External DTD text: the external subset and external parameter entities.
//!
//! Loaded resources are complete strings, so this processor works on them
//! directly. Unlike the internal subset, they admit conditional sections and
//! parameter references inside declarations (XML 1.0 §2.8, §3.4, §4.4.8).

use super::*;
use crate::dtd::{DtdErrorKind, DtdUnit};
use crate::parser::space;
use crate::resolve::ResourceKind;

#[derive(Clone, Copy, Eq, PartialEq)]
enum Source {
    Subset,
    Entity(EntityId),
}

/// One source being read; `base` is the outer base restored on exit.
struct Frame {
    source: Source,
    cursor: usize,
    base: Option<String>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Until {
    /// A markup declaration through its closing `>`.
    Declaration,
    /// A conditional-section keyword through its `[`.
    Keyword,
}

const LOCATION: ValueLocation = ValueLocation::Input(0);

fn syntax() -> CatalogError {
    error(CatalogErrorKind::Grammar(DtdErrorKind::Syntax), LOCATION)
}

impl DtdCatalog {
    pub(crate) fn set_loader(&mut self, loader: Loader) {
        self.loader = Some(loader);
    }

    /// Loads an external parsed entity once; `false` when it has no text.
    pub(crate) fn load(&mut self, id: EntityId) -> Result<bool, CatalogError> {
        let entity = &mut self.entities[id.0];
        let (Some(loader), EntityBody::External(external)) = (&mut self.loader, &mut entity.body)
        else {
            return Ok(false);
        };
        if !external.loaded && external.notation.is_none() {
            external.loaded = true;
            let kind = match entity.parameter {
                true => ResourceKind::ParameterEntity,
                false => ResourceKind::GeneralEntity,
            };
            let uri = external
                .uri
                .as_deref()
                .expect("loader resolves identifiers");
            external.text = loader
                .load(kind, uri, external.public.as_deref())
                .map_err(|kind| error(CatalogErrorKind::Xml(kind), LOCATION))?;
        }
        Ok(external.text.is_some())
    }

    /// Loads the external general entity `name` before content lookup.
    pub(crate) fn load_general(&mut self, name: &str) -> Result<(), CatalogError> {
        match self.general.get(name) {
            Some(&id) => self.load(id).map(drop),
            None => Ok(()),
        }
    }

    /// Processes an external PE referenced between internal declarations;
    /// `outer` lists the internal PEs already being expanded.
    pub(crate) fn include_parameter(
        &mut self,
        id: EntityId,
        outer: &[EntityId],
    ) -> Result<(), CatalogError> {
        let mut frames = Vec::new();
        self.enter(id, &mut frames, outer)?;
        self.run("", &mut frames, outer)
    }

    /// Loads and processes the external subset after the internal subset.
    pub(crate) fn include_subset(&mut self) -> Result<(), CatalogError> {
        let (Some(loader), Some(subset)) = (&mut self.loader, self.subset.take()) else {
            return Ok(());
        };
        let Some(uri) = subset.uri else {
            return Ok(());
        };
        let Some(text) = loader
            .load(ResourceKind::DtdSubset, &uri, subset.public.as_deref())
            .map_err(|kind| error(CatalogErrorKind::Xml(kind), LOCATION))?
        else {
            return Ok(());
        };
        let base = Some(std::mem::replace(&mut loader.base, uri));
        let mut frames = vec![Frame {
            source: Source::Subset,
            cursor: 0,
            base,
        }];
        self.run(&text, &mut frames, &[])
    }

    fn text<'a>(&'a self, subset: &'a str, source: Source) -> &'a str {
        match source {
            Source::Subset => subset,
            Source::Entity(id) => self.entities[id.0].body.text().expect("entered text"),
        }
    }

    fn leave(&mut self, frames: &mut Vec<Frame>) {
        let frame = frames.pop().expect("active frame");
        if let (Some(loader), Some(base)) = (&mut self.loader, frame.base) {
            loader.base = base;
        }
    }

    /// Pushes the replacement text of parameter entity `id`, as libxml2 does
    /// for a reference in external DTD text.
    fn enter(
        &mut self,
        id: EntityId,
        frames: &mut Vec<Frame>,
        outer: &[EntityId],
    ) -> Result<(), CatalogError> {
        increment(
            &mut self.usage.references,
            1,
            self.limits.max_references,
            CatalogLimit::References,
            LOCATION,
        )?;
        let active = frames
            .iter()
            .filter_map(|frame| match frame.source {
                Source::Entity(id) => Some(id),
                Source::Subset => None,
            })
            .chain(outer.iter().copied());
        let mut depth = 0;
        for active in active {
            if active == id {
                return Err(error(CatalogErrorKind::Recursion, LOCATION));
            }
            depth += 1;
        }
        if depth >= self.limits.max_depth {
            return Err(error(
                CatalogErrorKind::Limit(CatalogLimit::Depth),
                LOCATION,
            ));
        }
        if !self.load(id)? && matches!(self.entities[id.0].body, EntityBody::External(_)) {
            return Ok(());
        }
        let length = self.text("", Source::Entity(id)).len();
        increment(
            &mut self.usage.expanded_bytes,
            length,
            self.limits.max_expanded_bytes,
            CatalogLimit::ExpandedBytes,
            LOCATION,
        )?;
        let base = match (&mut self.loader, &self.entities[id.0].body) {
            (Some(loader), EntityBody::External(external)) => Some(std::mem::replace(
                &mut loader.base,
                external.uri.clone().expect("loader resolves identifiers"),
            )),
            _ => None,
        };
        frames
            .try_reserve(1)
            .map_err(|_| error(CatalogErrorKind::Allocation, LOCATION))?;
        frames.push(Frame {
            source: Source::Entity(id),
            cursor: 0,
            base,
        });
        Ok(())
    }

    /// Consumes `%name;` at the current cursor and enters the entity.
    fn reference(
        &mut self,
        subset: &str,
        frames: &mut Vec<Frame>,
        outer: &[EntityId],
    ) -> Result<(), CatalogError> {
        let frame = frames.last().expect("active frame");
        let rest = &self.text(subset, frame.source)[frame.cursor..];
        let end = rest.find(';').ok_or_else(syntax)?;
        validate_name(&rest[1..end], 0, true)
            .map_err(|_| error(CatalogErrorKind::InvalidReference, LOCATION))?;
        let id = self.parameter.get(&rest[1..end]).copied();
        frames.last_mut().expect("active frame").cursor += end + 1;
        self.parameter_reference = true;
        match id {
            Some(id) => self.enter(id, frames, outer),
            None if self.may_be_unread() => Ok(()),
            None => Err(error(CatalogErrorKind::EntityDeclared, LOCATION)),
        }
    }

    fn run(
        &mut self,
        subset: &str,
        frames: &mut Vec<Frame>,
        outer: &[EntityId],
    ) -> Result<(), CatalogError> {
        let mut sections = 0usize;
        while let Some(frame) = frames.last() {
            let rest = &self.text(subset, frame.source)[frame.cursor..];
            let spaces = rest.bytes().take_while(|byte| space(*byte)).count();
            let consumed = if rest.is_empty() {
                self.leave(frames);
                continue;
            } else if spaces > 0 {
                spaces
            } else if rest.starts_with('%') {
                self.reference(subset, frames, outer)?;
                continue;
            } else if rest.starts_with("<![") {
                frames.last_mut().expect("active frame").cursor += 3;
                let keyword = self.collect(subset, frames, outer, Until::Keyword)?;
                match keyword.trim_matches([' ', '\t', '\n']) {
                    "INCLUDE" => sections += 1,
                    "IGNORE" => self.ignore(subset, frames)?,
                    _ => return Err(syntax()),
                }
                continue;
            } else if rest.starts_with("]]>") {
                sections = sections.checked_sub(1).ok_or_else(syntax)?;
                3
            } else if rest.starts_with("<!--") || rest.starts_with("<?") {
                let close = if rest.starts_with("<?") { "?>" } else { "-->" };
                let end = rest.find(close).ok_or_else(syntax)? + close.len();
                let limits = self.loader.as_ref().expect("loaded text").unit;
                DtdUnit::parse(&rest[..end], limits)
                    .map_err(|e| error(CatalogErrorKind::Grammar(e.kind), LOCATION))?;
                end
            } else if rest.starts_with("<!") {
                let declaration = self.collect(subset, frames, outer, Until::Declaration)?;
                let limits = self.loader.as_ref().expect("loaded text").unit;
                let unit = DtdUnit::parse(declaration, limits)
                    .map_err(|e| error(CatalogErrorKind::Grammar(e.kind), LOCATION))?;
                if !matches!(
                    unit.view(),
                    DtdUnitView::Element(_)
                        | DtdUnitView::AttributeList(_)
                        | DtdUnitView::Entity(_)
                        | DtdUnitView::Notation(_)
                ) {
                    return Err(syntax());
                }
                self.apply_inner(
                    &unit,
                    DeclarationContext {
                        internal_subset: false,
                        literals: LiteralSource::Replacement,
                    },
                )?;
                continue;
            } else {
                return Err(syntax());
            };
            frames.last_mut().expect("active frame").cursor += consumed;
        }
        match sections {
            0 => Ok(()),
            _ => Err(syntax()),
        }
    }

    /// Reads through `until`, expanding parameter references outside literals
    /// with one space of padding on each side. Ending a reference made inside
    /// the declaration elsewhere is only a validity error in libxml2.
    fn collect(
        &mut self,
        subset: &str,
        frames: &mut Vec<Frame>,
        outer: &[EntityId],
        until: Until,
    ) -> Result<String, CatalogError> {
        let depth = frames.len();
        let mut output = String::new();
        let mut quote = None;
        loop {
            let frame = frames.last().ok_or_else(syntax)?;
            let rest = &self.text(subset, frame.source)[frame.cursor..];
            // WFC PE Between Declarations: text entered between declarations
            // holds whole declarations; deeper references may split one.
            let Some(character) = rest.chars().next() else {
                if frames.len() <= depth {
                    return Err(syntax());
                }
                self.leave(frames);
                output.push(' ');
                continue;
            };
            let reference = quote.is_none()
                && character == '%'
                && rest[1..]
                    .chars()
                    .next()
                    .is_some_and(|c| crate::name::name_start(c, true));
            if reference {
                output.push(' ');
                self.reference(subset, frames, outer)?;
                continue;
            }
            output
                .try_reserve(character.len_utf8())
                .map_err(|_| error(CatalogErrorKind::Allocation, LOCATION))?;
            frames.last_mut().expect("active frame").cursor += character.len_utf8();
            match (quote, character) {
                (Some(open), _) if open == character => quote = None,
                (Some(_), _) => {}
                (None, '"' | '\'') => quote = Some(character),
                (None, '[') if until == Until::Keyword => return Ok(output),
                (None, '>') if until == Until::Declaration => {
                    output.push('>');
                    increment(
                        &mut self.usage.work,
                        output.len(),
                        self.limits.max_work,
                        CatalogLimit::Work,
                        LOCATION,
                    )?;
                    return Ok(output);
                }
                _ => {}
            }
            output.push(character);
        }
    }

    /// Skips an ignored section's contents, which must end in the same entity.
    fn ignore(&mut self, subset: &str, frames: &mut [Frame]) -> Result<(), CatalogError> {
        let frame = frames.last_mut().expect("active frame");
        let rest = &self.text(subset, frame.source)[frame.cursor..];
        let mut nested = 1usize;
        let mut cursor = 0;
        while nested > 0 {
            let open = rest[cursor..].find("<![");
            let close = rest[cursor..].find("]]>").ok_or_else(syntax)?;
            match open.filter(|open| *open < close) {
                Some(open) => (nested, cursor) = (nested + 1, cursor + open + 3),
                None => (nested, cursor) = (nested - 1, cursor + close + 3),
            }
        }
        frame.cursor += cursor;
        Ok(())
    }
}
