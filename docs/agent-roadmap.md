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

**Status:** Complete

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

**Status:** Complete

- typed durable queues with owner leases, bounded attempts, capped backoff, and
  terminal dead-letter outcomes
- host-owned scheduled triggers with durable UTC fire identities, bounded
  catch-up, and explicit misfire skipping
- capability-scoped bounded object storage backed by the same transactional
  store
- separate `queue.publish`, `queue.consume`, `schedule.trigger`, `object.read`,
  and `object.write` authority through parser, analyzer, Core, WIT, manifest,
  permissions, explain, LSP, and assist redaction
- deterministic store schema-2 migration that preserves schema-1 data and
  strictly rejects foreign, newer, or malformed schemas
- `krit worker --once` and `krit schedule --once [--now]` bounded dispatch paths
  with host-supplied wall time and no unbounded loops
- strict host config schema 4 that can only narrow manifest-requested queues,
  schedules, and buckets onto owner-only stores

Gate met: `crates/krit-runtime/tests/jobs.rs` interrupts a delivery after one
completed HTTP effect, rolls its checkpoint back, redelivers under a new
`Runtime` and `AgentHost`, reuses the durable replay result without a second
mock call, and commits the acknowledgement atomically with the checkpoint.
Lease recovery, retry-then-success, dead-letter exhaustion, schedule fire
recovery, and bounded object persistence are covered by the same suite, and
`crates/krit-cli/tests/cli.rs` proves the same behavior across real processes.

Single-host durability only. Cross-host coordination and provider-side
exactly once remain explicitly unclaimed.

## Phase 7: data services

**Status:** Complete

### `phase7-database`

**Status:** Complete

- opaque non-serializable `DatabaseTransaction` handles with explicit
  `db_begin_read`/`db_begin_write`, named parameterized query and execute, and
  explicit `db_commit`/`db_rollback`
- host-owned strict statement catalog validated against the live schema:
  one statement, ordinal placeholders, declared parameter types and result
  columns, read-only queries versus mutating executes, and rejected
  `PRAGMA`/`ATTACH`/`DETACH`/`VACUUM`/transaction-control/schema SQL
- separate `database.read` and `database.write` authority per named database
  through parser, analyzer, Core, WIT, manifest, permissions, explain, LSP, and
  assist redaction
- strict host config schema 5 that can only narrow manifest-granted databases
  and owns every path, mode, bound, and statement
- an isolated `krit-database` crate that shares no schema or migration logic
  with the Krit durable-state store and adds no dependency

Gate met: `crates/krit-runtime/tests/database.rs` and the checked-in
`examples/database-webhook.krit` and its operator-owned
`examples/database-webhook.schema.sql` run a parameterized mutation and query inside
one explicit transaction, prove injection payloads stay data, and prove an
unclosed, trapped, or cancelled invocation rolls the transaction back.

Krit does not claim atomicity between an application database and Krit state:
they are separate SQLite files, `db_commit` publishes immediately, and the
two-resource window is documented rather than hidden.

### `phase7-cache-search`

**Status:** Complete

- bounded namespaced TTL cache with explicit miss, expiry, eviction, and outage
  behaviour, per-namespace and global entry and byte budgets, LRU eviction, and
  exact replacement accounting
- provider-neutral `search.query` and `search.vector` connectors with a strict
  generic `http-json` transport and a deterministic local transport
- `cache.read`, `cache.write`, `search.query`, and `search.vector` effects with
  four least-authority WIT interfaces and host config schema 6

Database and cache access are not prerequisites for the reference agent.
Correctness cannot depend on cache availability: `cache_get` returns
`Result<Option<String>, String>`, so a hit, a miss, and an outage are three
distinct values source must handle, and the same artifact runs unchanged with
the cache configured or absent.
[`examples/cached-search.krit`](../examples/cached-search.krit) demonstrates the
whole path: read, fall back on a miss or an outage, store with an explicit time
to live, and answer identically either way.

The cache is process local, non-durable, and non-transactional. It is shared
across fresh Wasm stores on one host, lost on restart, and a trap or failed
delivery does not undo an earlier write. That is stated plainly because it is
exactly why a cached value may never be load bearing.

## Roadmap completion gate

**Status:** Complete for the bounded local roadmap

The six review blockers recorded after `7b1c424` are resolved:

| Blocker | Resolution | Regression coverage |
| --- | --- | --- |
| Delivery reservation ordering | Scheduler ownership precedes reservation and lasts through outcome commit. Elapsed host time accounts for compilation/waiting; cancellation and remaining lease time are rechecked before guest work. Failed outcome commits release their owned deliveries. | `crates/krit-runtime/tests/jobs.rs`: concurrent queue/schedule dispatch, cancelled waiters, SQLite contention, timestamp horizons, and outcome conflicts |
| Replay lease lifecycle | HTTP/AI replay refuses open database transactions before approval or durable access. Typed failures, host traps, cancellation, approval denial, and completion failures abort owned replay leases; cleanup errors retain the original failure. | `crates/krit-runtime/tests/state.rs`: both operation kinds, transaction rollback, immediate retries, and stable provider keys |
| Replay response bounds | Local/durable inbound responses and durable AI results obey current runtime/adapter limits, including body/header boundaries and host-generated rejection responses. | `crates/krit-runtime/tests/state.rs`: policy narrowing, restarted hosts, and inclusive bounds |
| Lower-layer protocol maxima | Public state/job policies and service configuration validate shared count, byte, time, retention, and catch-up bounds. Durations are whole milliseconds; `open_with_jobs` validates the complete configuration before opening stores. The CLI reuses these APIs. | `crates/krit-runtime/tests/configuration.rs`: direct embedding, every maximum/one above, and rejection before file creation or schema migration |
| Deployment entrypoint integration | Direct `run` rejects all deployment entrypoints. Human explain, LSP module/entrypoint facts, hover, completion, declaration ranges, and assistance redaction/permission review include queues and schedules. | CLI, LSP, and assist regression suites |
| Strict configuration maps | Every map in host schemas 1 through 6 rejects conflicting and escaped-equivalent duplicate keys before service construction. | `crates/krit-cli/src/host_config.rs`: every map family, legacy defaults, and side-effect-free rejection |

The 2026-09-05 completion gate passed workspace formatting, strict Clippy,
629 unit/integration/conformance/command-line tests, documentation tests, and
the release build. All nine checked-in example sources formatted and checked;
the root package and all seven example packages built as validated Component
artifacts with allowed exact Krit imports and no WASI. Repeated factorial
builds produced identical 1,393-byte components and metadata, and a long-lived
release host returned identical search results on a cache miss and subsequent
hit.

The release binary uses rustls/static-libcurl and bundled SQLite without
dynamic libcurl or SQLite linkage. The one public HTTPS smoke test remains
opt-in and was not part of this gate; no public-network success is claimed.

This gate closes implementation and integration work, not product validation
or production readiness. The next priority is realistic end-to-end agent
applications, measured authoring/review effort, and the missing durable/data
service performance baselines. General Wasm layouts, schema-directed JSON,
modules/build caching, distributed coordination, and multi-tenant OS isolation
remain outside this completed scope. New service breadth must be justified by
application evidence rather than by the roadmap's phase count.

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
