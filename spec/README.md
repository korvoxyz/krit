# Krit specifications

This directory is the source of truth for Krit's language and ecosystem
contracts. Implementations must follow the normative documents and must not
infer semantics from historical interpreter behavior.

## Status levels

- **Normative:** implemented behavior required by the conformance suite.
- **Draft:** an accepted direction whose details may change before it becomes
  normative.
- **Exploratory:** a design under discussion and unsuitable for implementation
  commitments.

| Document | Status | Scope |
|---|---|---|
| [CHARTER.md](CHARTER.md) | Normative | Product and language principles |
| [LANGUAGE.md](LANGUAGE.md) | Normative | Krit 0.2 source syntax and runtime semantics |
| [DIAGNOSTICS.md](DIAGNOSTICS.md) | Normative | Human and machine diagnostic contract |
| [WEBHOOK-CONTRACTS.md](WEBHOOK-CONTRACTS.md) | Normative bounded runtime | Typed webhook, config, opaque secrets, and exact-origin HTTP |
| [AI-OBSERVABILITY.md](AI-OBSERVABILITY.md) | Normative bounded runtime | Neutral AI invocation, structured logs, retries, rate, cancellation, idempotency, and approval |
| [AGENT-APPLICATIONS.md](AGENT-APPLICATIONS.md) | Draft | Agent, bot, backend, and integration application model |
| [TYPES-AND-EFFECTS.md](TYPES-AND-EFFECTS.md) | Implemented baseline | Static types and effect checking |
| [CAPABILITIES.md](CAPABILITIES.md) | Implemented bounded HTTP host | Runtime authority and sandbox boundaries |
| [PACKAGES.md](PACKAGES.md) | Draft | Modules, manifests, lockfiles, and registries |
| [WASM-SANDBOX.md](WASM-SANDBOX.md) | Implemented Phase 4 | Policy-1 scalar and bounded stateless webhook components |
| [GUIDED-AUTHORING.md](GUIDED-AUTHORING.md) | Draft | Deterministic and optional LLM coding guidance |

`LANGUAGE.md` deliberately defines a new readable syntax for the Rust
implementation. The S-expression syntax in the archived Racket prototype is
not part of Krit 0.2.

## Authority

Krit and the Krit language are owned by Akshay Bhardwaj. The specifications
and implementation are available under the Apache License 2.0. That license
does not impose terms on programs written in Krit.

## Compatibility

Krit is pre-1.0. A document's status and the language edition identify its
compatibility contract:

- Normative behavior can change only with a specification change and matching
  conformance update.
- Draft behavior may change without migration support.
- Every package declares an edition. The first edition is `2026`.
- A compiler must reject a newer unknown edition rather than guessing.

## Change process

A language change must include:

1. Motivation and rejected alternatives.
2. Exact syntax and semantics.
3. Human-readability and AI-auditability analysis.
4. Security, capability, and determinism impact.
5. Conformance cases.
6. Compatibility and migration impact.
7. Performance impact or a measurement plan.

The specification and conformance cases are normative. Compiler internals,
optimization choices, and the archived `racket-v0.1.0` tag are not.
