# Contributing to Krit

Krit is an open language for AI-authored, human-auditable software. Changes
must preserve explicit semantics, deterministic behavior, and least authority.

Krit and the Krit language are owned by Akshay Bhardwaj.

## Security issues

Do not report a suspected vulnerability in a public issue or pull request.
Follow [SECURITY.md](SECURITY.md) to report it privately through GitHub
private vulnerability reporting.

## Setup

Install Rust 1.95 or newer, then run:

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

Portable language behavior belongs in the conformance suite:
`conformance/cases/` for evaluation, `conformance/check/` for static checking
(including build-target diagnostics), `conformance/format/` for canonical
formatting, and `conformance/package/` for manifest validation. Every case has
Krit source and its expected status, output, and diagnostic and warning codes;
[conformance/README.md](conformance/README.md) describes each harness. Keep
each case small and independent of host state.

Implementation regressions that do not define language behavior belong in
crate unit or integration tests.

## Documentation examples

Every `krit` code block in `README.md` and `spec/*.md` must be a complete
program that `krit check` accepts without diagnostics, and a `krit` block
followed directly by a `text` block must print exactly that text under
`krit run`. `crates/krit-cli/tests/doc_examples.rs` enforces both, so run it
after editing an example:

```sh
cargo test -p krit-cli --test doc_examples
```

Write signatures and fragments as `text` blocks, and format examples with
`krit fmt` before pasting them, so the documentation shows canonical source.

## Developer notes

Builds and tests are disk-hungry: debug test binaries for the whole workspace
take several gigabytes, and every git worktree has its own `target/`.
`scripts/clean.sh` reclaims that space. It is a dry run by default: it prints
what it would remove and how much space that frees, and it deletes only when
given `--yes`. Everything it removes is regenerable.

```sh
scripts/clean.sh                    # report debug, intermediates, dist, scratch
scripts/clean.sh --yes              # delete them, keeping target/release/krit
scripts/clean.sh --yes examples     # remove local example state and data
scripts/clean.sh --help             # list every category
```

The categories are `debug` (`target/debug`), `intermediates` (release build
intermediates, keeping the `krit` binary), `release`, `dist` (packaged
archives), `scratch` (other entries at the top of `target/`), `examples`
(`examples/target`, `examples/state`, and `examples/data`), `worktrees` (the
`target/` of every other git worktree of this repository), and `all`. Do not
clean `worktrees` while builds are running in them.

## Release workflow

CI runs the minimum-version formatting/Clippy/conformance gate and current-stable
tests on Linux, macOS, and Windows before building native Linux x86-64/ARM64,
macOS Intel/Apple Silicon, and Windows x86-64 packages. The Windows test job is
advisory (`continue-on-error`) until its suite is first green; then it becomes
required like the others. Downloadable archives and per-archive SHA-256 files
are attached to successful workflow runs for 14 days.

The release packager uses Python 3.12's standard library. It includes dependency
license notices and exercises the extracted archive's evaluator, compiler,
permission report, and Wasm host rather than relying only on the build-tree
binary. Missing notices fail packaging. Wasmtime-family crates that omit their
shared license may reuse the Wasmtime project license only when their declared
license and exact source revision match.

To publish a stable release:

1. Update the workspace version in `Cargo.toml` and move the completed changelog
   entries into a dated release section. Commit and push to `main`.
2. Wait for CI, including every native package, to succeed.
3. Create and push an annotated `vMAJOR.MINOR.PATCH` tag on that exact commit.
   The tag must match the CLI's Cargo version. For this release, use `v0.2.0`,
   not `v2.0.0`.
4. The Release workflow repeats the quality/package gate, builds and exercises
   non-root containers from the packaged Linux binaries, publishes both native
   images and a multi-architecture GHCR index, then attaches the native archives
   and `SHA256SUMS` to a public GitHub Release.
5. On the first GHCR publication, set the package's visibility to **Public** in
   GitHub Package settings. Linking a public repository does not do this
   automatically. The workflow reports the visibility and warns if this step
   remains outstanding.

Publication uses the workflow's `GITHUB_TOKEN`: read-only for quality/build
jobs, `packages: write` for container publication, and `contents: write` only
for the release publisher. No publishing credential is supplied to pull
requests. Actions are pinned to reviewed commit hashes.

Published release tags are immutable by convention: never move or overwrite
one. A failed unpublished release may be retried from its tag with the Release
workflow's manual dispatch, or by rerunning the failed jobs. Existing draft
assets can be replaced during recovery; published release assets cannot.

## License

By contributing, you agree that your contributions are licensed under the
Apache License 2.0.
