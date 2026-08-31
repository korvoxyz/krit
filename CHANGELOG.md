# Changelog

All notable Krit changes are documented here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Krit is pre-1.0 and does not yet promise stable syntax between minor releases.

## [Unreleased]

### Added

- Draft agent application, WebAssembly sandbox, and guided AI authoring
  specifications
- Narrow reference-agent MVP and explicit deferred scope
- Phased agent service roadmap from authority planning through durable state,
  queues, databases, and optional caches
- Strict configuration, secret, and outbound HTTP capability declarations
- Human and JSON `krit permissions` output for pre-deployment authority review
- Versioned provider-neutral Krit 0.2 generation prompt and usage contract

### Changed

- Selected WebAssembly components instead of custom bytecode as the first
  deployment target
- Defined LLM assistance as optional visible edits gated by deterministic
  formatting, checks, and permission analysis

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
