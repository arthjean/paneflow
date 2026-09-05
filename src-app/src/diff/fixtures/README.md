# Syntax priority regression corpus

The executable corpus lives in the adjacent parity_tests.rs module and is shared
with the editor's opening, incremental edit and deferred parse parity tests.
stock-priority-audit.txt freezes every conflicting stock capture on a node and
every changed byte range under the old narrowest/first rule and Zed's stack rule.
The test fails on any added, removed or changed range.

cpp-stock-0.23.4.scm is the unmodified MIT-licensed query from the crates.io
tree-sitter-cpp 0.23.4 package (SHA-256
52136576a9a9dacd9e95a8de0f351689bf46140738572ab4e9f24c9278e6b458).
Its audit uses the new pinned parser, which can parse the module fixture.
The old query is test data only; the runtime loads Zed's C++ query.

## Accepted changes

The full node and byte list is stock-priority-audit.txt. JSON, Python, Go, YAML,
C and C++ switch to Zed queries in this change; their intermediate stock
conflicts are recorded for the global priority transition, not retained as
the final output contract. Bash, TypeScript, TSX, Markdown block, CSS and HTML
have no changed byte ranges on the corpus.

The four retained stock queries are not reordered:

- TOML keys become property instead of type. A pair capture and its bare-key
  child can start together: auditing identical nodes alone misses this change.
- Java class names become type; declarations and calls become function.method.
- Ruby declarations and parameters receive their specific classes. An identifier
  in interpolation is treated as a possible implicit method call by the stock
  query, without local-variable semantic analysis. Its closing brace takes the
  later punctuation.bracket class, consistent with other neutral delimiters.
- HTML has no priority changes on the corpus.

The Zed C++ query adds concept and _parent to the deliberately unstyled captures;
module uses the existing neutral namespace role. Zed at the pinned revision
ignores the query's has-ancestor? predicate as well: syntax_map.rs only handles
has-parent? and not-has-parent?. No new predicate interpreter is introduced.
