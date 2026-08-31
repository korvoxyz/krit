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

**Status:** In progress; data syntax milestone complete

- records and built-in variants
- `Option<T>` and `Result<T, E>` annotations
- exhaustive list, Option, and Result matching
- parsed let, parameter, and return annotations
- dynamic deterministic JSON decoding and encoding
- name, type, and effect checking
- canonical formatter

Gate: webhook request, connector response, model output, and application errors
can be represented without untyped maps or hidden exceptions.

## Phase 3: WebAssembly boundary

- typed Core IR
- WebAssembly component generation
- artifact validation and hashing
- one Rust component host
- memory, fuel, stack, host-call, output, and deadline limits
- `krit explain` and effective `krit permissions`

Gate: a pure component behaves identically to the direct evaluator and cannot
access ambient host resources.

## Phase 4: stateless reference agent

- typed inbound HTTP/webhook interface
- bounded outbound HTTP client
- host-managed secret handles
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
