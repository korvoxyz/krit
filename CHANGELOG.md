# Changelog

All notable Krit changes are documented here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Krit is pre-1.0 and does not yet promise stable syntax between minor releases.

## [Unreleased]

### Added

- Draft agent application and guided AI authoring specifications plus the
  WebAssembly sandbox design now backed by an artifact-policy baseline
- Narrow reference-agent MVP and explicit deferred scope
- Phased agent service roadmap from authority planning through durable state,
  queues, databases, and optional caches
- Strict configuration, secret, and outbound HTTP capability declarations
- Human and JSON `krit permissions` output for pre-deployment authority review
- Versioned provider-neutral Krit 0.2 generation prompt and usage contract
- Readable records with deterministic rendering and field access
- Built-in `Option` and `Result` values with exhaustive variant matching
- Parsed type annotations for bindings, parameters, returns, lists, records,
  options, and results
- Deterministic `json_encode` and dynamic `json_decode` built-ins
- Stable `K4008` and `K4009` JSON diagnostics
- Lexical name resolution with duplicate declaration checks
- Deterministic static inference and checking for primitive, list, record,
  Option, Result, and function types
- Defensive exhaustive-family checking and stable `K3001` through `K3006`
  diagnostics
- Sorted `io.stdout` effect inference with recursive and higher-order call
  propagation
- Public `krit::analyze` API with inferred binding types, source spans, and
  effects
- Implementation-neutral static `check` conformance fixtures
- Public `krit::format_source` API with deterministic edition-2026 formatting
- Lossless standalone and end-of-line `//` comment preservation
- `krit fmt [--check] FILE...` with batch validation, atomic same-directory
  replacement, and stable `K8001` check diagnostics
- Formatter fixtures and repository-wide parse, analysis, and idempotence
  coverage
- Resolved typed Core IR with deterministic binding, value, function,
  parameter, capture, closure, block, and match-binding IDs
- Public normalized symbol, expression, block, and resolved-name analysis facts
  used directly by Core lowering
- Explicit left-to-right ANF evaluation order, nested control-flow blocks,
  recursive self values, and lexical closure capture arguments
- Explicit Core identities for stdout built-ins, pure Option/Result
  constructors, and pure JSON conversion
- Synthetic typed `module-init` entrypoint with inferred effects
- Core verification for ID ranges, dominance, branch and call types, captures,
  operation types, and conservative effects
- Stable `CoreModule::render_text` output with golden and repository-corpus
  lowering coverage
- `krit explain [--json] FILE` with human compiler facts and deterministic
  schema-1 JSON serialized through `serde_json`
- Dedicated `krit-wasm` artifact crate consuming verified Core without
  reparsing or inference
- Checked-in `krit:runtime@0.2.0` WIT package with effect-selected
  `pure-program` and stdout `program` worlds
- Deterministic core Wasm and Component Model emission for Int, Bool, Unit,
  checked operators, control flow, recursion, and non-capturing higher-order
  calls
- Fail-closed K7001/K7002 backend diagnostics for residual types, composites,
  JSON, lexical captures, unsupported output, and unlowered operations
- Explicit core/component feature and import validation with no WASI, start,
  memory, threads, GC, exceptions, or component async, plus effects and world
  derived from the validated import surface rather than metadata claims
- Bounded embedded producers/custom metadata plus deterministic adjacent
  schema-1 metadata and exact final-byte BLAKE3 digests
- `krit build [--manifest PATH] [--output PATH]` with safe package entry
  resolution, capability checking, deterministic defaults, and rollback-safe
  output replacement
- Public artifact inspection, policy validation, and digest verification APIs
- `krit-runtime`, a reusable-engine Wasmtime component host with a fresh Store
  and instance per invocation, exact pure/stdout WIT linking, no WASI, bounded
  precompile inputs, StoreLimits, stack, fuel, serialized epoch deadlines,
  host-call limits, and rollback-safe buffered output
- `krit sandbox [--manifest PATH] [--artifact PATH]` with no automatic build or
  evaluator fallback and stable authorization/runtime exit statuses
- Artifact-aware human and JSON `krit permissions --artifact PATH` reports
  with complete denied output and deployment status kept `not-evaluated`
- Stable host diagnostics `K5002` and `K5101` through `K5105`
- Genuine Wasm integer-overflow traps for compiler checked arithmetic so an
  adversarial `unreachable` remains generic `K4001`
