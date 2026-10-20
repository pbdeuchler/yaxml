use crate::encoding::{DecodeProgress, Decoded, Decoder, SourceEncoding, TokenSpan};
use crate::escape::invalid_char;
use crate::parser::{Budget, ParserState};
use crate::tokenizer::{TokenKind, Tokenizer, complete_token};
use crate::{Attributes, DecodeLimits, Error, ErrorKind, Event, Limits, SourceOrigin};

pub(crate) mod dtd;

/// Concrete incremental engine shared by callback and lending frontends.
pub(crate) struct Stream {
    tokenizer: Tokenizer,
    decoder: Decoder,
    state: ParserState<'static>,
    decoded_token_limit: usize,
    /// Under `RECOVER`; boxed, so strict streams stay small.
    recovery: Option<Box<Recovery>>,
}

/// `RECOVER` as the slice reader recovers ([`ParserState::rereads`]): a
/// token libxml2 ends early has its rest replayed as content, and invalid
/// UTF-8 ends the input.
#[derive(Default)]
struct Recovery {
    /// Decoded bytes scanned again before any further input, from `at`.
    replay: Vec<u8>,
    at: usize,
    /// The decoded offset of `replay[0]`.
    base: usize,
    /// Where the ready token ends once released (decoded, source offsets).
    end: Option<(usize, usize)>,
    /// The invalid UTF-8 where the input ends; it ends the parse.
    cut: Option<Error>,
}

impl Recovery {
    /// The state of a stream under `flags`, which recovers with `RECOVER`.
    #[cold]
    #[inline(never)]
    fn new(flags: crate::options::Flags) -> Option<Box<Self>> {
        flags.recovers().then(Box::default)
    }

    fn pending(&self) -> Option<&[u8]> {
        self.replay.get(self.at..).filter(|rest| !rest.is_empty())
    }

    /// Ends the ready token at `keep` of its `bytes`, starting at decoded
    /// `start`, and replays the rest, then the `<` a text token consumed: by
    /// stepping back when the token was itself replayed, else as a copy.
    fn unread(&mut self, bytes: &[u8], start: usize, keep: usize, lt: bool) -> Result<(), Error> {
        if self.pending().is_some() {
            self.at = start + keep - self.base;
            return Ok(());
        }
        let rest = &bytes[keep..];
        self.replay.clear();
        self.replay
            .try_reserve(rest.len() + 1)
            .map_err(|_| Error::new(ErrorKind::Allocation, start))?;
        self.replay.extend_from_slice(rest);
        if lt {
            self.replay.push(b'<');
        }
        (self.at, self.base) = (0, start + keep);
        Ok(())
    }
}

impl Stream {
    pub(crate) fn new(limits: Limits, decode: DecodeLimits, flags: crate::options::Flags) -> Self {
        Self {
            tokenizer: Tokenizer::new(limits.max_token_bytes, decode.max_decoded_token_bytes),
            decoder: Decoder::new(decode.max_decoded_bytes),
            state: ParserState::new(limits, 0)
                .copying_names()
                .lending_namespaces()
                .with_flags(flags)
                .rereading(true, false),
            decoded_token_limit: decode.max_decoded_token_bytes,
            recovery: Recovery::new(flags),
        }
    }

