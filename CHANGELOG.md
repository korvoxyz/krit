# Changelog

All notable Krit changes will be documented here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and the project intends to use [Semantic Versioning](https://semver.org/) from
version 1.0 onward.

## [Unreleased]

## [0.1.0] - 2026-08-31

### Added

- Source-located S-expression parser for `.krit` programs
- Integers, booleans, strings, lexical functions, and immutable lists
- Named recursion, top-level definitions, and simultaneous `let` bindings
- Exhaustive empty/cons list pattern matching
- Arithmetic, comparison, boolean, string, and output operations
- File runner, command-line evaluation, interactive REPL, and version command
- Free-variable analysis for all AST forms
- Racket package metadata and generated `krit` launcher
- RackUnit tests, examples, architecture notes, and language reference
- Apache License 2.0

### Changed

- Replaced the unfinished MUPL course-exercise implementation with Krit's own
  documented language surface and interpreter structure