- Explicit top-level `webhook fn` declarations with stable duplicate,
  placement, and exact-signature diagnostics
- Fixed `HttpHeader`, `HttpRequest`, and `HttpResponse` built-in contract
  aliases with exact closed response checking and ordered duplicate headers
- Deterministic draft-2020-12 webhook request/response JSON Schemas in
  versioned human and schema-1 JSON explanation facts
- `config_string` and `secret` host-operation identities with direct
  string-literal resource enforcement, sorted `config.read`/`secret.read`
  effects, and separate resource-specific capability requirements
- Opaque `Secret` analysis/Core type and static rejection of printing,
  comparison, JSON encoding, and ordinary structural storage
- Stable `K1004`, `K3007` through `K3009`, and `K5003` diagnostics
- Checked-in HTTP, config, WIT-resource secret, and typed webhook contracts
- Conformance, formatter, Core golden, explain-schema, WIT parseability,
  direct-run denial, build fail-closed, and unchanged policy-1 artifact tests
- Direct normalized-origin `http_request(origin, request, bearer)` with exact
  `http.request` requirements and bearer-only opaque `Secret` consumption
- Shared URL-based normalized origin parsing for source and manifest checks
- Finite effect-selected webhook worlds, including a separate anonymous HTTP
  surface that does not implicitly import secret acquisition
- Bounded policy-2 canonical ABI lowering for strings, fixed HTTP records,
  header lists, selected Result/Option layouts and matching, and static helper
  references while preserving policy-1 scalar artifact bytes
- Schema-1 embedded and adjacent exact resource requirements revalidated
  against component-derived effects/imports
- Typed `krit-runtime` webhook invocation with fresh Stores, instances,
  resource tables, handles, and rollback-safe output/response publication
- Explicit immutable config values and host-owned zeroizing secret storage
- Exact-origin ordered-header outbound HTTP/TLS via statically linked,
  rustls-backed libcurl with native platform trust roots,
  environment proxies disabled, redirects denied, DNS results pinned per
  request, public-address policy, independent timeouts, and body/header/call
  limits
- Strict host config JSON with relative secret-file references, bounded reads,
  no inline/environment values, owner-only Unix permission enforcement, and
  no-follow descriptor opens
- `krit invoke --request FILE` and loopback `krit serve [--once]` over existing
  artifacts only, using `tiny_http` rather than a handwritten parser
- Auditable `examples/webhook-agent.krit` plus manifest, host config, and
  request fixtures containing no credential
- Provider-neutral fallible `ai_invoke("adapter", input)` source/Core
  contract with exact `ai.invoke` requirements and typed Component Model AI
  imports
- Manifest schema-1 `ai` and `logs` requests plus exact least-authority
  AI-only, log-only, and combined finite webhook worlds
- Deterministic host-side `http-json` AI adapter with strict request/response
  mapping, bounded input/output/timeout, optional opaque bearer secret, and no
  provider SDK dependency
- Typed ordered `LogField` values and fallible `log_info`/`log_error`
  operations with invocation-local sequencing, atomic validation, bounded
  buffering, key-based and exact-secret-value redaction, and separate result
  events
- Success/failure structured-log JSON Lines publication on CLI stderr without
  changing invoke response stdout or served HTTP bodies
- Reusable stateful `AgentHost`, embedding `CancellationHandle`, and
  `ApprovalPolicy` APIs
- Bounded transport retries for connection/timeouts and 429/502/503/504,
  restricted to GET/HEAD or explicit valid idempotency keys, with capped
  deterministic backoff and Retry-After handling
- Finite per-resource fixed-window AI/HTTP rates with bounded LRU tracking and
  visible guest errors
- Entry- and byte-bounded process-local TTL/LRU inbound idempotency replay,
  exact request digest conflicts, response replay without guest execution,
  and exclusion of failed/trapped invocations
- Default-deny AI and bearer-HTTP approval checks before secrets/network and
  before every retry, with approval-required artifact permission facts
- Strict host config schema 2 for adapters, approvals, retries, rates, and
  idempotency; schema 1 remains readable but bearer operations now require
  migration to explicit schema-2 approval
- Stable pre-execution cancellation diagnostic `K5106`, libcurl progress
  cancellation/deadline aborts, bounded DNS workers, and cancellation-aware
  backoff
