# Allocation-free node string-value chunks

Add the same operation to the borrowing and owned node views:

```text
Node::text_chunks(self) -> impl Iterator<Item = &'document str> + 'document
OwnedNode::text_chunks(self) -> impl Iterator<Item = &'document str> + 'document
```

For a document or element, visit descendant Text and CDATA values in document
order, excluding comment and PI values. For a Text, CDATA, comment or PI leaf,
yield its own direct payload once. Empty containers yield no chunks; empty live
leaves yield one empty chunk. Keep each existing Text/CDATA boundary, including
adjacent and empty Text created by editing. Do not synthesize empty chunks for
containers, merge values, include attributes, or escape/reparse strings.

This follows the string value of supported node kinds in pinned libxml2 2.15.4
`xmlNodeGetContent`/`xmlBufGetNodeContent` (`tree.c`), while returning borrowed
pieces rather than an allocated string. Joining the pieces gives the same
value for those supported kinds. Entity nodes, attributes as nodes, namespace
nodes and DTDs are not added by this operation. It is not C API/ABI compatibility.

Keep `text()` unchanged: it still exposes only a direct leaf payload. Use the
existing iterative subtree iterator for `text_chunks()`. The operation needs
O(1) iterator state, allocates nothing and visits at most the selected subtree;
it does not scan unrelated siblings. A caller chooses whether/how to allocate
or stream the pieces. This avoids a speculative text-allocation error/limit
family before allocator-aware materialization is designed. No exact chunk
count is promised without iteration.

## Required evidence

- Public goldens cover both tree types, all supported node kinds, empty and
  mixed content, numeric/predefined reference normalization, Unicode/CR,
  ignored descendant comments/PIs, sibling boundaries and independent cursors.
- Owned checks also cover UTF-16 input, edited values, fresh/empty/adjacent
  Text/CDATA, permanent removal and linked document order after appending under
  an element followed by an epilog.
- Deep small-stack iteration proves no new recursive walk. Partial consumption,
  `count`, `last`, `nth`, and repeated exhaustion remain ordinary correct
  iterator behavior without an `ExactSizeIterator` claim.
- Fuzz checks compare borrowing whole-document chunks with independently
  captured reader events; owned parsed views with the borrowing reference;
  and mutated owned trees with the independent logical model. Update the input
  target inventory. Deliberately admitting comments, skipping CDATA or leaking
  siblings must fail their expected tests, then be restored.
- Run the pinned external oracle on concatenated string values for selected
  valid UTF-8/UTF-16 cases. Record exact scopes/options and failures.
- Full tests, doctests, strict Clippy, formatting, dependency/unsafe checks,
  measured fuzz-only coverage and independent review precede integration.

Do not add a serializer, a text-copy API, namespace mutation, DTD/entity support,
or node-type aliases in this change. Existing handles, limits and provenance
remain unchanged.
