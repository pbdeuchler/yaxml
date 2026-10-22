//! What a start tag reads from the declarations: attribute normalization,
//! kept references and defaults (see `ParserState::start_element`).

use std::borrow::Cow;

use crate::dtd_catalog::{AttributeKind, DtdCatalog, LiteralSource, StoredDefault};
use crate::escape::predefined;
use crate::parser::Budget;
use crate::valid::Code;
use crate::{Error, ErrorKind};

/// A defaulted attribute's name, value and kept form.
pub(crate) type Defaulted = (Cow<'static, str>, Cow<'static, str>, Option<String>);

/// The declarations one start tag reads, from `source`.
pub(crate) struct TagDeclarations<'c> {
    catalog: &'c mut DtdCatalog,
    source: LiteralSource,
    /// The element's declaration.
    element: Option<usize>,
    /// Which declared attributes the tag specifies: the first 64 by bit,
    /// the others listed.
    specified: u64,
    more: Vec<usize>,
}

impl<'c> TagDeclarations<'c> {
    pub(crate) fn new(catalog: &'c mut DtdCatalog, source: LiteralSource) -> Self {
        Self {
            catalog,
            source,
            element: None,
            specified: 0,
            more: Vec::new(),
        }
    }

    /// Starts the tag of element `qualified`.
    pub(crate) fn element(&mut self, qualified: &str) {
        self.element = self.catalog.element_decl(qualified);
    }

    /// Offsets are physical only outside entity replacement text.
    fn physical(&self, offset: usize) -> Option<usize> {
        (self.source == LiteralSource::Physical).then_some(offset)
    }

    /// Specified attribute `name`'s value, normalized as declared, and its
    /// kept form. Recovery empties a value whose references cannot be
    /// expanded.
    pub(crate) fn value<'e>(
        &mut self,
        budget: &mut Budget,
        (element, name): (&str, &str),
        literal: &'e str,
        (start, name_offset, value_offset): (usize, usize, usize),
    ) -> Result<(Cow<'e, str>, Option<String>), Error> {
        let declared = self.element.and_then(|element| {
            let index = self.catalog.attribute_position(element, name)?;
            let decl = &self.catalog.element_attributes(element)[index];
            Some((index, decl.kind != AttributeKind::Cdata, decl.internal))
        });
        let tokenized = declared.is_some_and(|(_, tokenized, _)| tokenized);
        match declared {
            Some((index @ ..64, ..)) => self.specified |= 1 << index,
            Some((index, ..)) => {
                self.more
                    .try_reserve(1)
                    .map_err(|_| Error::new(ErrorKind::Allocation, name_offset))?;
                self.more.push(index);
            }
            None => {}
        }
        // libxml2's XML_ATTVAL_NORM_CHANGE, judged on the literal's whitespace.
        if self.catalog.standalone()
            && declared.is_some_and(|(_, tokenized, internal)| tokenized && !internal)
            && tokenizing_changes(literal)
        {
            let message = format!(
                "standalone: normalization of attribute {name} on {element} by external subset declaration"
            );
            let offset = self.physical(name_offset);
            self.catalog.note(Code::NotStandalone, offset, message);
        }
        let mut requested = 0;
        let normalized = self.catalog.normalize_value(
            literal,
            (self.source, true, tokenized),
            budget.remaining_owned(),
            &mut requested,
        );
        budget.charge_owned(requested, value_offset)?;
        if let Some(warning) = self.catalog.undeclared_warning() {
            for &at in self.catalog.undeclared_references() {
                let offset = self.physical(value_offset + at).unwrap_or(start);
                budget.recover(Error::new(ErrorKind::Warning(warning), offset))?;
            }
        }
        let value = match normalized {
            Ok(value) => value,
            Err(error) if budget.recovers() => {
                let error = Error::new(error.kind, value_offset + error.offset.unwrap_or(0));
                budget.recover(error)?;
                return Ok((Cow::Borrowed(""), None));
            }
            // A failure inside a referenced entity is the tag's.
            Err(error) => {
                let offset = error.offset.map_or(start, |offset| value_offset + offset);
                return Err(Error::new(error.kind, offset));
            }
        };
        let kept = match self.catalog.keeps_references(name, literal) {
            true => kept_form(
                literal,
                tokenized,
                self.source == LiteralSource::Physical,
                |name| self.catalog.keeps(name),
                |bytes| budget.charge_owned(bytes, value_offset),
            )?,
            false => None,
        };
        Ok((value, kept))
    }

    /// The defaults of the declared attributes the tag does not specify,
    /// copied: namespace declarations, and with `DTDATTR` all. Every copy
    /// libxml2 would make is charged as amplification first.
    pub(crate) fn defaults(
        &mut self,
        budget: &mut Budget,
        element: &str,
        start: usize,
    ) -> Result<Vec<Defaulted>, Error> {
        let mut defaults = Vec::new();
        let Some(index) = self.element else {
            return Ok(defaults);
        };
        // Sorted and deduplicated (recovery keeps repeated attributes), so a
        // tag specifying many attributes stays n log n.
        self.more.sort_unstable();
        self.more.dedup();
        let (specified, more) = (self.specified, &self.more);
        let unspecified = |index: usize| match index {
            ..64 => specified & 1 << index == 0,
            _ => more.binary_search(&index).is_err(),
        };
        let element_decl = &self.catalog.dtd().elements[index];
        let declared = &element_decl.attributes;
        // Copied defaults amplify output like entities do (libxml2
        // `expandedSize`): all of the element's, less the specified ones.
        let bits = std::iter::successors(Some(specified), |bits| Some(bits & bits.wrapping_sub(1)))
            .take_while(|bits| *bits != 0)
            .map(|bits| bits.trailing_zeros() as usize);
        let copied = (bits.chain(more.iter().copied()))
            .map(|index| declared[index].copy_cost())
            .fold(element_decl.copied, usize::saturating_sub);
        // Without `DTDATTR` libxml2 applies only defaulted namespace
        // declarations; a standalone document notes external defaults.
        let complete = self.catalog.default_attributes();
        let standalone = self.catalog.standalone();
        let walk = complete || standalone || element_decl.namespace_defaults;
        self.catalog
            .charge_default(copied)
            .map_err(|error| Error::new(error.kind, start + error.offset.unwrap_or(0)))?;
        if !walk {
            return Ok(defaults);
        }
        let declared = self.catalog.element_attributes(index);
        let mut notes = Vec::new();
        for (index, decl) in declared.iter().enumerate() {
            let StoredDefault::Value { value, kept, .. } = &decl.default else {
                continue;
            };
            if !unspecified(index) {
                continue;
            }
            if standalone && !decl.internal {
                notes.push(format!(
                    "standalone: attribute {} on {element} defaulted from external subset",
                    decl.name
                ));
            }
            if !complete && !decl.declares_namespace() {
                continue;
            }
            defaults
                .try_reserve(1)
                .map_err(|_| Error::new(ErrorKind::Allocation, start))?;
            let mut copy = |text| budget.copy(text, start);
            let kept = kept.as_deref().map(&mut copy).transpose()?;
            defaults.push((copy(&decl.name)?, copy(value)?, kept.map(Cow::into_owned)));
        }
        for message in notes {
            let offset = self.physical(start);
            self.catalog
                .note(Code::StandaloneDefaulted, offset, message);
        }
        Ok(defaults)
    }
}

