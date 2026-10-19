use crate::encoding::{DecodeProgress, Decoded, Decoder, TokenSpan};
use crate::escape::invalid_char;
use crate::parser::{Budget, ParserState};
use crate::tokenizer::{TokenKind, Tokenizer};
use crate::{
    Attribute, Attributes, DecodeLimits, Element, Error, ErrorKind, Event, Limits, SourceOrigin,
};

pub(crate) mod dtd;

/// Concrete incremental engine shared by callback and lending frontends.
pub(crate) struct Stream {
    tokenizer: Tokenizer,
    decoder: Decoder,
    state: ParserState<'static>,
    decoded_token_limit: usize,
}

impl Stream {
    pub(crate) fn new(limits: Limits, decode: DecodeLimits) -> Self {
        Self {
            tokenizer: Tokenizer::new(limits.max_token_bytes, decode.max_decoded_token_bytes),
            decoder: Decoder::new(decode.max_decoded_bytes),
            state: ParserState::new(limits, 0).copying_names(),
            decoded_token_limit: decode.max_decoded_token_bytes,
        }
    }

    pub(crate) fn position(&self) -> usize {
        self.tokenizer
            .source_position()
            .max(self.decoder.position())
    }

    pub(crate) fn has_token(&self) -> bool {
        self.tokenizer.current().is_some()
    }

    pub(crate) fn token_kind(&self) -> Option<TokenKind> {
        self.tokenizer.current().map(|(kind, _, _)| kind)
    }

    pub(crate) fn is_non_event(&self) -> bool {
        self.tokenizer
            .current()
            .is_some_and(|(kind, bytes, _)| self.state.is_non_event(kind, bytes))
    }

    /// Releases the token; a declaration that selected a byte charset takes
    /// effect here, before any byte after it is decoded.
    pub(crate) fn release(&mut self) {
        self.tokenizer.release();
        let encoding = self.state.encoding();
        if matches!(encoding, crate::encoding::SourceEncoding::Byte(_)) {
            self.decoder.switch(encoding);
            self.tokenizer.switch(encoding);
        }
    }

    pub(crate) fn advance(&mut self, input: &[u8]) -> Result<usize, Error> {
        let tokenizer = &mut self.tokenizer;
        let state = &mut self.state;
        let limit = self.decoded_token_limit;
        self.decoder
            .advance(input, |decoded| decode(tokenizer, state, decoded, limit))
            .map(|progress| progress.consumed)
    }

    pub(crate) fn finish_step(&mut self) -> Result<bool, Error> {
        let tokenizer = &mut self.tokenizer;
        let state = &mut self.state;
        let limit = self.decoded_token_limit;
        if !self
            .decoder
            .finish_step(|decoded| decode(tokenizer, state, decoded, limit))?
        {
            tokenizer.finish_step()?;
        }
        Ok(self.has_token())
    }

    pub(crate) fn validate_finish(&self) -> Result<(), Error> {
        self.state.validate_finish(self.position())
    }

    #[cfg(test)]
    pub(crate) fn event(&mut self) -> Result<(Event<'_>, SourceOrigin), Error> {
        let position = self.position();
        parse(&self.tokenizer, &mut self.state)?
            .ok_or_else(|| Error::new(ErrorKind::InvalidState, position))
    }

