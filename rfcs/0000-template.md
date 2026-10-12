# RFC-NNNN: Title

- **Status:** Draft
- **Author:**
- **Created:** YYYY-MM-DD
- **Decider:** the language owner
- **Edition:** 2026 (additive) | 2027 (breaking, migrated by `krit fix`)
- **Earliest release:**
- **Security label:** yes | no (required for any change to `wit/`, the
  annotations sidecar, the authority snapshot, or a verify scheme)
- **Related:** roadmap sections, specifications, RFCs, issues

Write today's Krit in ```` ```krit ```` blocks: CI extracts every one and
requires it to format, check with no diagnostics, and run with the real
compiler. Write proposed syntax in ```` ```text ```` blocks. Delete the
guidance under each heading when you fill it in.

## Summary

One paragraph: what changes, for whom, and why now.

## Motivation

- **Pull.** Which reference app, at least two partner bots, or at least three
  KritBench tasks need this? Link them.
- **Before and after.** Show the shortest real program the change improves,
  as it is written today and as it would be written with the change.
- **Layer.** Explain why the cheapest layer that works is this one: host
  configuration, then the standard library, then local sugar, then the type
  system, then runtime semantics.

## Syntax and semantics

- The exact grammar, as an EBNF delta against `spec/LANGUAGE.md`.
- The meaning: a desugaring into existing Core, or a new verified Core
  operation with its verification rule.
- Typing and effect rules (`spec/TYPES-AND-EFFECTS.md`), including what is
  rejected and with which diagnostic code (allocated in roadmap §9.2).
- Evaluation order, failure behavior, and what the formatter prints: every
  construct has exactly one canonical spelling.
- The specification text to add or change, by document and section.

## Alternatives

Each serious alternative, including doing nothing, with the reason it loses.
Name the roadmap decisions this RFC confirms, revises, or reopens.

## Readability and AI-auditability

- `krit metrics` deltas on `examples/` and the apps: lines, nesting depth,
  nested matches, `record` count, tokens, longest line, and authority literals.
- Does a reviewer still see every exit, every effect, and every authority
  literal where it happens? No hidden control flow, authority, or conversion.
- **Habits.** The three mistakes models are most likely to make with the new
  form, and the coded diagnostic and fix each one gets.
- KritBench deltas on the frozen set (no statistically significant
  regression), and reviewer-study results where readability is the claim.

## Security, capability, and determinism

- Effect on literal resources, manifest grants, secret bindings, the
  permission report and authority card, and host enforcement.
- New untrusted inputs or sinks, and how provenance treats them.
- Determinism: evaluation order, host-call order, and replay identity.
- Changes motivated by least authority, secure failure, or sandboxed
  deployment (charter 5, 10, 11) report benchmark deltas that never block.

## Conformance

The implementation-neutral cases to add under `conformance/` (run, check,
format, build, package, durable), including one case per new diagnostic code
and a differential case for every target.

## Compatibility and migration

- Is the change additive in edition 2026? If not, which edition carries it,
  and does `krit fix` migrate it mechanically?
- Machine fixes never change authority: a fix that adds, changes, or removes a
  resource literal, manifest entry, secret binding, entrypoint kind, or
  idempotency flag is at most `maybe`, and `krit fix` aborts when the
  permission report changes.
- Effect on existing artifacts, durable records, and the prompt pack.

## Performance

Compile-time, artifact-size, and runtime cost, or the measurement plan that
will establish them (`benchmarks/`).

## Cost

Files and crates touched (more than ten means a refactor lands first), and
the estimate in person-weeks.

## Unresolved questions

What must be decided before acceptance, and what can wait for implementation.

## Decision

Left to the language owner. Record the decision, its date, and its reasons
here, or for a rejection, the reopen trigger.
