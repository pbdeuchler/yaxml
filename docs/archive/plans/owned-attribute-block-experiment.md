# Contiguous owned-attribute block experiment

Prototype attribute membership changes without changing `Content::Element`, the
public attribute iterator, or the parsed attribute layout. This is measurement
code on an isolated branch, not a public construction API or quota contract.

## Representation

Keep `OwnedDocument::attributes` as one `Vec<StoredAttribute>` and keep each
element's existing `Range<usize>`. Add one document-owned, append-only vector of
block descriptors sorted by physical block start. A descriptor records the
owning node slot and the block capacity. Parsed documents have no descriptors.

On an element's first membership change, append a new block, copy its live
metadata, fill reserved spare slots with private vacant values, append the
descriptor, then publish the new live range. Later appends fill spare slots.
When a block fills, append a geometrically larger block and publish it only after
all reservations and copies succeed. Old blocks and descriptors remain as
bounded geometric history. Removal shifts the live suffix within the current
block, clears the last live slot, and shrinks the range.

Descriptor lookup requires both the range start and owner slot. This
distinguishes parsed empty `0..0` ranges from a first edited block beginning at
zero, and it prevents another empty element at the same parsed boundary from
claiming that block. New block starts are strictly increasing, so descriptor
append order is search order.

## Bounds and failure model

Private test operations receive explicit ceilings for total physical attribute
slots and metadata copies. Initial migration copies the parsed live count;
growth copies the current live count; using spare capacity and removal copy no
metadata into a new block. With geometric growth, all obsolete edited blocks for
one element retain less than a constant multiple of its largest allocated block,
in addition to its one-time parsed baseline copy. Slots and descriptors are
never reclaimed in this experiment.

Every new block preflights arithmetic and both ceilings, reserves the global
attribute vector and descriptor vector fallibly, appends the complete block and
descriptor, and only then updates `Content::Element.attributes`. Safe
thread-local failpoints cover each reservation. A failure may retain capacity
but cannot change vector lengths, descriptors, ranges, values, counters, or
serialized output; the same editor remains usable. No allocator failure is
injected outside these reservation calls.

## Evidence and rejection gates

Tests must cover parsed empty and nonempty ranges, the block-at-zero ownership
case, spare-capacity appends, geometric growth, first/middle/last removal,
value edits through a migrated range, exact-size borrowed iteration,
serialization, counter accounting, arithmetic/ceiling errors, both reservation
failures, and recovery.

Measure unchanged `9e4b3a6` against this prototype with the same release profile,
locked dependency, counterbalanced runner, fixtures, observables, checksums, and
sample count used by the prior attribute experiment. First measure parsed
documents through public parse, traversal, value edit, serialization, and drop.
Then use only an ignored private unit-test driver to measure one and all migrated
elements outside phase clocks. Record layout, logical/physical slots, copied
metadata, descriptors, retained obsolete blocks, and exact output.

Reject the representation if parsed read paths retain membership dispatch or if
any stable parsed phase regresses by at least 5%. Treat edited results as scoped
cost evidence rather than a performance claim. Do not expose attribute mutation,
freeze counters or error types, add dependencies, use unsafe code, tag ranges,
or begin namespace/construction work in this experiment.