    /// Callback delivery for the DTD-free token loop. Returns the accepted
    /// input and `Some(finished)` once the input (or EOF) is exhausted, or
    /// `None` with a parked DOCTYPE token for the DTD session to adopt.
    pub(crate) fn drive(
        &mut self,
        input: &[u8],
        finalizing: bool,
        standalone: &mut bool,
        last_origin: &mut Option<SourceOrigin>,
        emit: &mut impl FnMut(Event<'_>, SourceOrigin, &mut Budget) -> Result<(), Error>,
    ) -> Result<(usize, Option<bool>), Error> {
        let mut consumed = 0;
        loop {
            if let Some((kind, _, _)) = self.tokenizer.current() {
                if kind == TokenKind::Dtd {
                    return Ok((consumed, None));
                }
                let origin = token_origin(&self.tokenizer);
                // The callback borrows the attribute list; its capacity returns
                // to the parser for the next start tag.
                let mut lent = Vec::new();
                if let Some(event) = parse_event(&self.tokenizer, &mut self.state)? {
                    let lends = matches!(
                        &event,
                        Event::Start(element) | Event::Empty(element)
                            if matches!(&element.attributes, Attributes::Owned(list) if list.capacity() > 0)
                    );
                    let event = if lends {
                        lend_attributes(event, &mut lent)
                    } else {
                        event
                    };
                    if let Event::Declaration(declaration) = &event {
                        *standalone = declaration.standalone == Some(true);
                    }
                    *last_origin = Some(origin);
                    emit(event, origin, self.state.owned_budget())?;
                }
                if lent.capacity() > 0 {
                    self.state.recycle_attributes(lent);
                }
                self.release();
                continue;
            }
            if finalizing {
                if self.finish_step()? {
                    continue;
                }
                self.validate_finish()?;
                return Ok((consumed, Some(true)));
            }
            consumed += self.advance(&input[consumed..])?;
            if !self.has_token() {
                if consumed == input.len() {
                    return Ok((consumed, Some(false)));
                }
                return Err(Error::new(ErrorKind::InvalidState, self.position()));
            }
        }
    }

    pub(crate) fn emit(
        &mut self,
        emit: &mut impl FnMut(Event<'_>, SourceOrigin, &mut Budget) -> Result<(), Error>,
    ) -> Result<(), Error> {
        if let Some((event, origin)) = parse(&self.tokenizer, &mut self.state)? {
            emit(event, origin, self.state.owned_budget())?;
        }
        Ok(())
    }
}

fn decode(
    tokenizer: &mut Tokenizer,
    state: &mut ParserState<'static>,
    decoded: Decoded<'_>,
    limit: usize,
) -> Result<DecodeProgress, Error> {
    match decoded {
        Decoded::Start { encoding, bom } => {
            tokenizer.configure(encoding, bom);
            let initial = if encoding == crate::encoding::SourceEncoding::Utf8 {
                bom
            } else {
                0
            };
            state.set_encoding(encoding, initial, limit);
            if encoding == crate::encoding::SourceEncoding::Utf8 && bom == 0 {
                state.allow_switch();
            }
            Ok(DecodeProgress {
                consumed: 0,
                suspended: false,
            })
        }
        Decoded::Bytes(bytes) => {
            let consumed = tokenizer.advance(bytes)?;
            Ok(DecodeProgress {
                consumed,
                suspended: tokenizer.current().is_some(),
            })
        }
    }
}

fn parse<'token>(
    tokenizer: &'token Tokenizer,
    state: &mut ParserState<'static>,
) -> Result<Option<(Event<'token>, SourceOrigin)>, Error> {
    let origin = token_origin(tokenizer);
    Ok(parse_event(tokenizer, state)?.map(|event| (event, origin)))
}

/// Moves an owned attribute list into `lent` and lends it to the event.
fn lend_attributes<'s, 'e: 's>(event: Event<'e>, lent: &'s mut Vec<Attribute<'e>>) -> Event<'s> {
    let lend = |element: Element<'e>, lent: &'s mut Vec<Attribute<'e>>| {
        let Element { name, attributes } = element;
        *lent = attributes.into_owned();
        Element {
            name,
            attributes: Attributes::Borrowed(lent),
        }
    };
    match event {
        Event::Start(element) => Event::Start(lend(element, lent)),
        Event::Empty(element) => Event::Empty(lend(element, lent)),
        event => event,
    }
}

/// Origin of the ready token; the input span when none is ready.
fn token_origin(tokenizer: &Tokenizer) -> SourceOrigin {
    let position = tokenizer.source_position();
    SourceOrigin::input(
        tokenizer
            .current()
            .map_or(position..position, |(_, _, span)| span.source.clone()),
    )
}

/// The ready token's event. Hot callers take the origin separately so the
/// event is returned in place instead of being rewrapped with it.
fn parse_event<'token>(
    tokenizer: &'token Tokenizer,
    state: &mut ParserState<'static>,
) -> Result<Option<Event<'token>>, Error> {
    let Some((kind, bytes, span)) = tokenizer.current() else {
        return Err(Error::new(
            ErrorKind::InvalidState,
            tokenizer.source_position(),
        ));
    };
    let raw = validate(kind, bytes, span)?;
    state
        .process_token(kind, raw, span.decoded.clone(), |raw, offset, budget| {
            budget.copy(raw, offset)
        })
        .map_err(|error| span.map_error(raw, error))
}

fn validate<'token>(
    kind: TokenKind,
    bytes: &'token [u8],
    span: &TokenSpan,
) -> Result<&'token str, Error> {
    // Invalid markup can be recognized on a leading UTF-8 byte. Its semantic
    // kind is rejected without reading the incomplete scalar payload.
    let raw = if matches!(kind, TokenKind::InvalidMarkup) {
        ""
    } else {
        std::str::from_utf8(bytes).map_err(|error| {
            Error::new(
                ErrorKind::InvalidUtf8,
                span.source.start + error.valid_up_to(),
            )
        })?
    };
    if let Some(relative) = invalid_char(raw) {
        return Err(span.map_error(
            raw,
            Error::new(ErrorKind::InvalidCharacter, span.decoded.start + relative),
        ));
    }
    Ok(raw)
}

#[cfg(test)]
#[path = "../tests/internal/stream.rs"]
mod tests;
