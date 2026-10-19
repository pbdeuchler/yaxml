use crate::encoding::{DecodeProgress, Decoded, Decoder, SourceEncoding, TokenSpan};
use crate::escape::invalid_char;
use crate::parser::{Budget, ParserState};
use crate::tokenizer::{TokenKind, Tokenizer, complete_token};
use crate::{Attributes, DecodeLimits, Element, Error, ErrorKind, Event, Limits, SourceOrigin};

pub(crate) mod dtd;

/// Concrete incremental engine shared by callback and lending frontends.
pub(crate) struct Stream {
    tokenizer: Tokenizer,
    decoder: Decoder,
    state: ParserState<'static>,
    decoded_token_limit: usize,
}

impl Stream {
    pub(crate) fn new(limits: Limits, decode: DecodeLimits, flags: crate::options::Flags) -> Self {
        Self {
            tokenizer: Tokenizer::new(limits.max_token_bytes, decode.max_decoded_token_bytes),
            decoder: Decoder::new(decode.max_decoded_bytes),
            state: ParserState::new(limits, 0)
                .copying_names()
                .lending_namespaces()
                .with_flags(flags),
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

    #[inline(always)]
    pub(crate) fn advance(&mut self, input: &[u8]) -> Result<usize, Error> {
        // Plain UTF-8 below the decoded ceiling needs no decoding: the
        // tokenizer takes it directly and the decoder counts it as forwarded.
        // As in forwarding, empty input reaches no token state.
        if self
            .decoder
            .utf8_allowance()
            .is_some_and(|allowance| allowance >= input.len())
        {
            if input.is_empty() {
                return Ok(0);
            }
            let consumed = self.tokenizer.advance(input)?;
            self.decoder.skip_utf8(consumed);
            return Ok(consumed);
        }
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
        let origin = token_origin(&self.tokenizer);
        let mut event = parse_event(&self.tokenizer, &mut self.state)?
            .ok_or_else(|| Error::new(ErrorKind::InvalidState, position))?;
        self.state.lend_event(&mut event);
        Ok((event, origin))
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
            if let Some((kind, bytes, span)) = self.tokenizer.current() {
                // Recovery reads a DOCTYPE inside the root as invalid markup.
                if kind == TokenKind::Dtd && !(self.state.recovers() && self.state.in_content()) {
                    return Ok((consumed, None));
                }
                let origin = SourceOrigin::input(span.source.clone());
                let raw = validate(kind, bytes, span)?;
                let event = parse_valid(kind, raw, span, &mut self.state)?;
                deliver(
                    &mut self.state,
                    event,
                    origin,
                    standalone,
                    last_origin,
                    emit,
                )?;
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
            self.drive_chunk(input, &mut consumed, standalone, last_origin, emit)?;
            if consumed == input.len() {
                return Ok((consumed, Some(false)));
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

    /// Parses the complete tokens at the front of a plain UTF-8 chunk in
    /// place, as the slice reader does: the chunk's valid prefix is checked
    /// once, and tokenizer and decoder counters (including a text token's
    /// consumed `<`) advance exactly as buffering each token would. Stops
    /// before anything the incremental tokenizer must classify or diagnose.
    fn drive_chunk(
        &mut self,
        input: &[u8],
        consumed: &mut usize,
        standalone: &mut bool,
        last_origin: &mut Option<SourceOrigin>,
        emit: &mut impl FnMut(Event<'_>, SourceOrigin, &mut Budget) -> Result<(), Error>,
    ) -> Result<(), Error> {
        // Short remainders rarely hold a whole token; the tokenizer takes them.
        if input.len() - *consumed < MIN_CHUNK_WINDOW {
            return Ok(());
        }
        let Some((base, false)) = self.tokenizer.idle_utf8() else {
            return Ok(());
        };
        let Some(allowance) = self.decoder.utf8_allowance() else {
            return Ok(());
        };
        let window = &input[*consumed..*consumed + allowance.min(input.len() - *consumed)];
        let text = valid_prefix(window);
        let limit = self.tokenizer.borrow_limit();
        // `at` starts the next token; `used` counts consumed window bytes,
        // which include a text token's terminating `<`.
        let (mut at, mut used) = (0, 0);
        while let Some((kind, length)) = complete_token(&text.as_bytes()[at..]) {
            if length > limit {
                break;
            }
            let end = at + length;
            let through = end + usize::from(kind == TokenKind::Text);
            self.tokenizer.skip_borrowed(kind, base + end);
            self.decoder.skip_utf8(through - used);
            *consumed += through - used;
            used = through;
            let range = base + at..base + end;
            let span = TokenSpan {
                decoded: range.clone(),
                source: range.clone(),
                encoding: SourceEncoding::Utf8,
            };
            let event = parse_valid(kind, &text[at..end], &span, &mut self.state)?;
            let origin = SourceOrigin::input(range);
            deliver(
                &mut self.state,
                event,
                origin,
                standalone,
                last_origin,
                emit,
            )?;
            self.release();
            if self.state.encoding() != SourceEncoding::Utf8 {
                break;
            }
            at = end;
        }
        Ok(())
    }

    /// Processes the ready token, which `is_non_event` classified.
    pub(crate) fn skip_non_event(&mut self) -> Result<(), Error> {
        parse_event(&self.tokenizer, &mut self.state).map(drop)
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

/// Smallest remaining chunk worth scanning for complete tokens in place.
const MIN_CHUNK_WINDOW: usize = 32;

/// Delivers one parsed event to a callback, lending its attribute list from
/// storage whose capacity returns to the parser afterwards.
#[inline(always)]
fn deliver(
    state: &mut ParserState<'static>,
    event: Option<Event<'_>>,
    origin: SourceOrigin,
    standalone: &mut bool,
    last_origin: &mut Option<SourceOrigin>,
    emit: &mut impl FnMut(Event<'_>, SourceOrigin, &mut Budget) -> Result<(), Error>,
) -> Result<(), Error> {
    let Some(event) = event else {
        return Ok(());
    };
    if let Event::Declaration(declaration) = &event {
        *standalone = declaration.standalone == Some(true);
    }
    *last_origin = Some(origin);
    // Each lending event is built directly as the callback's argument.
    let mut lent = Vec::new();
    let delivered = match event {
        Event::Start(Element {
            mut name,
            attributes: Attributes::Owned(list),
        }) if list.capacity() > 0 || state.lends_names() => {
            lent = list;
            let budget = state.lend(&mut name, &mut lent);
            let element = Element {
                name,
                attributes: Attributes::Borrowed(&lent),
            };
            emit(Event::Start(element), origin, budget)
        }
        Event::Empty(Element {
            mut name,
            attributes: Attributes::Owned(list),
        }) if list.capacity() > 0 || state.lends_names() => {
            lent = list;
            let budget = state.lend(&mut name, &mut lent);
            let element = Element {
                name,
                attributes: Attributes::Borrowed(&lent),
            };
            emit(Event::Empty(element), origin, budget)
        }
        Event::End(mut name) if state.lends_names() => {
            let budget = state.lend(&mut name, &mut []);
            emit(Event::End(name), origin, budget)
        }
        event => emit(event, origin, state.owned_budget()),
    };
    delivered?;
    if lent.capacity() > 0 {
        // Only the allocation returns; it no longer borrows the state.
        lent.clear();
        let lent = lent.into_iter().map(|_| unreachable!("cleared")).collect();
        state.recycle_attributes(lent);
    }
    Ok(())
}

/// The longest prefix of `input` that is valid UTF-8 made of XML characters.
pub(crate) fn valid_prefix(input: &[u8]) -> &str {
    let text = std::str::from_utf8(input).unwrap_or_else(|error| {
        std::str::from_utf8(&input[..error.valid_up_to()]).expect("validated prefix")
    });
    &text[..invalid_char(text).unwrap_or(text.len())]
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
    parse_valid(kind, raw, span, state)
}

#[inline(always)]
fn parse_valid<'token>(
    kind: TokenKind,
    raw: &'token str,
    span: &TokenSpan,
    state: &mut ParserState<'static>,
) -> Result<Option<Event<'token>>, Error> {
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
    // Invalid markup can be recognized on a leading UTF-8 byte, so it can end
    // inside a scalar: only its complete prefix is read (as recovered text).
    let raw = if matches!(kind, TokenKind::InvalidMarkup) {
        valid_prefix(bytes)
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