- Bounded policy-2 unescaped JSON-string decoding for explicit reference
  model-output validation; all general JSON component shapes remain
  fail-closed
- Local-only reference integration coverage proving GitHub-like -> neutral AI
  -> messaging-like order, retry/auth boundaries, output/log/stats facts, and
  exact permissions without Internet access or real credentials
- `krit-lsp`, a synchronous stdio language server with full-document sync,
  UTF-16 position handling, stable compiler diagnostics, canonical formatting,
  a deterministic format code action, hover, completion, and document symbols
- Schema-1 `krit/compilerFacts` responses for module/entrypoint effects,
  literal-resource requirements, resolved symbols and expressions, inferred
  and declared types, package metadata, requested/required permission status,
  reference status, and canonical edits
- Package-aware configuration, secret, exact-origin HTTP, and AI adapter
  completion plus compiler-owned built-in signatures and built-in record field
  facts
- Bounded 16 MiB protocol frames, 1 MiB documents, 128-document state,
  256 KiB validated manifests, recursive type rendering, and bounded
  completion/symbol/compiler-fact output with fail-closed malformed handling
- In-memory protocol and real `krit lsp` stdio coverage for malformed
  requests/documents, UTF-16 ranges, deterministic facts, formatting
  idempotence, no-execution behavior, framing purity, and graceful shutdown
- `krit-assist`, an isolated authoring layer consuming the public bounded
  language-server compiler-facts API with no dependency leakage into compiler,
  package, Wasm, runtime, or LSP layers
- Strict provider-neutral authoring protocol 1 request, response, proposal,
  review, and acceptance schemas for one digest-preconditioned package entry
- Disabled-by-default explicit provider configuration and one generic bounded
  HTTPS/loopback HTTP JSON adapter with host-managed optional environment
  credentials, disabled inherited proxies/redirects, and no branded SDK
- `krit assist inspect|suggest|review|accept` with exact pre-provider context
  inspection, JSON Lines output, untrusted proposal artifacts, explicit
  `--reviewed`, and exact added-permission approval
- Canonical package containment, `.kritignore`, generated/non-Krit exclusion,
  descriptor-relative no-follow source reads, bounded UTF-16 source ranges,
  source/type/diagnostic resource redaction, host-local real digests, and
  strict stale/overlap/path/symlink/size validation
- Deterministic canonical unified diffs and before/after diagnostics, types,
  effects, requested/required/granted permissions, usage, missing grants, and
  authority deltas
- Canonical format/parse/analyze/Core validation plus permission-preserving
  atomic exchange single-source acceptance with pre-write staged permissions,
  displaced-source digest validation, stale rollback, and fail-closed
  acceptance on platforms without the audited exchange primitive
- Fake provider, loopback HTTP provider, malicious response/context, prompt
  injection, redaction, stale/atomic failure, permission escalation, semantic
  cleanup, AI-off, and reference webhook guided-edit coverage
- Normative durable-state/replay specification with explicit local crash,
  transaction, checkpoint, migration, filesystem, retention, and exactly-once
  limitations
- `krit-state`, a bundled-SQLite schema-1 transactional store using WAL,
  configurable FULL/NORMAL synchronization, application/schema identity,
  integrity validation, bounded page count, busy timeout, revision conflicts,
  replay leases/results, and durable idempotency records
- Edition-2026 `state_get`, `state_put`, `state_delete`, `checkpoint_get`,
  `checkpoint_put`, `replay_http`, and `replay_ai` built-ins with direct
  canonical store/checkpoint/operation/resource facts
- Manifest `state` grants, `state.transaction` analysis/Core/metadata facts,
  policy-2 state artifact validation, typed `krit:runtime/state@0.2.0` WIT,
  finite exact state world selection, and bounded Wasm lowering
- Invocation-local state/checkpoint overlays with revision-checked commit only
  after successful guest completion and rollback on traps, cancellation,
  deadlines, invalid responses, conflicts, and host failures
- Durable completed HTTP/AI replay with exact artifact/operation/input identity,
  current grant and approval rechecks, stable AI idempotency keys, safe/keyed
  HTTP enforcement, leases, expiry, LRU, and byte bounds
- Optional durable inbound `Idempotency-Key` reservations/responses scoped by
  artifact identity and credential-sensitive request digests, while schema
  1/2 and unconfigured schema 3 preserve process-local behavior
