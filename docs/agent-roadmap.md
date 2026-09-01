# Agent platform roadmap

**Status:** Accepted delivery order

Krit develops agent infrastructure only when the previous layer supports a
complete, reviewable application. A service is exposed as a narrow typed
capability implemented by the host, never as ambient process access.

## Phase 0: language baseline

**Status:** Complete

- readable source syntax
- deterministic parser and diagnostics
- immutable lexical values and functions
- Rust-only compiler/runtime bootstrap
- strict package manifest
- implementation-neutral conformance suite

## Phase 1: authority plan

**Status:** Complete

- validated configuration keys
- opaque secret names
- exact outbound HTTP origins
- `io.stdout`
- strict manifest validation
- `krit permissions` human and JSON output
- versioned provider-neutral prompt material for current implemented syntax
- `krit prompt` for Claude, ChatGPT, Gemini, and local models

This phase makes proposed authority reviewable. It does not perform external
operations yet.

## Phase 2: readable agent data

**Status:** Complete

- records and built-in variants
- `Option<T>` and `Result<T, E>` annotations
- exhaustive list, Option, and Result matching
- parsed let, parameter, and return annotations
- dynamic deterministic JSON decoding and encoding
- name, type, and effect checking (complete)
- canonical comment-preserving formatter (complete)

Gate met: webhook request, connector response, model output, and application
errors can be represented without untyped maps or hidden exceptions.

## Phase 3: WebAssembly boundary

**Status:** Complete for the policy-1 scalar/stdout host

- typed Core IR (complete)
- deterministic `krit explain` compiler facts (complete)
- stable `K7001` residual-layout diagnostics (complete for artifact policy 1;
  general specialization remains future work)
- stable rejection of layouts and captures outside the initial backend subset
  (complete)
- WebAssembly component generation for Int, Bool, Unit, non-capturing
  functions, control flow, checked operators, and scalar stdout (complete)
- explicit feature/import validation, schema-1 metadata, and exact-byte BLAKE3
  hashing (complete)
- one reusable-engine Rust component host with a fresh Store/instance per call
  (complete)
- memory, fuel, stack, host-call, output, and deadline limits (complete)
- effective artifact-aware `krit permissions` (complete)
- `krit sandbox` with no build or evaluator fallback (complete)
- direct-evaluator/component differential tests for the policy-1 subset
  (complete)

The phase is complete only for the current Int/Bool/Unit, non-capturing
function, control-flow, checked-arithmetic, and scalar-stdout subset. Full
language layouts and the HTTP/agent host belong to later phases.

## Phase 4: stateless reference agent

**Status:** In progress

### `phase4-agent-contracts`

**Status:** Complete

- explicit top-level typed `webhook fn` declaration
- fixed `HttpHeader`, `HttpRequest`, and `HttpResponse` contracts
- deterministic draft-2020-12 request and response JSON Schemas
- literal-resource `config.read` and `secret.read` compiler facts
- opaque `Secret` type and checked non-disclosure restrictions
- checked-in inbound HTTP, config, and secret WIT contracts
- deterministic human/JSON explain facts and fail-closed run/build boundaries

This milestone is compiler contracts only. It does not listen on sockets,
perform outbound HTTP/TLS, load configuration or secret values, or call an AI
provider.

### `phase4-http-runtime`

**Status:** Complete

- `krit invoke --request FILE` and loopback `krit serve [--once]`
- bounded canonical webhook request/response/string/header-list layouts
- exact effect-selected webhook worlds and revalidated artifact requirements
- explicit immutable host configuration and owner-only secret-file loading
- opaque zeroizing host-side secret resources and bearer-only consumption
- DNS-pinned, no-redirect, exact-origin outbound HTTP/TLS with independent
  connect/read/overall timeouts and body/header/call limits
- fresh Store/instance/handles per request with response/stdout rollback
- local deterministic integration and serve acceptance coverage

### Remaining Phase 4 work

- provider-neutral AI call
- structured redacted logs and traces
- retries, backoff, rate limits, deadlines, cancellation, and idempotency
- human approval before declared sensitive operations

Gate: the reference webhook agent calls GitHub, an AI adapter, and one
messaging adapter inside the sandbox with every permission visible.

## Phase 5: guided authoring

- language-server compiler facts
- parser/type/effect/capability completion
- deterministic fixes and formatting
- optional provider-neutral inline prediction
- accepted suggestions checked immediately
- semantic cleanup shown as a reviewable diff

Gate: disabling AI changes neither source semantics nor build/runtime
availability.

## Phase 6: durable execution

- transactional key/value state
- workflow checkpoints
- replay and idempotency records
- typed queues with retry and dead-letter outcomes
- scheduled triggers
- bounded object storage

Gate: interrupted agent work resumes safely without repeating completed
external side effects.

## Phase 7: data services

- capability-scoped parameterized database operations
- explicit transaction boundaries
- cache with namespace, TTL, size, and miss behavior
- search/vector connectors as libraries

Database and cache access are not prerequisites for the reference agent.
Correctness cannot depend on cache availability.

## Deferred platform work

- public package registry
- many connector providers
- arbitrary native extensions
- custom bytecode VM
- native compiler backend
- distributed agent coordination
- autonomous whole-project rewriting
- Krit-hosted model training

## Decision rule

A phase begins only when its capability is required by a representative agent
task and the previous gate passes. Infrastructure breadth is not progress if
application source becomes harder to understand.
