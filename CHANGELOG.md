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