- Strict schema-3 host config with manifest-narrowed named stores, no default
  path, bounded durability/SQLite/replay policy, owner-only directories/files,
  symlink denial, and corrupt/newer/foreign database rejection
- Stateful checkpoint example plus compiler, WIT, artifact, runtime,
  restart, killed-writer recovery, cancellation, replay, idempotency,
  filesystem, CLI, and legacy factorial-byte regression coverage
- Normative durable queue, scheduled-trigger, and object-storage specification
  with lifecycle state machines, ordering, lease, retry, dead-letter, catch-up,
  atomicity, crash-window, filesystem, limit, and non-goal contracts
- Store schema 2 with strict `queue_jobs`, `queue_dead`, `schedule_fires`,
  `schedule_cursors`, and `objects` tables, indexed reservation and cleanup
  queries, and a deterministic in-place schema-1 migration that preserves data
  and still rejects foreign, newer, extra, or malformed schemas
- Contextual `queue "name" fn` and `schedule "name" fn` entrypoints with the
  fixed `QueueJob` and `ScheduleEvent` contracts and `Result<String, String>`
  outcomes, plus `queue_publish`, `object_get`, `object_put`, and
  `object_delete` built-ins with direct canonical resource facts
- Separate `queue.publish`, `queue.consume`, `schedule.trigger`, `object.read`,
  and `object.write` effects across parser, analyzer, Core, manifest
  (`queues`, `consumes`, `schedules`, `buckets`, `readOnlyBuckets`),
  permissions, explain, LSP completion, and assist redaction
- Typed `krit:runtime/queue`, `objects-read`, `objects-write`, `job`, and
  `schedule` WIT interfaces, deterministic least-authority world generation for
  every import mask, and validation that re-derives every effect from the real
  component imports and export
- Durable queue publish, reservation, acknowledgement, retry with capped
  backoff, attempt caps, dead letters, and owner leases whose expiry recovers
  interrupted work without holding a database lock across guest execution
- Host-owned scheduled triggers with UTC epoch occurrences, durable
  `(schedule, due)` fire identities, bounded catch-up with explicit misfire
  skipping, and shared retry/dead-letter handling
- Capability-scoped object buckets with bounded object count, key, value, and
  total bytes, replacement accounting, and deterministic host-side listing
- One-transaction outcome commits that bind staged state, checkpoints, object
  writes, queue publishes, and the delivery acknowledgement together
- `krit worker --queue NAME` and `krit schedule --schedule NAME` with `--once`,
  bounded `--max-deliveries`, explicit `--now` wall time, and schema-1 JSON
  delivery reports
- Strict host config schema 4 that binds manifest-granted queues, schedules,
  and buckets to already-configured owner-only stores and can only narrow them
- Enqueue, worker, and scheduled-trigger examples plus store, compiler,
  artifact, runtime, and real-process CLI coverage for FIFO delivery,
  concurrent reservation, lease recovery, retry-then-success, dead-letter
  exhaustion, schedule fire recovery, object bounds, schema migration, and
  non-repeated completed external effects

### Changed

- Selected WebAssembly components instead of custom bytecode as the first
  deployment target
- Defined LLM assistance as optional visible edits gated by deterministic
  formatting, checks, and permission analysis
- Updated the provider-neutral generation prompt to version 0.2.2 for readable
  data and static checking, to version 0.2.3 for canonical formatting, and to
  version 0.2.4 for the checked explanation workflow
- Made `krit check` perform semantic analysis without executing source while
  preserving its success output, then lower and verify typed Core IR
- Marked the readable-agent-data phase complete
- Completed Phase 3 for the policy-1 scalar/stdout subset: Core IR, component
  artifacts, bounded sandbox hosting, differential execution, and effective
  local permission inspection. Full-language layouts and agent interfaces
  remain later work.
- Updated the provider-neutral generation prompt to version 0.2.5 for the
  checked artifact-build workflow and strict backend subset
- Updated the provider-neutral generation workflow to version 0.2.6 with
  explicit sandbox execution and artifact permission-review commands
- Changed the Wasmtime requirement from an exact 47.0.4 pin to compatible
  47.0.4 while retaining the tested patch in `Cargo.lock`; documented the
  short non-LTS support window and planned audited migration to 48 LTS
- Made direct `krit run` fail with `K5003` when source requires an unavailable
  webhook/config/secret host, without changing existing dynamic conformance
- Marked `phase4-http-runtime` complete while keeping Phase 4 in progress for
  AI, observability, and reliability work
