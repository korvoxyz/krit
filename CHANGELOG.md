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
