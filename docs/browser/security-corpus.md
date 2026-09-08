# Browser security corpus

The R2 release corpus is the eight SEC cases of the Linux Browser plan. EP-005
certifies the automated security baseline and native sandbox evidence for its
real Linux x86_64 reference; the three native-only cases remain release work in
EP-007. The single source of truth is `CORPUS` in
[src-app/src/browser/security_corpus.rs](../../src-app/src/browser/security_corpus.rs);
this document explains how to read it, not what it contains.

```sh
paneflow browser-security-corpus
```

The verb prints the corpus as JSON: for each case, the negative input, the
required result, the entry point it must be exercised from, and whether its
proof is `automated` or `native`. The counts at the end of the document say how
many of each. A qualification report is expected to carry this output next to
its own observations, so a reader can tell which refusals were proven by tests
and which needed a rendered session.

## Automated proofs

`automated` cases name a test. Running it runs the proof:

```sh
cargo test -p paneflow-app --locked browser::security_corpus
cargo test -p paneflow-browser-host --locked qualification
```

They exercise real entry points, not helpers written for the test: the
controller's `dispatch` for message and generation refusals, `origin_of` for the
forbidden schemes, `ProfileStore::open` for the root lock, and `verify_runtime`
for the runtime checksum.

## Native proofs

`native` cases cannot be closed by a test process. They need a qualified runtime,
a rendered session and, for the certificate case, a live navigation. Their
`detail` field states the configuration required. Until that configuration has
been exercised and archived, those cases stay unproven and the public R2 Linux
release stays unqualified, whatever the automated cases or EP-005 reference
certification report.

## Keeping the corpus honest

A test in the same file asserts that the corpus lists exactly SEC-01 through
SEC-08 and that every case names both an entry point and a proof. Adding a case
to the plan without adding it here fails that test.
