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

**Status:** Complete

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

### `phase4-ai-observability`

**Status:** Complete

- provider-neutral `ai_invoke` with one typed AI component interface and the
  deterministic host-side `http-json` adapter
- typed ordered `log_info`/`log_error` events, invocation buffering,
  deterministic redaction, and stderr-only JSON Lines publication
- bounded safe-request retries, capped backoff and Retry-After handling,
  finite per-resource rate policy, and numeric attempt stats
- embedding cancellation checked before instantiation, on host calls and
  backoff, and by libcurl during active transfers
- process-local bounded TTL/LRU inbound idempotency with replay and conflict
  detection
- default-deny approval callbacks for AI and bearer HTTP, plus explicit
  noninteractive CLI allow entries
- strict schema-2 host policy with schema-1 migration compatibility
- auditable GitHub-like -> neutral AI -> messaging-like reference flow using
  only bounded loopback mocks in tests

Gate: the reference webhook agent calls GitHub, an AI adapter, and one
messaging adapter inside the sandbox with every permission visible. The gate
is demonstrated by `crates/krit-runtime/tests/phase4.rs` and
`examples/webhook-agent.krit`; artifact permissions separately report exact
grants, imports, and approval-required resources.

Phase 4 idempotency and rate state are process-local and best-effort. Model
output remains nondeterministic and untrusted. Deployment approval/grant
evaluation remains outside the local artifact report.

## Phase 5: guided authoring

**Status:** Complete

### `phase5-language-server`

**Status:** Complete

- synchronous stdio Language Server Protocol transport with UTF-16 positions
- push diagnostics using the stable compiler codes and spans
- canonical whole-document formatting and a deterministic format code action
- hover facts for inferred/declared types, effects, resources, symbols, and
  webhook/package context
- parser/type/field/symbol/built-in and manifest-resource completion for the
  implemented edition-2026 language
- top-level document symbols and schema-1 `krit/compilerFacts` responses with
  expressions, entrypoints, package metadata, requested/required permissions,
  grant status, reference status, and formatting edits
- bounded 16 MiB protocol frames, 1 MiB documents, 128-document state,
  256 KiB validated manifests, type rendering, completion/fact outputs, and
  fail-closed malformed update handling
- no evaluator/runtime invocation, package installation, provider call, or
  socket/network operation in the language-server code path
- `krit lsp` stdio integration with stdout reserved exclusively for protocol
  frames and operational failures on stderr

### `phase5-guided-assistance`

**Status:** Complete

- isolated `krit-assist` layer consuming schema-1 language-server compiler
  facts without adding authoring dependencies to compiler, package, Wasm,
  runtime, or LSP layers
- strict authoring protocol 1 for one digest/version-preconditioned package
  entry document and bounded UTF-16/byte text edits
- disabled-by-default explicit provider configuration with one generic
  HTTPS/loopback HTTP JSON adapter, no branded SDK, inherited proxy, redirect,
  embedded credential, runtime artifact, or provider-specific language rule
- explicit `krit assist inspect`, `suggest`, `review`, and `accept` workflow;
  generation never writes source and acceptance requires `--reviewed`
- package-root containment, canonical entry validation, `.kritignore`,
  descriptor-relative no-follow source reads, generated/non-Krit exclusion,
  bounded user-selected context, and capability/secret-like literal plus
  compiler-type/diagnostic resource redaction
- strict malformed, overlapping, stale, ignored, out-of-root, symlink,
  oversized, non-canonical, invalid, or cross-document proposal rejection
- deterministic unified diffs plus before/after diagnostics, top-level types,
  effects, required/requested/granted permission facts, and exact authority
  deltas
- canonical formatting, parse/analyze/Core checking, exact added-permission
  approval, manifest grant enforcement, and permission-preserving atomic
  exchange acceptance with displaced-source digest validation and rollback
- completion, diagnostic-repair, and semantic-cleanup intents all use the same
  visible untrusted proposal pipeline
- loopback fake-provider/reference-webhook acceptance coverage and AI-off
  compiler/runtime availability invariants

Gate: disabling AI changes neither source semantics nor build/runtime
availability.

## Phase 6: durable execution

**Status:** In progress

### `phase6-state`

**Status:** Complete

- transactional key/value state
- workflow checkpoints
- replay and durable idempotency records

Gate met for local single-host coordination: the checkpoint/replay integration
test cancels an invocation after one completed HTTP effect, rolls back its
checkpoint, restarts with a new `AgentHost`, reuses the durable result without
a second mock call, and commits the checkpoint. Provider-side and distributed
exactly-once behavior remain explicitly unclaimed.

### `phase6-jobs-storage`

**Status:** Not started

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
