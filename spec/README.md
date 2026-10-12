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
| [LANGUAGE.md](LANGUAGE.md) | Normative | Krit 0.3 source syntax, grammar, sugar and its meaning, standard library, and runtime semantics (edition 2026) |
| [DIAGNOSTICS.md](DIAGNOSTICS.md) | Normative | Every diagnostic code with its severity, the human and JSON Lines formats, fix hints, and exit statuses |
| [TYPES-AND-EFFECTS.md](TYPES-AND-EFFECTS.md) | Normative (open decisions draft) | Static types, `?` and let-else typing, match exhaustiveness, effects, capability requirements, and lints |
| [WASM-SANDBOX.md](WASM-SANDBOX.md) | Normative | Deployable subset (policy 1 modules; policy 2 entrypoints with strings, `?` exits, nested matches, and the standard library), artifact compatibility window, and the Wasmtime 48 LTS host |
| [CAPABILITIES.md](CAPABILITIES.md) | Normative (open decisions draft) | Authority model, manifest grants, secret-to-origin binding (`K5006`), and fail-closed dependencies (`K6003`) |
| [WEBHOOK-CONTRACTS.md](WEBHOOK-CONTRACTS.md) | Normative | Typed webhook, config, opaque secrets, exact-origin HTTP, and the request path rule |
| [AI-OBSERVABILITY.md](AI-OBSERVABILITY.md) | Normative bounded runtime | Neutral AI invocation, structured logs, retries, rate, cancellation, idempotency, and approval |
| [DURABLE-STATE.md](DURABLE-STATE.md) | Normative bounded local host | Transactional local state, checkpoints, replay, and durable idempotency |
| [JOBS-AND-STORAGE.md](JOBS-AND-STORAGE.md) | Normative bounded local host | Durable queues, host-owned scheduled triggers, and bounded object storage |
| [DATABASE.md](DATABASE.md) | Normative bounded local host | Capability-scoped catalogued parameterized database transactions |
| [CACHE-AND-SEARCH.md](CACHE-AND-SEARCH.md) | Normative bounded local host | Bounded namespaced TTL cache and provider-neutral search and vector connectors |
| [GUIDED-AUTHORING.md](GUIDED-AUTHORING.md) | Implemented baseline | Offline LSP facts, repair-oriented diagnostics, and review-gated provider-neutral suggestions |
| [PACKAGES.md](PACKAGES.md) | Draft (single-package manifests implemented) | Manifests and secret bindings (implemented); modules, lockfiles, and registries (draft) |
| [AGENT-APPLICATIONS.md](AGENT-APPLICATIONS.md) | Draft | Agent, bot, backend, and integration application model |

`LANGUAGE.md` deliberately defines a new readable syntax for the Rust
implementation. The S-expression syntax in the archived Racket prototype is
not part of Krit.

Every `krit` code block in these documents, apart from the layout examples
of the formatter section of `LANGUAGE.md`, is a complete program that
`krit check` accepts without warnings; `crates/krit-cli/tests/doc_examples.rs`
checks them. Signatures and fragments are written as `text` blocks instead.

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
