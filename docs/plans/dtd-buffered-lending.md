# Private DTD buffered lending adapter

Compose the reviewed StreamingSession over caller-provided BufRead in the private
`stream::dtd::buffered` module. Existing public BufferedReader remains unchanged.

`DtdBufferedReader` offers new(input, SessionLimits), with_limits(input, Limits,
SessionLimits), with_decode_limits(input, Limits, DecodeLimits, SessionLimits),
read_event, position, event_origin, get_ref and into_inner. Its error preserves either the exact
SessionError or the original io::Error. No event is cached inside its source.

At each mutable read, mark the adapter poisoned before acknowledging the prior
lent event, changing session state or calling the source. Drain prepare(empty)
(and finish_step after physical EOF) before fill_buf, including generated content
and a held DTD End. On Ready, lend once and retain its source until the next read.
Successful EOF repeats None without further I/O; extraction/drop never drains.

Commit exactly successful SessionStep.consumed with consume before lending. On a
prepare error, commit checked session.position()-before, matching BufferedReader's
existing error-progress contract. Validate consumed <= offered buffer (and source
progress geometry); impossible progress yields terminal InvalidState without an
invalid consume. Nonempty input returning NeedInput without consuming transport is
terminal InvalidState, preventing a refill loop. Never count prefetched bytes or replacement work. Limit offered
transport by the remaining whole-input ceiling, checking physical EOF before
rejecting a known byte beyond an exact ceiling. InputBytes uses the established
whole-input diagnostic offset zero.

Interrupted fill_buf is retried. Return the first other I/O error once, leaving
the adapter poisoned so later reads return Session(Xml InvalidState) without I/O.
Session errors are exact and sticky. A caught fill_buf/consume panic cannot resume.

Author tests use real sessions and transport, with scripted BufRead failures only
at the I/O boundary. Cover bounded buffers/UTF16, DTD/GE drain ordering, consume
before publication/error, exact input limits, error identity, panic poison, EOF,
extraction/drop, and deliberate production faults. No public activation or
conformance/fuzz/performance claim belongs to this slice.