/// Whether discarding leading, trailing and repeated spaces changes a
/// literal after whitespace normalization; references are not expanded.
fn tokenizing_changes(literal: &str) -> bool {
    let mut previous = true;
    for c in literal.chars().filter(|&c| c != '\r') {
        let space = matches!(c, ' ' | '\t' | '\n');
        if space && previous {
            return true;
        }
        previous = space;
    }
    previous && !literal.is_empty()
}

/// What an attribute literal's next character or reference contributes to
/// libxml2's value without `NOENT`.
enum Piece<'a> {
    Char(char),
    /// Literal whitespace or `&#32;`, collapsed in tokenized values.
    Space,
    /// A kept general reference's name.
    Kept(&'a str),
    /// An undeclared entity, which expands to nothing.
    Nothing,
}

/// libxml2's attribute value without `NOENT` (`xmlParseAttValueInternal`):
/// general references `keep` accepts (declared ones, never predefined or
/// character references) stay `&name;`, an `&` from a reference is `&#38;`,
/// other references are resolved, an undeclared one is dropped, and
/// whitespace is normalized around the kept references, not inside them.
/// `None`, without allocating, when nothing is kept. `charge` approves the
/// one exact reservation (the literal's length) first.
pub(crate) fn kept_form(
    literal: &str,
    tokenized: bool,
    physical: bool,
    keep: impl Fn(&str) -> bool,
    charge: impl FnOnce(usize) -> Result<(), Error>,
) -> Result<Option<String>, Error> {
    // A validated literal: every `&` starts a reference ending at `;`.
    let name = |at: usize| {
        let end = literal[at..].find(';').map_or(at + 1, |end| at + end);
        &literal[at + 1..end]
    };
    if !memchr::memchr_iter(b'&', literal.as_bytes()).any(|at| keep(name(at))) {
        return Ok(None);
    }
    charge(literal.len())?;
    let mut form = String::new();
    form.try_reserve_exact(literal.len())
        .map_err(|_| Error::new(ErrorKind::Allocation, 0))?;
    let (mut in_space, mut at) = (true, 0);
    while let Some(c) = literal[at..].chars().next() {
        let piece = if c == '&' {
            let name = name(at);
            at += name.len() + 2;
            match name.strip_prefix('#') {
                Some(number) => match crate::escape::numeric_reference(number) {
                    Some(' ') => Piece::Space,
                    found => Piece::Char(found.unwrap_or(char::REPLACEMENT_CHARACTER)),
                },
                None => match predefined(name) {
                    Some(c) => Piece::Char(c),
                    None if keep(name) => Piece::Kept(name),
                    None => Piece::Nothing,
                },
            }
        } else {
            at += c.len_utf8();
            match c {
                '\r' if physical && literal[at..].starts_with('\n') => Piece::Nothing,
                ' ' | '\t' | '\n' | '\r' => Piece::Space,
                c => Piece::Char(c),
            }
        };
        match piece {
            Piece::Space if tokenized && in_space => {}
            Piece::Space => form.push(' '),
            Piece::Char('&') => form.push_str("&#38;"),
            Piece::Char(c) => form.push(c),
            Piece::Kept(name) => {
                form.push('&');
                form.push_str(name);
                form.push(';');
            }
            Piece::Nothing => continue,
        }
        in_space = matches!(piece, Piece::Space);
    }
    // A kept reference precedes any trailing space, which was pushed.
    if tokenized && in_space {
        form.pop();
    }
    Ok(Some(form))
}

#[cfg(test)]
#[path = "../tests/internal/dtd_tag.rs"]
mod tests;
