# Modules and packages

**Status:** Draft package system; state grants implemented
**Manifest schema:** 1  
**Lockfile schema:** 1

## Goals

Krit packaging must be deterministic, auditable, fast after the first build,
and independent of one registry.

## Project layout

```text
my-project/
  krit.pkg
  krit.lock
  src/
    main.krit
    helpers.krit
  tests/
  benches/
```

`krit.pkg` is authored and reviewed by people. `krit.lock` is generated,
committed for applications, and verified by tooling.

## Manifest

The manifest uses a strict TOML subset. Unknown fields are errors.

```toml
schema = 1

[package]
name = "akshay/hello"
version = "0.1.0"
edition = "2026"
entry = "src/main.krit"
license = "Apache-2.0"
target = "wasm-component"

[dependencies]
"krit/json" = "1.2.3"

[capabilities]
stdout = true
config = ["agent.model", "agent.timeout-ms"]
http = ["https://api.github.com", "https://slack.com"]
secrets = ["github-token", "slack-token"]
ai = ["reviewer"]
logs = true
state = ["agent-work"]
```

Package names contain a lowercase namespace and name separated by `/`.
Allowed characters are ASCII lowercase letters, digits, and `-`. Names are
globally identified by `(registry, namespace, name)`, not name alone.

Versions follow Semantic Versioning 2.0.0. Published package contents are
immutable for a version.

`wasm-component` is the implemented artifact target and the default when
`target` is omitted for schema-1 compatibility. Unknown targets are errors.
The target selects an artifact contract, not additional ambient capabilities.

## Dependency sources

Dependency resolution is deferred until the single-package WebAssembly MVP is
working end to end. Its accepted design is:

Supported source kinds:

- registry name and exact or compatible version requirement
- Git URL plus immutable commit
- local path for development

The lockfile records the resolved source, exact version or commit, package
content hash, and dependency edges. A path dependency cannot be published.

Registry URLs are configured outside source or named explicitly in the
manifest. The default registry is replaceable and mirrorable.

## Lockfile

The lockfile is deterministic and contains no timestamps, machine paths,
credentials, or registry access tokens.

Conceptual entry:

```toml
schema = 1

[[package]]
name = "krit/json"
version = "1.2.3"
source = "registry+https://packages.krit.dev/index"
checksum = "sha256:..."
dependencies = []
```

Entries sort by source, package name, and version. Dependency arrays sort
lexicographically.

`--locked` rejects any operation that would change the lockfile. `--offline`
uses only verified local content.

## Content store

Downloaded package archives are verified and unpacked into an immutable
content-addressed store:

```text
~/.krit/store/sha256/<digest>/
```

Compilers must not modify store entries. Concurrent installations use atomic
temporary directories and rename after checksum verification.

Garbage collection operates from explicit roots: active projects, pinned
packages, and retained build artifacts.

## Build cache

A compiled module cache key includes:

- compiler build identifier
- language edition
- target triple
- optimization profile
- normalized source hash
- imported public-interface hashes
- enabled language features
- capability/effect contract hash

It excludes absolute project paths, wall time, and secret values.

For schema-1 agent contracts, package build planning compares the analyzer's
sorted literal-resource requirements with the manifest before backend
emission. `config.read("agent.model")` requires an exact `config` entry and
`secret.read("github-token")` requires an exact `secrets` entry. Missing
resources are `K5001`. `http.request("https://api.example.com")` likewise
requires one exact normalized `http` entry, and `ai.invoke("reviewer")`
requires one exact sorted `ai` entry. `observe.log` requires `logs = true`.
`state.transaction("agent-work")` requires one exact `state` entry.
`queue.publish("render-jobs")` requires one exact `queues` entry and
`queue.consume("render-jobs")` requires one exact `consumes` entry, so publish
and consume authority are separately reviewable.
`schedule.trigger("hourly-sweep")` requires one exact `schedules` entry.
`object.write("render-output")` requires one exact `buckets` entry, and
`object.read("render-output")` is satisfied by `buckets` or the disjoint
read-only `readOnlyBuckets` list, and `search.query("docs")` requires an
exact `searchIndexes` entry while `search.vector("embeddings")` requires an exact
`vectorIndexes` entry.
`cache.write("lookups")` requires one exact `cacheNamespaces` entry, and
`cache.read("lookups")` is satisfied by `cacheNamespaces` or the disjoint
read-only `readOnlyCacheNamespaces` list. `database.write("catalog")` requires one exact
`databases` entry, and `database.read("catalog")` is satisfied by `databases` or
the disjoint `readOnlyDatabases` list. Replay
operations separately retain their exact HTTP/AI requirement. Manifest state,
queue, schedule, and bucket names are logical authority only; database paths,
durability settings, lease durations, attempt budgets, and byte bounds are
strict host configuration and never package data. Database files, SQL text,
statement catalogs, and schemas are likewise strict host configuration.
Matching resources permit the bounded webhook backend only; unsupported
general composite layouts still fail closed. Host-config adapter origins,
secrets, retry/rate keys, and approval resources must also be manifest-granted
and cannot widen this plan.

Cache hits must be behaviorally identical to clean builds. A corrupt artifact
is rejected by checksum rather than executed.

## Modules

Every source file is a module. The path below `src/` determines its local
module name.

Proposed import and export syntax:

```krit
use helpers::{sum, total};

pub fn answer() -> Int {
    total()
}
```

Wildcard imports are not allowed because they hide authority and name
origins. Imports are static and top-level. Module initialization is pure;
effects begin only through an explicit entry function.

The exact syntax remains draft and is not accepted by the 0.2 parser.

## Publishing

Publication requires:

- valid manifest and lockfile
- clean conformance and package tests
- explicit included-file list
- recognized license expression
- no path dependencies
- no uncommitted generated artifacts
- package content hash confirmation

Registry metadata and package blobs are separate protocols so either can be
mirrored.

## Supply-chain rules

- No install or lifecycle scripts.
- Checksums are mandatory.
- Signatures may add identity but never replace content hashes.
- A dependency cannot widen root capabilities.
- Package archives reject absolute paths, parent traversal, device files, and
  conflicting normalized paths.
- Credentials are host configuration, never manifest or lockfile data.

## CLI contract

Planned commands:

```text
krit new
krit add
krit remove
krit resolve
krit fetch
krit check
krit build
krit run
krit test
krit bench
krit permissions
krit publish
```

All mutating package commands support `--dry-run`. Resolution and build plans
have stable JSON output for agent use.

The bootstrap implements `krit build [--manifest PATH] [--output PATH]`,
`krit sandbox [--manifest PATH] [--artifact PATH]`, and
`krit permissions --artifact PATH [--json] [MANIFEST]` for one local package.
The default build/sandbox artifact is
`target/krit/<package-name>.wasm` beside the manifest, with schema-1 metadata
at `<component>.json`. Entry paths are package-relative and must resolve inside
the package. Missing inferred authority fails before output replacement or
execution. Sandbox never builds or falls back to source evaluation.
Registry, publishing, workspaces, dependency resolution, caching, and the
full-language/agent component runtime remain deferred.

## Open decisions

- Registry index protocol
- Package archive format
- Signature and transparency-log model
- Feature resolution
- Workspace manifests
- Standard library versioning
