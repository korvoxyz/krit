# Contributing to Krit

Krit is an open language for AI-authored, human-auditable software. Changes
must preserve explicit semantics, deterministic behavior, and least authority.

Krit and the Krit language are owned by Akshay Bhardwaj.

## Setup

Install Rust 1.94 or newer, then run:

```sh
cargo build --workspace --locked
cargo test --workspace --all-targets --locked
```

Before submitting a change:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets --locked
cargo build --workspace --release --locked
```

## Language changes

The documents in `spec/` are authoritative. A syntax or semantics change must
include:

1. Motivation and rejected alternatives.
2. Exact specification changes.
3. Human-readability and AI-auditability impact.
4. Determinism, type/effect, and capability impact.
5. Implementation-neutral conformance cases.
6. Parser, runtime, diagnostic, and CLI tests as applicable.
7. Compatibility and performance impact.
8. A changelog entry.

Do not preserve behavior merely because the archived Racket prototype had it.

## Implementation rules

- Stable Rust and the workspace minimum version only.
- No `unsafe` without a separately reviewed runtime invariant design.
- Unknown versions, fields, capabilities, and artifact features fail closed.
- No package install scripts.
- Diagnostics use stable codes and source spans.
- User output is deterministic and excludes host paths, addresses, and secret
  values.
- Dependencies should provide clear value relative to build time and
  supply-chain surface.

## Conformance cases

Portable language behavior belongs in `conformance/cases/`. Every case has
Krit source, expected status, optional exact standard output, and optional
diagnostic codes. Keep each case small and independent of host state.

Implementation regressions that do not define language behavior belong in
crate unit or integration tests.

## License

By contributing, you agree that your contributions are licensed under the
Apache License 2.0.
