# Private DTD buffered lending adapter

Compose the reviewed `StreamingSession` over caller-provided `BufRead` in the
private `stream::dtd::buffered` module. The existing public `BufferedReader`
remains unchanged.

`DtdBufferedReader` offers `new(input, SessionLimits)`, `with_limits(input,
Limits, SessionLimits)`, `with_decode_limits(input, Limits, DecodeLimits,
SessionLimits)`, `read_event`, `position`, `event_origin`, `get_ref`, and
`into_inner`. Its error preserves either the exact `SessionError` or the original
`io::Error`. No event is cached inside its source.

At each mutable read, mark the adapter poisoned before acknowledging the prior
lent event, changing session state, or calling the source. Drain `prepare([])`
and, after physical EOF, `finish_step()` before `fill_buf`, including generated
content and a held DTD End. On Ready, lend once and retain its source until the
next read. Successful EOF repeats `None` without further I/O; extraction and drop
never drain.

Commit exactly a successful `SessionStep::consumed` with `consume` before
lending. On a preparation error, commit the checked
`session.position() - before`, matching `BufferedReader`'s existing error-progress
contract. Validate that progress equals the position delta and does not exceed
the offered buffer. Reject impossible geometry without an invalid `consume`.
Reject a nonempty input window that reports NeedInput without consuming transport
to prevent a refill loop. Never count prefetched bytes or replacement work.

Limit offered transport by the remaining whole-input ceiling. At an exact
ceiling, still call `fill_buf` to distinguish physical EOF, excess input, and a
late I/O failure. InputBytes keeps the established whole-input diagnostic offset
zero.

Retry `Interrupted`. Return the first other I/O error once, leaving the adapter
poisoned so later reads return `Session(Xml InvalidState)` without I/O. Session
errors are exact and sticky. A caught `fill_buf` or `consume` panic cannot resume.

Author tests use real sessions and transport, with scripted `BufRead` failures
only at the I/O boundary. They cover bounded buffers and UTF-16, DTD and general
entity drain ordering, consume-before-publication/error, exact input limits,
error identity, panic poison, EOF, extraction/drop, and deliberate production
faults. No public activation or conformance, fuzz, or performance claim belongs
to this slice. The next reviewed checkpoint is the private
[stable-input Reader](dtd-stable-reader.md), which returns input-lifetime events
without retaining a loan on reader state.