    /// A new stream that keeps this one's emptied allocations.
    pub(crate) fn recycled(
        self,
        limits: Limits,
        decode: DecodeLimits,
        flags: crate::options::Flags,
    ) -> Self {
        Self {
            tokenizer: self
                .tokenizer
                .recycled(limits.max_token_bytes, decode.max_decoded_token_bytes),
            decoder: Decoder::new(decode.max_decoded_bytes),
            state: self
                .state
                .recycled(limits)
                .copying_names()
                .lending_namespaces()
                .with_flags(flags)
                .rereading(true, false),
            decoded_token_limit: decode.max_decoded_token_bytes,
            recovery: Recovery::new(flags),
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

    /// Whether a ready DOCTYPE token starts the DTD session: recovery reads
    /// one inside the root as invalid markup.
    pub(crate) fn hands_over(&self) -> bool {
        !(self.state.recovers() && self.state.in_content())
    }

    /// Under `RECOVER`, whether a kept undeclared reference ends next.
    pub(crate) fn ends_reference(&self) -> bool {
        self.state.reference_ends()
    }

    /// Under `RECOVER`, whether invalid UTF-8 ended the input.
    pub(crate) fn is_cut(&self) -> bool {
        (self.recovery.as_ref()).is_some_and(|recovery| recovery.cut.is_some())
    }

    fn replaying(&self) -> bool {
        (self.recovery.as_ref()).is_some_and(|recovery| recovery.pending().is_some())
    }

    /// The error that ends a recovering parse: past invalid UTF-8 that ended
    /// the input, that error, as the slice reader reports it.
    pub(crate) fn terminal(&self, error: Error) -> Error {
        match self.recovery.as_ref().and_then(|recovery| recovery.cut) {
            Some(cut) if error.offset >= cut.offset => cut,
            _ => error,
        }
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

    /// [`Stream::release`] of a token loop that rereads (`REREADS`: under
    /// `RECOVER`): the rest of a token libxml2 ends early is reread.
    #[inline(always)]
    pub(crate) fn release_in<const REREADS: bool>(&mut self) {
        if REREADS
            && let Some((at, source)) = self
                .recovery
                .as_mut()
                .and_then(|recovery| recovery.end.take())
        {
            self.tokenizer.rewind(at, source);
        }
        self.release();
    }

    /// [`Stream::advance`] of a token loop that rereads (`REREADS`).
    #[inline(always)]
    pub(crate) fn advance_in<const REREADS: bool>(&mut self, input: &[u8]) -> Result<usize, Error> {
        match REREADS {
            true => self.advance_recovering(input),
            false => self.advance(input),
        }
    }

    /// [`Stream::finish_step`] of a token loop that rereads (`REREADS`).
    #[inline(always)]
    pub(crate) fn finish_in<const REREADS: bool>(&mut self) -> Result<bool, Error> {
        match REREADS {
            true => self.finish_recovering(),
            false => self.finish_step(),
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

    /// [`Stream::advance`] under `RECOVER`: replayed bytes come first, input
    /// past a cut is ignored, and each token is screened.
    #[cold]
    #[inline(never)]
    fn advance_recovering(&mut self, input: &[u8]) -> Result<usize, Error> {
        let recovery = self.recovery.as_mut().expect("recovering stream");
        if let Some(replay) = recovery.pending() {
            let replayed = self.tokenizer.advance(replay)?;
            recovery.at += replayed;
            if self.has_token() {
                return self.screen().map(|()| 0);
            }
        }
        if self.is_cut() {
            return Ok(0);
        }
        let consumed = self.advance(input)?;
        self.screen().map(|()| consumed)
    }

    /// Recovery screens the ready token: invalid UTF-8 ends the input there,
    /// the token's bytes before it replayed, and a character outside `Char`
    /// makes the parser repair from then on.
    fn screen(&mut self) -> Result<(), Error> {
        let Some((kind, bytes, span)) = self.tokenizer.current() else {
            return Ok(());
        };
        let valid = match std::str::from_utf8(bytes) {
            Ok(valid) => valid,
            // Invalid markup can end inside a character the next bytes complete.
            Err(error) if kind == TokenKind::InvalidMarkup && error.error_len().is_none() => {
                std::str::from_utf8(&bytes[..error.valid_up_to()]).expect("valid prefix")
            }
            Err(error) => {
                let (keep, start, source) =
                    (error.valid_up_to(), span.decoded.start, span.source.start);
                let recovery = self.recovery.as_mut().expect("recovering stream");
                recovery.replay.clear();
                recovery.unread(&bytes[..keep], start, 0, false)?;
                recovery.cut = Some(Error::new(ErrorKind::InvalidUtf8, source + keep));
                self.tokenizer.rewind(start, source);
                return Ok(());
            }
        };
        if invalid_char(valid).is_some() {
            self.state.repair_characters();
        }
        Ok(())
    }

    pub(crate) fn finish_step(&mut self) -> Result<bool, Error> {
        self.finish_input(true)
    }

    /// Ends the input: the decoder's, unless recovery cut it, then the
    /// tokenizer's.
    fn finish_input(&mut self, decoder: bool) -> Result<bool, Error> {
        let tokenizer = &mut self.tokenizer;
        let state = &mut self.state;
        let limit = self.decoded_token_limit;
        if !decoder
            || !self
                .decoder
                .finish_step(|decoded| decode(tokenizer, state, decoded, limit))?
        {
            tokenizer.finish_step().or_else(|error| {
                // Recovery reads a start tag cut off by the end as an empty
                // element. Otherwise a token cut off by EOF fails its start
                // check first, as in the slice reader.
                if error.kind == ErrorKind::UnexpectedEof
                    && state.rereads()
                    && tokenizer.finish_cut_start()
                {
                    return Ok(());
                }
                let cut = tokenizer.cut_kind();
                Err(
                    (cut.and_then(|kind| state.check_token_start(kind, error.offset).err()))
                        .unwrap_or(error),
                )
            })?;
        }
        Ok(self.has_token())
    }

    /// [`Stream::finish_step`] under `RECOVER`: replayed bytes come first,
    /// and a cut input has nothing more to decode.
    #[cold]
    #[inline(never)]
    fn finish_recovering(&mut self) -> Result<bool, Error> {
        loop {
            self.advance_recovering(&[])?;
            if self.has_token() {
                return Ok(true);
            }
            let decoder = !self.is_cut();
            self.finish_input(decoder)?;
            self.screen()?;
            // A cut replays the bytes before it.
            if self.has_token() || !self.replaying() {
                return Ok(self.has_token());
            }
        }
    }

    pub(crate) fn validate_finish(&self) -> Result<(), Error> {
        match self.recovery.as_ref().and_then(|recovery| recovery.cut) {
            Some(cut) => Err(cut),
            None => self.state.validate_finish(self.position()),
        }
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
        emit: &mut impl FnMut(&Event<'_>, SourceOrigin, &mut Budget) -> Result<(), Error>,
    ) -> Result<(usize, Option<bool>), Error> {
        if self.recovery.is_some() {
            return self.drive_recovering(input, finalizing, standalone, last_origin, emit);
        }
        self.drive_tokens::<false>(input, finalizing, standalone, last_origin, emit)
    }

    /// [`Stream::drive`] under `RECOVER`, out of the strict path.
    #[cold]
    #[inline(never)]
    fn drive_recovering(
        &mut self,
        input: &[u8],
        finalizing: bool,
        standalone: &mut bool,
        last_origin: &mut Option<SourceOrigin>,
        emit: &mut impl FnMut(&Event<'_>, SourceOrigin, &mut Budget) -> Result<(), Error>,
    ) -> Result<(usize, Option<bool>), Error> {
        self.drive_tokens::<true>(input, finalizing, standalone, last_origin, emit)
    }

    /// [`Stream::drive`]; `REREADS` is recovery ([`Recovery`]), so strict
    /// parsing compiles without its checks.
    #[inline(always)]
    fn drive_tokens<const REREADS: bool>(
        &mut self,
        input: &[u8],
        finalizing: bool,
        standalone: &mut bool,
        last_origin: &mut Option<SourceOrigin>,
        emit: &mut impl FnMut(&Event<'_>, SourceOrigin, &mut Budget) -> Result<(), Error>,
    ) -> Result<(usize, Option<bool>), Error> {
        let mut consumed = 0;
        loop {
            if let Some((kind, bytes, span)) = self.tokenizer.current() {
                if kind == TokenKind::Dtd && self.hands_over() {
                    return Ok((consumed, None));
                }
                let (event, origin) = if REREADS {
                    let recovery = self.recovery.as_mut().expect("recovering stream");
                    parse_recovering(&self.tokenizer, &mut self.state, recovery)?
                } else {
                    let raw = validate(kind, bytes, span, self.tokenizer.screened())?;
                    let event = parse_valid(kind, raw, span, &mut self.state)?;
                    (event, SourceOrigin::input(span.source.clone()))
                };
                deliver(
                    &mut self.state,
                    event,
                    origin,
                    standalone,
                    last_origin,
                    emit,
                )?;
                self.release_in::<REREADS>();
                // A kept undeclared reference ends where it starts.
                if REREADS && self.state.take_reference_end() {
                    let end = Some(Event::EntityEnd);
                    deliver(&mut self.state, end, origin, standalone, last_origin, emit)?;
                }
                continue;
            }
            if finalizing || (REREADS && self.is_cut()) {
                if self.finish_in::<REREADS>()? {
                    continue;
                }
                self.validate_finish()?;
                return Ok((input.len(), Some(true)));
            }
            if !REREADS {
                self.drive_chunk(input, &mut consumed, standalone, last_origin, emit)?;
            }
            if consumed == input.len() && !(REREADS && self.replaying()) {
                return Ok((consumed, Some(false)));
            }
            consumed += self.advance_in::<REREADS>(&input[consumed..])?;
            if !self.has_token() && !(REREADS && self.is_cut()) {
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
        emit: &mut impl FnMut(&Event<'_>, SourceOrigin, &mut Budget) -> Result<(), Error>,
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

/// Delivers one parsed event to a callback by reference: its names borrow
/// the parser's retained namespaces, and its attribute list's capacity
/// returns to the parser afterwards.
#[inline(always)]
fn deliver(
    state: &mut ParserState<'static>,
    event: Option<Event<'_>>,
    origin: SourceOrigin,
    standalone: &mut bool,
    last_origin: &mut Option<SourceOrigin>,
    emit: &mut impl FnMut(&Event<'_>, SourceOrigin, &mut Budget) -> Result<(), Error>,
) -> Result<(), Error> {
    let Some(mut event) = event else {
        return Ok(());
    };
    if let Event::Declaration(declaration) = &event {
        *standalone = declaration.standalone == Some(true);
    }
    *last_origin = Some(origin);
    let budget = state.lend_event(&mut event);
    emit(&event, origin, budget)?;
    if let Event::Start(element) | Event::Empty(element) = event
        && let Attributes::Owned(list) = element.attributes
    {
        // Only the allocation returns; it no longer borrows the state.
        let list = crate::parser::emptied(list);
        state.recycle_attributes(list);
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
    let raw = validate(kind, bytes, span, tokenizer.screened())?;
    parse_valid(kind, raw, span, state)
}

#[inline(always)]
fn parse_valid<'token>(
    kind: TokenKind,
    raw: &'token str,
    span: &TokenSpan,
    state: &mut ParserState<'static>,
) -> Result<Option<Event<'token>>, Error> {
    if span.encoding != SourceEncoding::Utf8 {
        return parse_decoded(kind, raw, span, state);
    }
    state
        .process_token(kind, raw, span.decoded.clone(), |raw, offset, budget| {
            budget.copy(raw, offset)
        })
        .map_err(|error| span.map_error(raw, error))
}

/// [`parse_valid`] for a decoded token: kept errors count source bytes, as
/// returned ones do.
#[inline(never)]
fn parse_decoded<'token>(
    kind: TokenKind,
    raw: &'token str,
    span: &TokenSpan,
    state: &mut ParserState<'static>,
) -> Result<Option<Event<'token>>, Error> {
    let recorded = state.errors().len();
    let event = state.process_token(kind, raw, span.decoded.clone(), |raw, offset, budget| {
        budget.copy(raw, offset)
    });
    (state.owned_budget()).remap(recorded, |error| span.map_error(raw, error));
    event.map_err(|error| span.map_error(raw, error))
}

/// The ready token's event under `RECOVER`, with its origin, which ends
/// where recovery rereads the token's rest (replayed once it is released).
#[cold]
#[inline(never)]
fn parse_recovering<'token>(
    tokenizer: &'token Tokenizer,
    state: &mut ParserState<'static>,
    recovery: &mut Recovery,
) -> Result<(Option<Event<'token>>, SourceOrigin), Error> {
    let (kind, bytes, span) = tokenizer.current().expect("a ready token");
    // Screened: only invalid markup can end inside a character.
    let valid = std::str::from_utf8(bytes).map_or_else(|error| error.valid_up_to(), str::len);
    let raw = std::str::from_utf8(&bytes[..valid]).expect("valid prefix");
    let recorded = state.errors().len();
    let event =
        state.read_token::<true, _>(kind, raw, span.decoded.clone(), |raw, offset, budget| {
            budget.copy(raw, offset)
        });
    // Recovered errors count source bytes, as returned ones do.
    (state.owned_budget()).remap(recorded, |error| span.map_error(raw, error));
    let event = event.map_err(|error| span.map_error(raw, error))?;
    let Some(at) = state.take_rewind() else {
        return Ok((event, SourceOrigin::input(span.source.clone())));
    };
    // From a character boundary past the start, as the slice reader rereads.
    let keep = (1..=(at - span.decoded.start).min(bytes.len()))
        .rev()
        .find(|&keep| bytes.get(keep).is_none_or(|&byte| (byte as i8) >= -0x40))
        .unwrap_or(bytes.len());
    let end = span.source.start + span.encoding.source_length(&bytes[..keep]);
    let start = span.decoded.start;
    recovery.unread(bytes, start, keep, tokenizer.has_lookahead())?;
    recovery.end = Some((start + keep, end));
    Ok((event, SourceOrigin::input(span.source.start..end)))
}

/// The token's text, checked to be UTF-8 made of `Char`s; `screened` bytes
/// are known to be `Char`s once they are UTF-8.
#[inline(always)]
fn validate<'token>(
    kind: TokenKind,
    bytes: &'token [u8],
    span: &TokenSpan,
    screened: bool,
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
    if !screened && let Some(relative) = invalid_char(raw) {
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
