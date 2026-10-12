# Krit

Krit is an open, human-auditable programming language for small sandboxed
agents, bots, and integration APIs written with AI and trusted by people. A
Krit program compiles to a WebAssembly component that can reach only the
origins, secrets, models, and stores its manifest grants, and every one of
them is written as a literal in the source.

```krit
/// Summarizes one GitHub issue with a model and returns the summary.
fn review(request: HttpRequest) -> Result<HttpResponse, String> {
    let repository = config_string("reference.repository")?;
    let token = secret("github-token")?;
    let issue = http_request(
        "https://github.example.invalid",
        HttpRequest {
            method: "GET",
            path: "/repos/example/issues/1",
            headers: ["x-reference-repository": repository],
        },
        Some(token),
    )?;
    // Model output is raw, nondeterministic, untrusted text.
    let summary = ai_invoke("reviewer", issue.body)?;
    log_info("review.completed", ["delivery": request.path])?;
    Ok(HttpResponse { status: 200, body: summary })
}

webhook fn handle(request: HttpRequest) -> HttpResponse {
    match review(request) {
        Ok(response) => response,
        Err(reason) => {
            log_error("review.failed", ["reason": reason]);
            HttpResponse { status: 502, body: "review failed" }
        },
    }
}
```

Every fallible step is visible: `?` passes a failure up to `handle`, which
names both outcomes. `krit check` proves that this file builds into a
component. `krit explain` lists the exact capabilities it needs,
`ai.invoke("reviewer")`, `config.read("reference.repository")`,
`http.request("https://github.example.invalid")`, and
`secret.read("github-token")`, plus the `observe.log` effect. `krit build`
refuses a package whose manifest does not grant them, and the host refuses to
send `github-token` to any origin its manifest binding does not list.

Krit favors readable source, deterministic behavior, immutable values,
explicit authority, and machine-readable compiler facts. Natural language can
help generate Krit, but natural language is never executable Krit.

Krit and the Krit language are owned by Akshay Bhardwaj.

## Status

Krit 0.3 ("Honest and flat") is an early release for people who let AI write
small bots and integrations and must approve them. It makes Krit **honest**
(what checks is what ships, failures are diagnostics, authority is bound) and
**flat** (the happy path reads top to bottom). What 0.3 makes possible:

- **What checks is what ships.** For a file with a webhook, queue, or
  schedule entrypoint, `krit check` runs the complete WebAssembly build
  pipeline with the output discarded and reports every `K7001`/`K7002` that
  `krit build` would, at its source span, so a clean check means the file
  builds. `krit run` refuses every program `krit check` refuses. Deep
  nesting and runaway recursion are diagnostics (`K1005`, `K4010`), never a
  crash.
- **Strings deploy.** Routing on `request.method == "POST"`, composing text
  with `+`, interpolation (`"\(name)"`), and `"""` text blocks build into
  components, and so does the first standard library: `int_to_string`,
  `url_path_segment`, `url_query`, and five string predicates.
- **Flat code.** Postfix `?`, let-else, nested patterns with `_` and
  exhaustiveness checking, named construction with defaults
  (`HttpResponse { status: 200 }`), entry lists (`["accept": "text/plain"]`),
  the unit value `()`, and `///` doc comments. All are additive in edition
  2026, defined by their meaning in the core language, and deployable where
  the underlying types are.
- **Bound authority.** A manifest binds each secret to the origins it may
  reach (`[secrets.github-token] origins = ["https://api.github.com"]`), and
  the host refuses to send it anywhere else (`K5006`) before a byte leaves.
  `krit permissions` labels a legacy unbound secret
  `UNBOUND (any granted origin)`. Declared dependencies fail closed (`K6003`),
  and request paths that a server could normalize into another resource are
  refused; `url_path_segment` encodes untrusted text so it fails as a value
  instead.
- **Repairable.** Constructs that models import from other languages
  (`return`, `for`, assignment, `class`, `import`, `await`, `throw`,
  indexing, arrow functions, `?:`, `switch`, `else { return … }`) each report
  their own code (`K1101`-`K1112`) with a one-line fix; undefined names get a
  did-you-mean; warnings (`K2003`, `K9013`) are reported without failing.
- **Upgradeable.** The 0.3 runtime loads artifacts built by earlier
  toolchains inside a published compatibility window (0.2.0 artifacts are
  tested); the host runs Wasmtime 48 LTS, and the minimum Rust version is
  1.95.

In 0.2, every fallible host call cost a two-arm `match`, and the reference
agent nested 15 levels deep. The same step from that agent, before:

```krit
fn fetch_issue() -> Result<HttpResponse, String> {
    match config_string("reference.repository") {
        Ok(repository) => match secret("github-token") {
            Ok(token) => http_request(
                "https://github.example.invalid",
                record {
                    method: "GET",
                    path: "/repos/example/issues/1",
                    query: "",
                    headers: [record { name: "x-reference-repository", value: repository }],
                    body: "",
                },
                Some(token),
            ),
            Err(error) => Err(error),
        },
        Err(error) => Err(error),
    }
}
```

and after, in 0.3:

```krit
fn fetch_issue() -> Result<HttpResponse, String> {
    let repository = config_string("reference.repository")?;
    let token = secret("github-token")?;
    http_request(
        "https://github.example.invalid",
        HttpRequest {
            method: "GET",
            path: "/repos/example/issues/1",
            headers: ["x-reference-repository": repository],
        },
        Some(token),
    )
}
```

Both mean the same thing; the old spelling still compiles.