- Updated the provider-neutral generation prompt to version 0.2.7 for typed
  webhook, configuration, opaque-secret, and fail-closed runtime contracts
- Updated the provider-neutral generation prompt to version 0.2.8 for the
  buildable bounded webhook, explicit host-input, invoke, and serve workflow
- Completed `phase4-ai-observability` and the Phase 4 stateless reference-agent
  gate; Phase 5 LSP/guided authoring and Phase 6 durable state remain
  unstarted
- Updated the provider-neutral generation prompt to version 0.2.9 for neutral
  AI calls, structured logs, explicit model-output validation, and bounded
  reliability/approval policy
- Completed `phase5-language-server` while keeping provider-neutral prediction,
  accepted-suggestion checking, and semantic cleanup in the later
  `phase5-guided-assistance` milestone
- Updated the provider-neutral generation prompt to version 0.2.10 for the
  offline language-server compiler-facts and editor workflow
- Completed `phase5-guided-assistance` and the Phase 5 gate without adding an
  LLM dependency to source checking, builds, runtime execution, or deployment
- Updated the provider-neutral generation prompt to version 0.2.11 for strict
  authoring protocol 1, review-gated edits, and permission-delta approval
- Completed `phase6-state` for durable local single-host coordination while
  retaining honest provider crash windows and no distributed exactly-once
  claim
- Updated the provider-neutral generation prompt to version 0.2.12 for durable
  state, checkpoints, replay, and schema-3 host configuration
- Completed `phase6-jobs-storage` and the Phase 6 durable-execution gate:
  interrupted worker deliveries resume safely without losing committed state or
  repeating completed external side effects, while distributed queues, brokers,
  cron expressions, and provider-side exactly once remain unclaimed
- Added `K5205` for invalid durable delivery leases, acknowledgements, and
  outcome details
- Updated the provider-neutral generation prompt to version 0.2.13 for durable
  queues, scheduled triggers, bounded object storage, and schema-4 host
  configuration
- Committed terminal queue and schedule transitions when a reservation reaches
  its scan bound, so a depth-one or single-attempt resource dead-letters and
  stays usable instead of wedging
- Required every configured queue and schedule lease to cover the runtime
  execution deadline plus the backing store's busy timeout
- Validated the complete schema-1 object set inside the exclusive migration
  transaction before any DDL and revalidated the finished schema before commit,
  so a rejected migration leaves the database byte-for-byte unchanged
- Installed the database page ceiling before initialization or migration,
  re-verified the materialized page count against the byte budget, and raised
  the minimum configurable database budget to a truthful 1 MiB
- Validated every schema-4 job definition, grant, limit, and store reference
  before any database is created, opened, or migrated
- Decoupled queue publication from the state revision so independent publishers
  never conflict, while combined state outcomes still advance it exactly once
- Charged staged queue depth per queue so an atomic fan-out to several queues
  commits
- Rejected unrepresentable schedule and queue instants before any cursor, fire,
  or job row moves
- Replaced case-insensitive `LIKE` object-prefix matching with exact
  case-sensitive binary matching in which `%` and `_` are ordinary characters
- Kept `krit worker --json` and `krit schedule --json` standard output a single
  machine-readable report by moving bounded guest output into an explicit
  `outputs` array

## [0.2.0] - 2026-09-01

### Added

- Normative language charter, readable syntax, runtime semantics, and
  diagnostic contract
- Draft type/effect, capability, module, and package specifications
- Rust technical design and reproducible performance methodology
- Rust workspace with source mapping, lexer, parser, direct evaluator, CLI,
  and strict manifest validation
- Stable human and JSON Lines diagnostics
- Checked signed 64-bit arithmetic
- Immutable lexical closures, recursion, lists, and exhaustive list matching
- Implementation-neutral conformance fixture format and cases
- Rust-only formatting, linting, testing, release build, and CLI CI

### Changed

- Replaced the active Racket implementation with a Rust-only bootstrap
- Replaced prototype S-expressions with Krit's edition-2026 readable syntax
- Made specifications and conformance cases the semantic authority

### Removed

- Racket runtime, package metadata, tests, installation instructions, and CI

## [0.1.0] - 2026-08-31

### Added

- Racket-based educational interpreter, CLI, tests, examples, and
  documentation

The immutable historical baseline is tagged `racket-v0.1.0`.
