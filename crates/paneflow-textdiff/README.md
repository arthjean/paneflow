# paneflow-textdiff

Line, word, and character comparison pipeline for the diff dock, ported from
the IntelliJ Platform comparison utilities (`ByLineRt`, `ByWordRt`, `ByCharRt`,
`LineFragmentSplitter`, `ComparisonManagerImpl`). The only external dependency
is `imara-diff`, used as the longest common subsequence engine with
`Algorithm::Myers`. See `NOTICE` for the attribution.

## Public API

- `compare_lines(lines1, lines2, policy) -> Vec<Range>`: line pass, never fails.
- `compare_lines_inner(text1, text2, policy, highlight) -> Vec<LineFragment>`:
  line pass plus word highlighting inside each changed block, squashed.
- `compare_words` and `compare_chars`: fine passes on a single block, returning
  `Err(DiffTooBig)` when one side exceeds 20 000 chunks.
- `split_lines`: the `\n` line splitter that every offset in the crate assumes.
- `BlockTracker`: the incremental block model behind the editor's git markers,
  ported from `DocumentTracker` in the IntelliJ Platform. `range_changed`
  shifts and merges blocks on every edit without diffing, and `refresh_dirty`
  re-diffs only the dirty blocks against the base, falling back to a whole
  block marked `too_big` past `TOO_BIG_BLOCK_LINES`.

Offsets are byte offsets into the input `&str`, always on char boundaries.

## Whitespace policy

`\r` is whitespace everywhere: unimportant-line counting, `TrimWhitespaces`,
`IgnoreWhitespaces`, word boundaries, and the punctuation matcher. IntelliJ
only treats `\r` as whitespace in some of these places because its documents
are normalized to `\n` before comparison. Paneflow compares raw file content,
so CRLF files behave like LF files under every policy.

## Documented deviations from the IntelliJ fixtures

The oracle tests under `src/oracle/` port `LineComparisonUtilTest.kt`,
`SplitComparisonUtilTest.kt`, `WordComparisonUtilTest.kt`,
`CharComparisonUtilTest.kt`, and `TrimUtilTest.kt`. IntelliJ resolves LCS ties
with its own Myers implementation, and `imara-diff` picks a different but
equally minimal alignment in three fixtures. Myers, minimal Myers, and
Histogram all produce the same three deviations, so the choice of algorithm
cannot close them. Each fixture is pinned to the output this crate produces so
a change in `imara-diff` shows up as a failing test.

| Fixture | Inputs | IntelliJ | This crate |
|---|---|---|---|
| `chars::non_deterministic_cases`, raw pass only | `" x \n y \n z "` vs `"x z"` | `-  ------ -` | `- -- ---- -` |
| `chars::two_steps`, raw pass only | `"😂🤫 🧒"` vs `" 🔫🤫🧒 "` | `--   --` / `---  -- ` | `----   ` / ` ----  -` |
| `words::algorithm_specific` | `"A B\nC D"` vs `"A\nB C\nD"` | Default ` -  -- ` / ` - --  ` | Default ` --  - ` / `  -- - ` |

The two character cases only differ in the raw `by_char::compare` pass. The
policy-aware `compare_chars` entry point, which runs the two-step correction,
matches IntelliJ on both inputs and keeps the original expectations. In the
word case the Trim and Ignore policies follow the same alternate alignment
(`" --  - "` / `"  --   "` and `"  -    "` / `"  -    "`). All three results
are valid minimal diffs: the changed ranges differ only in which of two equal
tokens gets attached to the change.