Krit is **not production-ready**. Known limits in 0.3:

- No user-defined `type` or `enum`, generics, modules, list functions, or
  schema-directed `json_parse` yet: model output is decoded with
  `json_decode` and validated by hand. The compiler reports one error at a
  time. Multi-error reporting is planned for 0.4, and the language features
  follow it ([master plan](docs/roadmap.md)).
- Components support a bounded subset of the checked language: user records
  and lists, capturing closures, and general JSON shapes fail closed with
  `K7001`/`K7002` instead of building
  ([deployable subset](spec/WASM-SANDBOX.md#implemented-backend-support)).
- The checker does not yet prove which secret reaches which origin; the host
  enforces bindings at run time.
- Durable state, queues, schedules, objects, the database connector, and the
  cache are local and single-host; no distributed exactly-once behavior is
  claimed. Multi-tenant deployments need OS isolation that Krit does not
  provide.
- On Windows, host mode refuses secrets and durable state, and
  guided-assistance acceptance is unavailable.

What else is in the box:

- **Language:** immutable lexical bindings and closures, checked 64-bit
  integers, strings, lists, ordered records, built-in `Option` and `Result`,
  exhaustive matching, deterministic JSON conversion, and static type, effect,
  and literal-resource inference with no annotations required inside
  functions.
- **Entrypoints and host operations:** a typed `webhook fn` with fixed HTTP
  contracts and JSON Schemas; `queue` and `schedule` handlers; configuration,
  opaque secrets, exact-origin HTTP, a provider-neutral AI call, structured
  redacted logs, transactional state, checkpoints, HTTP/AI replay, durable
  queues and scheduled triggers, object buckets, catalogued database
  transactions, an optional TTL cache, and search connectors.
- **Artifacts and host:** deterministic Component Model artifacts with
  least-authority WIT worlds, adjacent metadata, and BLAKE3 digests; a
  Wasmtime host with fuel, memory, stack, deadline, host-call, and output
  bounds, a fresh store per invocation, DNS-pinned no-redirect HTTP/TLS, and
  default-deny approval for AI and bearer calls.
- **Tooling:** `krit check`, `run`, `build`, `explain`, `fmt`, `permissions`,
  `sandbox`, `invoke`, `serve`, `worker`, and `schedule`; human and JSON Lines
  diagnostics; an offline language server; optional review-gated AI
  suggestions (`krit assist`); a provider-neutral generation prompt
  (`krit prompt`); and implementation-neutral conformance cases.

## Install a prebuilt release

Download Krit from [GitHub Releases](https://github.com/korvoxyz/krit/releases/latest).
The archives are self-contained: **no Rust, Python, or Node.js installation is
required**. Each includes the CLI, runnable examples, `BUILD.json` with the source
commit and binary digest, and dependency license notices.

| Platform | Archive for release 0.3.0 |
| --- | --- |
| Linux x86-64, glibc 2.35+ | `krit-0.3.0-x86_64-unknown-linux-gnu.tar.gz` |
| Linux ARM64, glibc 2.35+ | `krit-0.3.0-aarch64-unknown-linux-gnu.tar.gz` |
| macOS 12+, Intel | `krit-0.3.0-x86_64-apple-darwin.tar.gz` |
| macOS 12+, Apple Silicon | `krit-0.3.0-aarch64-apple-darwin.tar.gz` |
| Windows x86-64 | `krit-0.3.0-x86_64-pc-windows-msvc.zip` |

Download `SHA256SUMS` alongside your archive. On Linux, verify the downloaded
archive with `sha256sum --check --ignore-missing SHA256SUMS`. On macOS, select
its checksum entry, for example:

```sh
grep '  krit-0.3.0-aarch64-apple-darwin.tar.gz$' SHA256SUMS | shasum -a 256 -c -
tar -xzf krit-0.3.0-aarch64-apple-darwin.tar.gz
cd krit-0.3.0-aarch64-apple-darwin
./krit --version
./krit run examples/factorial.krit
```

On Windows, compare `Get-FileHash <archive> -Algorithm SHA256` with its entry
in `SHA256SUMS`, extract the ZIP, and run `.\krit.exe --version`. Add the
extracted directory to `PATH` if desired.

The macOS and Windows downloads are not developer-signed or notarized. Use
your OS's per-application approval flow only after checking the download's
source and checksum; do not disable system-wide protections. Guided-assistance
acceptance still fails closed on platforms without the audited atomic-exchange
primitive, including Windows. On Windows, host mode (`invoke`, `worker`,
`schedule`, `serve`) also refuses host configurations that grant secrets or
durable state, because owner-only file checks exist only on Unix; see
[Platform support](spec/WASM-SANDBOX.md#platform-support).

### GitHub Packages

Krit is also published to
[GitHub's Container registry](https://github.com/korvoxyz/krit/pkgs/container/krit),
with native Linux AMD64 and ARM64 images:

```sh
docker run --rm ghcr.io/korvoxyz/krit:0.3.0 --version
docker run --rm ghcr.io/korvoxyz/krit:0.3.0 run examples/factorial.krit
```

The image runs as a non-root user. To use a local project on Linux or macOS,
mount it explicitly and use your own user identity so generated files remain
yours:

```sh
docker run --rm --user "$(id -u):$(id -g)" \
  --volume "$PWD:/work" ghcr.io/korvoxyz/krit:0.3.0 check main.krit
```

The `0.3` tag tracks the 0.3 series; `latest` tracks the latest stable release.
Pin a full version or image digest for reproducible use. GitHub Packages hosts
the container; standalone native archives are GitHub Release assets.

GitHub creates a new container package with private visibility even when its
repository is public. A package administrator must make it **Public** once in
**Package settings > Change visibility** to enable anonymous pulls. Subsequent
versions retain that setting.

### Verifying a release

From 0.3.1 on, the Release workflow attests every archive, `SHA256SUMS`, and
the multi-architecture container image with signed SLSA build provenance.
Check that a download or image was built from this repository by that
workflow with the [GitHub CLI](https://cli.github.com/manual/gh_attestation_verify):

```sh
gh attestation verify krit-0.3.1-aarch64-apple-darwin.tar.gz --repo korvoxyz/krit
gh attestation verify oci://ghcr.io/korvoxyz/krit:0.3.1 --repo korvoxyz/krit
```

Add `--signer-workflow korvoxyz/krit/.github/workflows/release.yml` to also
require the Release workflow as the signer. Published releases are never
modified; a correction ships as a new patch release. Report a download or
image that fails verification as described in [SECURITY.md](SECURITY.md).

## Requirements

These requirements apply only when building from source:

- Rust 1.95 or newer
- Cargo

Install Rust through [rustup](https://rustup.rs/) if it is not already
available.

## Build

```sh
cargo build --release --locked
./target/release/krit --version
```

Install the CLI from this checkout:

```sh
cargo install --path crates/krit-cli --locked
krit --version
```

## Use

Run a source file:

```sh
krit run examples/factorial.krit
krit run examples/lists.krit
```

Check syntax, lexical names, types, matches, and inferred effects without
executing. A successful check also lowers and verifies typed Core IR:

```sh
krit check examples/factorial.krit
krit check --diagnostic-format json examples/webhook-agent.krit
krit check --target component examples/lists.krit
```

`--target component|module|none` also runs the complete WebAssembly build
pipeline with the output discarded and reports every `K7001`/`K7002` that
`krit build` would, each at its source span and in source order.
`component` checks the artifact `krit build` emits; `module` checks the file
as a module program; `none` skips the target check. A file that declares a
webhook, queue, or schedule entrypoint is checked as `component` by default,
because a component is the only way it can run, so a clean `krit check` of an
entrypoint file means it builds. Other files default to `none`. Target checks
read no manifest; `krit build` still enforces package grants (`K5001`).

Inspect stable compiler facts and the resolved Core program:

```sh
krit explain examples/factorial.krit
krit explain --json examples/factorial.krit
```

The explanation shows the synthetic `module-init` entrypoint, any source
webhook contract, inferred effects and literal-resource capability
requirements, top-level binding/function types, and deterministic typed Core
IR. Webhook JSON includes exact draft-2020-12 request and response schemas.
Core executable references use numeric IDs; source names appear only as debug
metadata. Explanation JSON schema 1 is serialized with `serde_json` and does
not include absolute compiler or cache paths.

Core names are resolved and its types are normalized inference results, but
not every Core type is a concrete storage layout. Constrained parametric type
variables may remain in otherwise valid generic Core. The WebAssembly artifact
stage must specialize such variables, or report a stable source diagnostic,
before choosing layouts or emitting code. Open structural record requirements
likewise describe required fields rather than a final closed Wasm record
layout.

Format one or more files after validating the complete batch:

```sh
krit fmt examples/factorial.krit examples/lists.krit
krit fmt --check examples/factorial.krit examples/lists.krit
```

`krit fmt` preserves every `//` comment, emits four-space indentation and LF
line endings, and leaves all requested files untouched if any file cannot be
read or parsed. `--check` writes nothing and returns status `1` when a file is
not canonical.

Validate a package manifest:

```sh
krit package check
```

Inspect every capability requested by the package:

```sh
krit permissions
krit permissions --json
```

Without `--artifact`, this remains the phase-1 requested-authority report.
Artifact-aware inspection validates the adjacent metadata and component, then
compares its exact effects/imports with the local manifest:

```sh
krit permissions --artifact target/krit/krit.wasm
krit permissions --artifact target/krit/krit.wasm --json
```

Deployment grants remain explicitly `not-evaluated`. Artifact-aware reports
also list approval-required AI adapters and bearer HTTP origins separately;
they do not claim that deployment approval has been evaluated.

Build the package's validated WebAssembly component:

```sh
krit build
krit build --manifest path/to/krit.pkg --output dist/program.wasm
krit build --diagnostic-format json
```

`--diagnostic-format human|json` selects the format of every build
diagnostic, from manifest errors to backend failures; a failed backend check
reports all of its findings at once.

The default output is `target/krit/krit.wasm` for this repository, with
metadata at `target/krit/krit.wasm.json`. Metadata schema 1 includes the exact
`blake3:<hex>` digest and byte size, package-relative entry, WIT world, sorted
effects/imports, exact resource and approval-required facts, and policy
version. The digest covers the final component bytes after bounded embedded
metadata is attached. Pure programs select the
zero-import `pure-program` world; programs with the checked `io.stdout` effect
select `program` and its stdout interface. Unused manifest grants do not widen
artifact imports, and validation derives the world and effects from the actual
component/core import surface before accepting metadata claims. A runtime
loads artifacts from its own and the two previous minor toolchains, never
newer ones, when their metadata schema, policy, `krit:runtime` WIT version,
and edition are supported; see
[artifact compatibility](spec/WASM-SANDBOX.md#artifact-compatibility).

Artifact policy 1 supports `Int`, `Bool`, `Unit`, recursive and higher-order
non-capturing functions, blocks, conditionals/short circuit, checked integer
operators, primitive comparisons, and scalar `print`/`println`. The bounded
entrypoint policy-2 path additionally supports strings with `+`, `==`, `!=`,
ordering, interpolation, and text blocks; the fixed HTTP records, written as
records or by named construction; header and log-field lists, entry lists
included; Result/Option matching with nested patterns; postfix `?` and
let-else; the standard library; static non-capturing helper references; every
host operation; and `json_decode` into `String` (every JSON escape) used to
validate model output. User records and lists, general JSON shapes, data
captures, and unresolved parametric layouts fail closed with stable
`K7001`/`K7002` diagnostics or a bounded guest trap; helpers the entrypoint
never reaches are eliminated first, so an unused generic helper does not fail
the build. `krit build` never falls back to direct interpretation. Run only an
existing validated artifact:

```sh
krit sandbox
krit sandbox --manifest path/to/krit.pkg --artifact dist/program.wasm
```

`sandbox` never builds or falls back to source execution. It uses a reusable
Wasmtime engine with a fresh Store and instance, no WASI or inherited stdout,
and buffered output released only on success. The exact default and hard
limits plus the serialized epoch-scheduling and pre-deadline compilation
limitations are documented in [the sandbox specification](spec/WASM-SANDBOX.md).
`krit run` checks source exactly as `krit check` does (without a target
check, since it builds no artifact), refuses any program that fails with
check's diagnostic and status, and then evaluates pure/stdout
source directly; it rejects webhook, queue, and schedule entrypoints and
host-only operations with `K5003` rather than fabricating values.

Invoke a webhook deterministically from an exact JSON fixture:

```sh
krit build --manifest examples/webhook-agent.krit.pkg
krit invoke \
  --manifest examples/webhook-agent.krit.pkg \
  --host-config examples/webhook-agent.host.json \
  --request examples/webhook-agent.request.json
```

The checked-in host file contains placeholder policy and no secret values, so
the default invocation fails safely as an application response before making
network calls. The successful all-local three-service gate is the
`reference_webhook_runs_github_ai_and_messaging_with_exact_audit_facts`
integration test in `crates/krit-runtime/tests/phase4.rs`.

Serve an already-built artifact on loopback, once for tests or without
`--once` for a local process:

```sh
krit serve --manifest examples/webhook-agent.krit.pkg --bind 127.0.0.1:3000 --once
```

Neither command builds or falls back to source interpretation. Host config
schema 1 remains accepted for immutable strings and secret **file
references**, but sensitive bearer calls are now default-denied. Migrate those
hosts to schema 2 and add exact `http.bearer` approval entries. Schema 2 adds
strict AI adapter, retry, rate, byte-bounded idempotency, and noninteractive
approval policy:

```json
{
  "schema": 2,
  "config": {"reference.repository": "example/repository"},
  "secrets": {"ai-token": {"file": "secrets/ai-token"}},
  "aiAdapters": {
    "reviewer": {
      "kind": "http-json",
      "origin": "https://ai.example.invalid",
      "path": "/v1/invoke",
      "model": "deployment-model",
      "secret": "ai-token",
      "maxInputBytes": 65536,
      "maxResponseBytes": 65536,
      "timeoutMs": 750
    }
  },
  "approvals": [{"operation": "ai.invoke", "resource": "reviewer"}]
}
```

On Unix, secret files must grant no group/other permissions (for example,
`chmod 600 secret.bin`). Host inputs cannot add names or origins absent from
the package manifest. Unknown fields and values above hard count, byte,
attempt, timeout, window, or TTL bounds fail closed.

`invoke` writes only the exact response JSON to stdout. Structured application
logs are compact JSON Lines on stderr after completion. `serve` never puts
logs in HTTP bodies. Validated redacted logs from a failed invocation may be
published with `"outcome":"failure"`; partial response and normal output
remain rolled back.

Retries never re-execute guest code. They apply only to connection/timeouts or
429/502/503/504 for GET/HEAD or a request with a valid ordinary
`idempotency-key`. AI and HTTP rate state remain process-local. Inbound response
idempotency remains process-local unless schema 3 explicitly selects a
manifest-granted durable store. Model output remains nondeterministic raw UTF-8
and must be parsed or validated by source before structured use.

### Durable local state and replay

Schema-1 manifests request logical stores:

```toml
[capabilities]
state = ["agent-work"]
```

Edition 2026 provides bounded string operations. This webhook remembers the
first body it sees:

```krit
/// Stores the first body this webhook sees and answers with it afterwards.
fn remember(body: String) -> Result<String, String> {
    match checkpoint_get("agent-work", "processed-request")? {
        Some(previous) => Ok(previous),
        None => {
            state_put("agent-work", "last-body", body)?;
            checkpoint_put("agent-work", "processed-request", body)?;
            Ok(body)
        },
    }
}

webhook fn handle(request: HttpRequest) -> HttpResponse {
    match remember(request.body) {
        Ok(body) => HttpResponse { status: 200, body },
        Err(error) => HttpResponse { status: 500, body: error },
    }
}
```

`state_get` reads a key and `state_delete` removes one. State/checkpoint mutations are invocation-local
and commit only after valid successful guest completion; traps, cancellation,
deadline, response failure, or revision conflict roll them back.

External work can use stable completed-operation replay:

```krit
fn summarize() -> Result<String, String> {
    let issue = replay_http(
        "agent-work",
        "fetch-issue",
        "https://api.example.com",
        HttpRequest { method: "GET", path: "/issues/1" },
    )?;
    replay_ai("agent-work", "summarize-issue", "reviewer", issue.body)
}
```

`replay_http` is anonymous and requires GET/HEAD or one valid ordinary
`Idempotency-Key`. Completed bounded results survive process restart and are
reused after current grants and AI approval are rechecked. Krit cannot close
the crash window between a remote effect and its local replay-record commit;
provider idempotency remains necessary and distributed exactly once is not
claimed.

Host config schema 3 owns database paths and limits. No path is chosen by
source or enabled by default. The complete schema is shown in
[the durable-state specification](spec/DURABLE-STATE.md). On Unix the
containing state directory must already be owner-only and database/WAL/SHM
files are owner-only regular files without symlinks.

The checked-in
[`examples/stateful-webhook.krit`](examples/stateful-webhook.krit) uses
transactional state and a named checkpoint. Create an owner-only
`examples/state` directory before using its schema-3 host config.

### Durable queues, scheduled triggers, and object storage

Publish, consume, trigger, and bucket authority are separate manifest grants:

```toml
[capabilities]
queues = ["render-jobs"]        # queue.publish
consumes = ["render-jobs"]      # queue.consume
schedules = ["hourly-sweep"]    # schedule.trigger
buckets = ["render-output"]     # object.read and object.write
```

An ingress webhook enqueues without any consume or state authority:

```krit
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match queue_publish("render-jobs", request.body) {
        Ok(id) => HttpResponse { status: 202, body: id },
        Err(error) => HttpResponse { status: 500, body: error },
    }
}
```

A worker declares the queue it consumes and returns a typed outcome:

```krit
queue "render-jobs" fn handle(job: QueueJob) -> Result<String, String> {
    object_put("render-output", job.id, job.body)?;
    Ok(job.id)
}
```

`Ok(detail)` acknowledges the delivery and commits staged state, checkpoints,
object writes, and queue publishes in the same transaction. `Err(detail)`
commits nothing and lets the host retry with capped backoff until the attempt
budget is exhausted, after which the job moves to a bounded dead-letter table.

Scheduled triggers are host-owned. Occurrences are `start + k * interval` UTC
epoch instants, a fire identity is `(schedule, dueAtMillis)`, and the guest only
receives typed facts:

```krit
schedule "hourly-sweep" fn handle(event: ScheduleEvent) -> Result<String, String> {
    Ok(event.id)
}
```

Dispatch is explicit and bounded — there is no daemon and no polling loop:

```sh
krit worker --queue render-jobs --manifest examples/jobs-worker.krit.pkg   --host-config examples/jobs-worker.host.json --once --json
krit schedule --schedule hourly-sweep --manifest examples/jobs-schedule.krit.pkg   --host-config examples/jobs-schedule.host.json --once --now 7200000 --json
```

With `--json` both commands emit exactly one schema-1 report on standard output
— never mixed with guest bytes — carrying parallel `outcomes` and `outputs`
arrays plus dispatch counts.

Host config schema 4 binds manifest-granted queues, schedules, and buckets to
already-configured owner-only stores and can only narrow them. Every job
definition, grant, limit, and store reference is validated before any database
is created, opened, or migrated, and a delivery lease must cover one complete
guest execution. The complete
contract, state machine, limits, and crash model are normative in
[the jobs and storage specification](spec/JOBS-AND-STORAGE.md). The checked-in
[`examples/jobs-webhook.krit`](examples/jobs-webhook.krit),
[`examples/jobs-worker.krit`](examples/jobs-worker.krit), and
[`examples/jobs-schedule.krit`](examples/jobs-schedule.krit) form the reference
enqueue -> worker -> object flow; create an owner-only `examples/state`
directory before running them.

### Capability-scoped database access

A package requests databases by logical name; the host owns the file, the mode,
and every statement:

```toml
[capabilities]
databases = ["catalog"]              # database.read and database.write
readOnlyDatabases = ["reference"]    # database.read only
```

```krit
fn failure(status: Int, error: String) -> HttpResponse {
    HttpResponse { status, body: error }
}

webhook fn handle(request: HttpRequest) -> HttpResponse {
    let Ok(transaction) = db_begin_write("catalog") else Err(error) => failure(503, error);
    let Ok(_) = db_execute(transaction, "record-visit", [request.path]) else
        Err(error) => match db_rollback(transaction) {
            Ok(_) => failure(500, error),
            Err(fatal) => failure(500, fatal),
        };
    let Ok(rows) = db_query(transaction, "count-visits", []) else
        Err(error) => match db_rollback(transaction) {
            Ok(_) => failure(500, error),
            Err(fatal) => failure(500, fatal),
        };
    let Ok(_) = db_commit(transaction) else Err(error) => failure(500, error);
    HttpResponse { status: 200, body: rows }
}
```

`DatabaseTransaction` is opaque exactly like `Secret`: it cannot be printed,
compared, encoded, logged, or stored, and it may appear only as the first
argument of a database operation. Statement text lives in schema-5 host config,
so parameters are always bound and an injection payload stays ordinary data. A
transaction must be explicitly completed; an invocation that ends with one open
rolls it back and fails closed, and no external HTTP or AI call is allowed while
one is open. Query results are bounded deterministic JSON text.

The complete contract is normative in
[the database specification](spec/DATABASE.md).
[`examples/database-webhook.krit`](examples/database-webhook.krit) runs against
an owner-only database the operator provisions from
[`examples/database-webhook.schema.sql`](examples/database-webhook.schema.sql);
Krit never creates, migrates, or resets an application schema.

```sh
mkdir -p examples/data && chmod 700 examples/data
sqlite3 examples/data/catalog.db < examples/database-webhook.schema.sql
chmod 600 examples/data/catalog.db
krit build --manifest examples/database-webhook.krit.pkg
krit invoke \
  --manifest examples/database-webhook.krit.pkg \
  --host-config examples/database-webhook.host.json \
  --request examples/webhook-agent.request.json
```

Krit does not claim atomicity between an application database and its own
durable state: they are separate SQLite files, `db_commit` publishes
immediately, and the two-resource window is documented rather than hidden. Every
invocation exit rolls back any transaction the guest left open, SQLite work is
interrupted at a bounded deadline, and write-ahead logging is refused because
its on-disk size cannot be bounded.

### Cache and search

Krit adds a bounded namespaced TTL cache and provider-neutral search and vector
connectors as **optional** capabilities. The rule is absolute: correctness never
depends on cache availability. `cache_get` returns
`Result<Option<String>, String>`, so a hit, a miss, and an outage are three
distinct values source must handle; the host never substitutes a value and never
turns an outage into a miss.

```sh
krit build --manifest examples/cached-search.krit.pkg
krit serve \
  --manifest examples/cached-search.krit.pkg \
  --host-config examples/cached-search.host.json \
  --bind 127.0.0.1:3000
```

[`examples/cached-search.krit`](examples/cached-search.krit) reads the cache,
falls back to a named connector on a miss *or an outage*, stores the result with
an explicit time to live, and answers identically either way. The same artifact
runs unchanged with the cache configured or absent.

The cache is process local and explicitly non-transactional: it is shared across
fresh Wasm stores on one host, lost on restart, and a trap or failed delivery
does not undo an earlier write. That is stated plainly because it is exactly why
a cached value may never be load bearing.

Connectors are host-owned named bindings. Guest code never sees an endpoint,
credential, provider identity, or raw handle, and results are re-encoded by the
host into a fixed bounded shape, so a provider cannot control the structure the
guest parses. Results are data and are never executed. The complete contract is
normative in [the cache and search specification](spec/CACHE-AND-SEARCH.md).

Krit does not lose or silently duplicate committed queue, schedule, or object
state on one host. It does not provide distributed queues, brokers, consumer
groups, cron expressions, guest-visible listing, or provider-side exactly
once. It does not provide a distributed cache, cache invalidation protocols,
branded provider SDKs, embedding generation, or index management.

Request JSON Lines diagnostics for tools and AI agents:

```sh
krit run --diagnostic-format json broken.krit
```

```json
{"schema":1,"severity":"error","code":"K2001","message":"undefined name `total`","file":"broken.krit","span":{"start":{"line":1,"column":9,"byte":8},"end":{"line":1,"column":14,"byte":13}},"labels":[],"notes":[]}
```

Show all commands:

```sh
krit --help
```

### Editor integration

Start the offline language server over stdio:

```sh
krit lsp
```

The server publishes stable parser/type/effect diagnostics and supports
canonical whole-document formatting, a deterministic format code action,
UTF-16-correct hover, parser/type/field/symbol/built-in completion, manifest
resource completion, and top-level document symbols. It uses only compiler and
local package facts: source is never executed, components are never built or
run, packages are never installed, and no runtime, provider, secret, or
network authority is available.

Editors and deterministic authoring tools can request
`krit/compilerFacts` with a standard `textDocument` identifier. Schema 1
returns stable byte/LSP spans, inferred and declared types, resolved names,
effects, literal-resource requirements, entrypoints, package metadata,
requested/required permissions and grant status, reference status, and
canonical formatting edits. Protocol frames are limited to 16 MiB, open
documents to 1 MiB, the open set to 128 documents, and applicable manifest
reads to 256 KiB. Recursive type rendering and response collections are also
bounded, and package facts require the normal canonical entry-containment
check. Standard output contains LSP frames only; operational failures use
standard error.

### Review-gated authoring assistance

Assistance is optional and disabled without an explicit provider config. The
implemented provider-neutral adapter sends strict authoring-protocol JSON to
an HTTPS or loopback HTTP endpoint; it uses no branded SDK. An optional
`credentialEnv` value names a host-managed bearer credential. Credential
values never enter requests, source, proposals, compiler facts, artifacts, or
logs.

Inspect the exact redacted request without contacting the provider:

```sh
krit assist inspect \
  --provider-config assist-provider.json \
  --manifest krit.pkg \
  --file src/main.krit \
  --range all \
  --intent "Add explicit error handling."
```

Generate a proposal artifact without writing source:

```sh
krit assist suggest \
  --provider-config assist-provider.json \
  --manifest krit.pkg \
  --file src/main.krit \
  --range all \
  --intent "Add explicit error handling." \
  --proposal target/assist-proposal.json
```

Revalidate and review its exact canonical diff, diagnostics, type/effect facts,
and requested/required/granted permission delta:

```sh
krit assist review --manifest krit.pkg --proposal target/assist-proposal.json
```

Acceptance is a separate explicit command. It prints the same review before
writing, requires `--reviewed`, requires exact approval for every newly
required permission, and cannot add a manifest grant:

```sh
krit assist accept \
  --manifest krit.pkg \
  --proposal target/assist-proposal.json \
  --reviewed \
  --approve-permission config.read=agent.model
```

Only the package entry source may be edited. Model context must be explicitly
selected, remain under the canonical package root, use `.krit` files, and pass
`.kritignore`. Generated/non-Krit files, host/runtime data, capability literal
values, secret-like strings, and credentials are excluded or redacted.
Provider edits are untrusted: stale, overlapping, cross-document,
out-of-selection, malformed, oversized, non-canonical, ill-typed, ungranted,
or unapproved changes fail closed. Completion, diagnostic repair
(`--kind repair`), and semantic cleanup (`--kind cleanup`) use the same visible
proposal pipeline. Stale-detecting atomic acceptance is implemented on macOS
and Linux; other platforms fail acceptance with `K8107` before changing
source. `--json` emits stable JSON Lines events.

Generate Krit with Claude, ChatGPT, Gemini, or a local model using the exact
provider-neutral instruction shipped with this compiler:

```sh
krit prompt
```

See [ai/README.md](ai/README.md) for the generation and diagnostic-repair
workflow. Prompt material contains only currently implemented syntax so models
cannot confuse draft agent APIs with compilable Krit 0.3 code.

### Agent contract authoring

Krit can check and explain a minimal webhook agent boundary:

```krit
webhook fn handle(request: HttpRequest) -> HttpResponse {
    let Ok(token) = secret("github-token") else
        Err(error) => HttpResponse { status: 500, body: error };
    match http_request("https://api.example.com", request, Some(token)) {
        Ok(response) => response,
        Err(error) => HttpResponse { status: 502, body: error },
    }
}
```

`config_string` returns `Result<String, String>`, `secret` returns
`Result<Secret, String>`, and `http_request` returns
`Result<HttpResponse, String>`. `ai_invoke("adapter", input)` returns raw
`Result<String, String>`. `log_info` and `log_error` accept a canonical event
literal plus an ordered `List<LogField>`, where `LogField` is
`Record { name: String, value: String }`, usually written as an entry list
such as `["delivery": request.path]`. Resource names and normalized HTTP
origins must be direct literals so `krit explain`, metadata, and permission
review can report exact authority. `Secret` cannot be revealed or
structurally stored; only direct `Some(secret)` in the bearer position is
accepted, and the host injects it only into an origin its manifest binding
lists (`K5006` otherwise). A webhook returns `HttpResponse`, so it binds each
fallible step with let-else, whose arm is the response for the failure; a
helper that returns `Result` passes failures on with `?`.

## Language tour

### Values and checked operators

```krit
let answer = 20 + 22;
let greeting = "Hello, " + "Krit!";
let ready = answer == 42 && true;

println(answer);
println(greeting);
println(ready);
```

Integers are signed 64-bit values. Overflow, division by zero, wrong value
kinds, unresolved names, and incorrect function arguments are errors rather
than silent conversions.

### Lexical functions

```krit
let offset = 40;
let add_offset = fn(value) {
    value + offset
};

println(add_offset(2));
```

Bindings are immutable and functions use lexical scope.

### Recursion

```krit
fn factorial(number) {
    if number == 0 {
        1
    } else {
        number * factorial(number - 1)
    }
}

println(factorial(6));
```

### Lists and exhaustive matching

```krit
fn length(items) {
    match items {
        [] => 0,
        [head, ..tail] => 1 + length(tail),
    }
}

println(length(["human", "and", "AI"]));
```

The two list shapes are visible and mandatory. Pattern names exist only in the
non-empty branch.

### Strings and the standard library

```krit
let name = "Krit";
let greeting = "Hello, \(name)!";

println(greeting);
println(string_starts_with(greeting, "Hello"));
println(int_to_string(40 + 2) + " is the answer");
```

```text
Hello, Krit!
true
42 is the answer
```

`\(name)` inserts a `String` value; an `Int` is converted explicitly with
`int_to_string`, since nothing converts implicitly.

### Early exits

```krit
fn parse_port(text: String) -> Result<Int, String> {
    if text == "443" {
        Ok(443)
    } else if text == "80" {
        Ok(80)
    } else {
        Err("unknown port \(text)")
    }
}

fn origin(port: String) -> Result<String, String> {
    let number = parse_port(port)?;
    Ok("https://example.com:" + int_to_string(number))
}

fn describe(port: String) -> String {
    let Ok(text) = origin(port) else Err(reason) => "invalid: \(reason)";
    text
}

println(describe("443"));
println(describe("8080"));
```

```text
https://example.com:443
invalid: unknown port 8080
```

`?` returns a failure from the enclosing function unchanged; let-else binds
the success and names what the function returns otherwise. Both exits are
visible on the line where they happen.

### Nested patterns

```krit
fn label(value: Result<Option<String>, String>) -> String {
    match value {
        Ok(Some(text)) => "found \(text)",
        Ok(None) => "missing",
        Err(error) => "failed: \(error)",
    }
}

println(label(Ok(Some("cache"))));
println(label(Ok(None)));
println(label(Err("timeout")));
```

```text
found cache
missing
failed: timeout
```

Every outcome is named exactly once. A missing arm is a compile error that
lists it (``non-exhaustive match; missing: `Err(_)` ``), and a catch-all
`_ =>` arm is rejected.

See [spec/LANGUAGE.md](spec/LANGUAGE.md) for complete normative syntax and
runtime semantics.

## Package baseline

`krit.pkg` is strict TOML:

```toml
schema = 1

[package]
name = "akshay/krit"
version = "0.3.0"
edition = "2026"
entry = "examples/factorial.krit"
license = "Apache-2.0"
target = "wasm-component"

[capabilities]
stdout = true
```

Unknown fields, malformed names, unsupported editions, invalid versions, and
unsafe entry paths fail closed. Dependency resolution and lockfile generation
will follow [spec/PACKAGES.md](spec/PACKAGES.md).

## Architecture and specifications

The specification is the semantic authority:

- [Language charter](spec/CHARTER.md)
- [Krit 0.3 language](spec/LANGUAGE.md) — grammar, sugar and its meaning,
  standard library, and runtime semantics
- [Diagnostic contract](spec/DIAGNOSTICS.md) — every code with its severity
- [Webhook agent contracts](spec/WEBHOOK-CONTRACTS.md) — compiler contracts
  and bounded HTTP runtime
- [AI and observability policy](spec/AI-OBSERVABILITY.md) — AI, logs,
  reliability, idempotency, cancellation, and approval
- [Agent application model](spec/AGENT-APPLICATIONS.md) — draft
- [Types and effects](spec/TYPES-AND-EFFECTS.md) — static types, exits,
  exhaustiveness, effects, and lints
- [Capabilities](spec/CAPABILITIES.md) — grants, secret-to-origin binding,
  and fail-closed dependencies
- [Modules and packages](spec/PACKAGES.md) — single-package manifests
  implemented; modules and dependencies draft
- [WebAssembly sandbox](spec/WASM-SANDBOX.md) — deployable subset, artifact
  compatibility window, and the bounded host
- [Guided AI authoring](spec/GUIDED-AUTHORING.md) — implemented deterministic
  LSP and review-gated provider-neutral assistance baseline
- [Durable state and replay](spec/DURABLE-STATE.md) — transactional local
  stores, checkpoints, replay, and durable idempotency
- [Jobs and object storage](spec/JOBS-AND-STORAGE.md) — durable queues,
  host-owned scheduled triggers, and bounded object buckets
- [Database access](spec/DATABASE.md) — catalogued parameterized transactions
  against an operator-owned database
- [Cache and search](spec/CACHE-AND-SEARCH.md) — bounded optional cache and
  provider-neutral connectors
- [Narrow product MVP](docs/mvp.md)
- [Agent platform roadmap](docs/agent-roadmap.md) — delivered phases
- [Krit master plan](docs/roadmap.md) — what comes next
- [0.3 requirements](docs/prd-0.3.md) and [changelog](CHANGELOG.md) — what
  0.3 delivers
- [Rust technical design](docs/technical-design.md)
- [Performance methodology](docs/performance.md)
- [Initial measured baseline](benchmarks/baseline.json)
- [Policy-1 Wasm host baseline](benchmarks/phase3-wasm-host.json)
- [Grounded use-case sketches](benchmarks/sketches/README.md) — the programs
  behind the 0.3 fit measurement
- [Conformance suite](conformance/README.md)

The Racket prototype is preserved only in Git history at tag
`racket-v0.1.0`. It is not an active implementation, runtime dependency,
semantic reference, CI requirement, or contributor tool.

## Develop

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked
cargo build --workspace --release --locked
```

The conformance suite runs through the Rust tests, and so does a check of
every Krit example in this README and in `spec/`. See
[CONTRIBUTING.md](CONTRIBUTING.md) for the language-change process and
developer notes such as reclaiming disk space with `scripts/clean.sh`.

## Direction

The accepted implementation path is:

1. Rust source, parser, diagnostics, and direct evaluator
2. readable records, built-in `Option` and `Result`, parsed annotations, and
   dynamic JSON conversion
3. name resolution, static type/effect checking, and canonical formatting
   (complete)
4. typed verified Core IR and deterministic explanations (complete)
5. Core layout diagnostics, validated WebAssembly component artifacts, and one
   bounded host (complete for policy 1)
6. stateless webhook/config/secret/HTTP/AI/log host and reliability policy
   (complete)
7. offline language-server compiler, package, permission, completion, and
   formatting facts (complete)
8. optional provider-neutral inline prediction with visible checked edits
   and separately approved semantic cleanup (complete)
9. durable transactional state, checkpoints, replay, and idempotency
   (complete for local single-host coordination)
10. typed durable queues, host-owned scheduled triggers, and bounded object
    storage (complete for local single-host coordination)
11. capability-scoped catalogued parameterized database transactions (complete
    for the local SQLite connector)
12. cache and search services (complete for bounded optional local cache and
    provider-neutral connectors)
13. 0.3 "Honest and flat": one honest check across `check`, `run`, and
    `build`; deployable strings; secret-to-origin binding; flat code with `?`,
    let-else, nested patterns, named construction, entry lists,
    interpolation, and text blocks; standard library v0; habit diagnostics;
    and the artifact compatibility window

Performance claims follow [docs/performance.md](docs/performance.md), not
implementation-language assumptions.

What comes next is planned in the [master plan](docs/roadmap.md): 0.4 "One
semantics" brings a single Core interpreter, multi-error diagnostics,
`krit test`, durable protocol 1.1, and compiler-proven secret pairs, followed
by user-defined `type` and `enum`, `json_parse`, and the first deployable
repository bot. Completed implementation phases do not make Krit
production-ready or remove the documented backend, package-system, and
OS-isolation limitations.

## Security

Report suspected vulnerabilities privately through GitHub private
vulnerability reporting (**Report a vulnerability** under the repository's
**Security** tab), never in a public issue. [SECURITY.md](SECURITY.md) lists
the supported versions, scope, response targets, and advisory process.

## License

Krit's specifications, implementation, and documentation are licensed under
the [Apache License 2.0](LICENSE). It is permissive and includes an explicit
patent grant. It does not place licensing requirements on programs written in
Krit.
