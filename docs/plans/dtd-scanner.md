# Incremental DTD scanner prototype

Base: f2bd0438804660f8b9004e59743011f45e85d50d. This private prototype extracts
bounded decoded UTF-8 units. It does not enable DTD acceptance, public events,
entity expansion, defaults, or I/O. The complete lexical-unit grammar remains
in the separate dtd-unit prototype; the semantic/frame activation gates in
[dtd-foundations.md](dtd-foundations.md) remain unchanged.

## Boundary with the existing tokenizer

Use a separate explicit DTD scanner rather than modifying Tokenizer now.
Tokenizer.release resets to content Text, maintains encoding-specific physical
spans, and recognizes <!DOCTYPE only to reject it. A DTD scanner must remain
inside a subset across unit releases, and must also scan replacement-frame
subset fragments without a physical header/closing delimiter. Keeping decoded
local coordinates here avoids conflating these with physical TokenSpan mapping.
A future controller can hand off the recognized prefix and map resulting spans;
this prototype does not perform that integration or duplicate a decoder.

## Interface and units

DtdScanner::new(mode, limits) starts decoded-local position zero. Modes are
Doctype (header plus internal subset/closing delimiter) and SubsetFragment
(between-declaration replacement text, EOF permitted between units; no closing
bracket or header permitted). advance(&[u8]) accepts arbitrary byte splits,
consumes only through the next ready unit, and returns its consumed count.
current() lends (kind, raw bytes, decoded-local range) until release().
finish_step() seals further advance calls (zero consumption) at final EOF; callers drain ready units and repeat it.
is_complete() permits handoff of the untouched suffix after a complete physical
DOCTYPE or finished fragment. position() and work() are local counters only.

Kinds: Header, Declaration, Comment, Instruction, ParameterReference, Whitespace,
End. Header includes its '[' or '>' terminator, matching DtdUnit::parse. A header
without a subset completes the scanner and emits no synthetic End unit. The
future logical Begin/End event adapter must split that real final delimiter's
origin; it must not invent an empty range. End is '] S? >'. Whitespace runs are
units, subject to the same explicit unit/scratch bounds. PE references are
complete '%Name;' units, using the XML Name grammar; namespace/context policy
remains with the engine.

Header/declaration quotes, comment '-->', and PI '?>' have persistent lexical
state across calls. An unquoted '>' ends a declaration, but a quoted '>' does
not. Internal conditional-section prefixes are rejected as unsupported syntax
in this internal-subset scanner. Declarations are candidate units: the separate
unit constructor checks their complete production, references and XML chars.
No accumulated prefix is rescanned per input byte or chunk. UTF-8 is checked
once on unit completion (and once at incomplete final EOF), not per prefix;
current lends bytes without rescanning them. The controller/unit constructor
may convert these validated bytes to &str safely.

## Limits and failures

Independent ceilings bound decoded bytes per unit, requested retained scratch
capacity, and cumulative consumed-byte work. Reservations are fallible and use
geometric growth, capped by both unit and scratch ceilings. Scratch is reused
across releases and never retains a whole subset. Work counts consumed bytes;
the fixed scanner state does constant work per byte plus bounded final UTF-8/
PE-name validation. Non-consuming whitespace lookahead is constant overhead.
No normalization is performed, including on replacement text or CR bytes.

Private Copy errors carry kind and exact decoded-local offset. Syntax errors
point at the offending byte; incomplete EOF points at position(); byte/scratch/
work limits point at the first byte that cannot be consumed; allocation errors
point at the growing unit's current position. Failure is sticky. At incomplete PE-reference EOF, a known invalid Name byte is reported before the missing semicolon; an otherwise valid Name remains UnexpectedEof. Ready units
remain lent until release; repeated advance while ready consumes zero. A
completed physical DOCTYPE leaves following XML bytes untouched. No physical
source progress or Error.offset conversion is inferred from local counters.

## Verification and staging

Wire the module only under cfg(test) while unactivated; do not add blanket
allow(dead_code) or production test-only helpers. Remove this staging boundary
when the reviewed scanner/grammar/frame engine is activated together.

Tests use literal unit/range catalogs, every split (including Unicode scalar
splits), one-byte feeds, quotes containing delimiters, comments/PIs, EOF at every
prefix, malformed structure, suffix preservation, readiness/release lifecycle,
exact limits and precedence, repeated-unit scratch reuse, and fixed deterministic
model inputs. Exercise both modes and all result/limit cases. Check work remains
linear and independent of feed partition. No dependency or unsafe code is added.
Focused tests, all-target/doc checks, strict Clippy and formatting are required;
independent review/tests precede any integration. Full fuzz/XMLTS/timing need a
later activated-source freeze and coordinated runs; these prototype tests cannot
support DTD compatibility claims.
