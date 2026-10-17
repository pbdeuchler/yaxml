# Borrowed physical DTD units and body-state handoff

Base: `f43f90b0b502fd1e913ee9128d116f9bd5ad8b67`. This is a private controller
foundation. It does not activate DTD parsing in Reader, push, buffered, DOM, or
owned parsing and makes no performance claim.

## Scope and invariants

The controller gains two structurally distinct input modes:

- `DtdController::new(...)` remains the streaming constructor. `advance(bytes,
  budget)` accepts caller-provided decoded UTF-8 and every published unit owns
  its raw spelling.
- A stable UTF-8 constructor stores one complete borrowed source window and its
  absolute start. `advance_borrowed(budget)` alone drives that window. It never
  accepts caller-provided bytes, so a unit cannot borrow from a source other
  than the controller's stored input. The constructor checks absolute range
  overflow, that the physical DTD start lies within the window, and that its
  relative position is a UTF-8 boundary.

Calling the input method for the other mode returns typed `InvalidState`.
Streaming `finish_step` remains available only in streaming mode. Borrowed mode
knows its real window end and seals the physical scanner itself when that end is
reached. It stops when the DTD completes and does not consume a body suffix.

Physical units in stable UTF-8 mode map their exact `FragmentOrigin::Input`
range into the stored string and construct `DtdUnit` directly from that slice.
The scanner remains the lexical boundary oracle; the mapping performs checked
subtraction, bounds and UTF-8-boundary checks, but does not compare or reparse
raw bytes. Replacement-created units always own their raw spelling. Streaming
physical units continue to own theirs. Validated token metadata is moved in all
modes and remains subject to the existing unit limits.

## Publication and ownership order

Every ordinary scanner unit is parsed and checked against its scanner family
before any conditional raw copy. Only a unit that passes grammar and family
checks is promoted to owned storage when its source mode requires ownership.
Consequently malformed input and a wrong scanner/unit family win over a zero
owned-copy ceiling in both input modes. Tests pin this private precedence.

Owned raw promotion alone charges the shared parser `Budget`, controller
`owned_bytes`, and the controller copy-work charge. Borrowed physical raw does
not charge those three quantities. Scanning, grammar, catalog, frame and
physical-byte accounting remain identical. Generated replacement units remain
owned and charged even in borrowed mode.

`current()` remains a lending compatibility view for existing private tests.
`take_current()` moves the validated unit and origin out of the controller and
performs the same release transition. It returns an owning Begin/Item/End event;
no event borrow crosses the next controller step.

## End handoff

After End has been taken or released and the controller is complete,
`into_body_state()` consumes the controller. It moves the same `DtdCatalog` and
the same `EntityFrames` object, preserving catalog state, frame nonce and frame
counters. It also moves controller limits, counters, physical position and the
DOCTYPE range needed by the later body controller. Physical and fragment
scanners, current-unit scratch, active DTD expansion state and the stable input
window are dropped. Calling the handoff before complete End returns typed
`InvalidState`.

No catalog clone, frame reconstruction, nonce reset or counter refund is
allowed. A later body controller must continue using the moved frame object for
general-entity occurrences.

## Verification slices

The first source checkpoint only establishes the compiling private API while
preserving the existing controller tests. A separate author suite then covers:

- stable-window start/end/boundary/overflow validation and wrong-mode calls;
- all physical units borrowing exact source slices, while generated units own;
- body suffix preservation and borrowed EOF failure;
- malformed and wrong-family input under a zero copy quota in both modes;
- shared-budget, owned-byte and work deltas for borrowed versus owned raw;
- `take_current` ordering and exact origins;
- complete-only handoff, catalog identity/effects, frame nonce continuation and
  unchanged controller/frame counters.

The test-only implementation keeps metadata limits explicit. Allocation
exhaustion is not fabricated, and public Reader or streaming activation remains
outside this slice.
