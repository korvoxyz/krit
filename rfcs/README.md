# Krit RFCs

An RFC proposes a change to the Krit language, its standard library, its
authority model, or a contract that programs depend on. It extends the change
process in [`spec/README.md`](../spec/README.md#change-process) and
[`CONTRIBUTING.md`](../CONTRIBUTING.md#language-changes) with the admission
rule in [`docs/roadmap.md`](../docs/roadmap.md) §3.4.

## Process

1. Copy [`0000-template.md`](0000-template.md) to `NNNN-short-name.md` with the
   next free number and status **Draft**.
2. Fill in every section. Write today's Krit in ```` ```krit ```` blocks; CI
   extracts each one and requires it to format, check with no diagnostics,
   and run with the real compiler (`crates/krit-cli/tests/prompt_examples.rs`).
   Write proposed syntax in ```` ```text ```` blocks, which are not checked.
3. Measure what the RFC claims: `krit metrics` deltas on the examples and
   apps, and KritBench deltas where the roadmap requires them.
4. The language owner decides. An accepted RFC lands with its specification
   text and conformance cases; a rejected one stays here with its reasons and
   a reopen trigger.

Status values: **Draft** (under discussion), **Accepted** (approved for
implementation), **Implemented** (normative in `spec/`), **Rejected**, and
**Withdrawn**.

## Index

| RFC | Title | Status |
|---|---|---|
| [0000](0000-template.md) | Template | n/a |
| 0001–0006 | Reserved by the roadmap (§4, 0.3 "RFCs first"): one semantics, flat sugar, diagnostics schema 2, editions and naming, durable protocols, guest runtime strategy | not yet written |
| [0007](0007-resource-handles.md) | Resource handles | Draft |
