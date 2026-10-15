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

## Before you push

Run `scripts/gate.sh` before pushing or opening a pull request. It runs what
CI runs, with the same commands and flags: formatting, the example metrics
checks, Clippy with `-D warnings`, the workspace tests and conformance, the
documentation tests, `cargo deny`, and `cargo audit`. It uses Rust 1.95.0,
CI's quality toolchain, and `--locked`, so a newer local compiler cannot hide
a lint that CI's toolchain reports. Install the toolchain and CI's versions
of the supply-chain tools once:

```sh
rustup toolchain install 1.95.0 --profile minimal --component clippy,rustfmt
cargo install --locked --version 0.20.2 cargo-deny
cargo install --locked --version 0.22.2 cargo-audit
```

Then run the gate. It always runs at the repository root, wherever it is
started from:

```sh
scripts/gate.sh                     # what CI runs, on Rust 1.95.0
scripts/gate.sh --skip-supply       # offline: skip cargo deny and cargo audit
scripts/gate.sh --toolchain stable  # the current-stable test toolchain
scripts/gate.sh --help              # every option
```

Every step runs even after one fails. The output names the toolchain, OS, and
commit it ran on, gives a result per step, and ends with a `gate: PASS` or
`gate: FAIL` line. The script exits non-zero on any failure, on a missing
toolchain, and on a cargo-deny or cargo-audit version other than CI's. A run
with `--skip-supply` is partial and says so in its final line. Builds go to
`target/gate-<toolchain>` (change it with `--target-dir`), so the gate does
not disturb your everyday build cache. The gate does not build the release
profile, which CI's package jobs do; a change to build settings or to the
release profile should also pass
`cargo +1.95.0 build -p krit-cli --release --locked`.

`deny.toml` holds the supply-chain policy: RustSec advisories, including
unmaintained, unsound, and yanked crates, are errors; version requirements
may not be wildcards; the license allow-list names exactly the licenses the
dependency graph needs; and crates come only from crates.io. A new
dependency whose license is not on the list needs a reviewed change to
`deny.toml`, and an advisory may be ignored only with a stated reason and an
expiry release.

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
archives), `scratch` (other entries at the top of `target/`, including the
`target/gate-<toolchain>` build caches of `scripts/gate.sh`), `examples`
(`examples/target`, `examples/state`, and `examples/data`), `worktrees` (the
`target/` of every other git worktree of this repository), and `all`. Do not
clean `worktrees` while builds are running in them.

## Release workflow

CI runs on pull requests and on pushes to `main` and to `release/**`
branches. It runs the minimum-version formatting/Clippy/conformance gate, the
supply-chain job (`cargo deny` and `cargo audit` at pinned versions), and
current-stable tests on Linux, macOS, and Windows before building native Linux
x86-64/ARM64, macOS Intel/Apple Silicon, and Windows x86-64 packages.
`scripts/gate.sh` runs the same quality and supply-chain steps locally. The
Windows test job is advisory (`continue-on-error`) until its suite is first
green; then it becomes required like the others. Downloadable archives and
per-archive SHA-256 files are attached to successful workflow runs for 14 days.

The release packager uses Python 3.12's standard library. It includes dependency
license notices and exercises the extracted archive's evaluator, compiler,
permission report, and Wasm host rather than relying only on the build-tree
binary. Missing notices fail packaging. Wasmtime-family crates that omit their
shared license may reuse the Wasmtime project license only when their declared
license and exact source revision match.

To publish a stable release:

1. Update the workspace version in `Cargo.toml` and move the completed changelog
   entries into a dated release section. On a minor or major release, also
   raise the `version` requirement of the workspace's own crates under
   `[workspace.dependencies]`. Commit and push to `main`.
2. Wait for CI, including every native package, to succeed.
3. Create and push an annotated `vMAJOR.MINOR.PATCH` tag on that exact commit.
   The tag must match the CLI's Cargo version. For this release, use `v0.3.3`,
   not `v3.1.0`.
4. The Release workflow repeats the quality/package gate, builds and exercises
   non-root containers from the packaged Linux binaries, and publishes both
   native images and a multi-architecture GHCR index under the version tag. It
   attests the index (pushing the attestation to the registry), then the
   native archives and `SHA256SUMS`, and checks each attestation with
   `gh attestation verify`. It uploads the archives and `SHA256SUMS` to a draft
   GitHub Release, compares the draft's assets byte for byte with the attested
   files, and publishes the release as Latest. Only then does it move the
   series (for example `0.3`) and `latest` image tags to the attested index.
5. On the first GHCR publication, set the package's visibility to **Public** in
   GitHub Package settings. Linking a public repository does not do this
   automatically. The workflow reports the visibility and warns if this step
   remains outstanding.

Publication uses the workflow's `GITHUB_TOKEN`: read-only for quality/build
jobs, `packages: write` for container publication and the final tag move, and
`contents: write` only for the release publisher. Only the two jobs that
attest, the container index job and the release publisher, also get
`id-token: write` and `attestations: write`. No publishing credential is
supplied to pull requests. Actions are pinned to reviewed commit hashes.

Published releases and their tags are immutable: never move or overwrite one,
and ship a correction as a new patch release. The workflow refuses to touch a
release that is already published, and it uploads assets only to a draft that
the same workflow run created, so rerunning the failed jobs of a run resumes
that run's own draft. Any other draft for the tag stops the run; a maintainer
must review and delete it before retrying with the Release workflow's manual
dispatch. If the release was published but moving the image tags failed,
rerun only the failed jobs; a full rerun stops at the release identity check.

## License

By contributing, you agree that your contributions are licensed under the
Apache License 2.0.
