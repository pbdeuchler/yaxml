use crate::encoding::{DecodeProgress, Decoded, Decoder, TokenSpan};
use crate::escape::xml_char;
use crate::parser::{Budget, ParserState};
use crate::tokenizer::{TokenKind, Tokenizer};
use crate::{DecodeLimits, Error, ErrorKind, Event, Limits, SourceOrigin};

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
            state: ParserState::new(limits, 0),
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

    pub(crate) fn is_non_event(&self) -> bool {
        self.tokenizer
            .current()
            .is_some_and(|(kind, bytes, _)| self.state.is_non_event(kind, bytes))
    }

    pub(crate) fn release(&mut self) {
        self.tokenizer.release();
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

    pub(crate) fn event(&mut self) -> Result<(Event<'_>, SourceOrigin), Error> {
        let position = self.position();
        parse(&self.tokenizer, &mut self.state)?
            .ok_or_else(|| Error::new(ErrorKind::InvalidState, position))
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
        .map(|event| event.map(|event| (event, SourceOrigin::input(span.source.clone()))))
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
    if let Some((relative, _)) = raw
        .char_indices()
        .find(|(_, character)| !xml_char(*character))
    {
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
