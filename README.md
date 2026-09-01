# Krit

Krit is an open, human-auditable programming language for small sandboxed
agents, bots, and integration APIs written with AI and trusted by people.

```krit
fn sum(items) {
    match items {
        [] => 0,
        [head, ..tail] => head + sum(tail),
    }
}

println(sum([10, 20, 12]));
```

```text
42
```

Krit favors readable source, deterministic behavior, immutable values,
explicit authority, and machine-readable compiler facts. Natural language can
help generate Krit, but natural language is never executable Krit.

Krit and the Krit language are owned by Akshay Bhardwaj.

## Status

Krit 0.2 is an early Rust bootstrap implementing the normative dynamic core:

- UTF-8 source with precise byte and human positions
- familiar expressions, blocks, functions, calls, and operators
- immutable lexical bindings and closures
- checked 64-bit integer arithmetic
- booleans, strings, unit, immutable lists, and ordered records
- built-in `Option` and `Result` values with exhaustive matching
- value annotations enforced by `krit check` and deterministic JSON conversion
- lexical name resolution and deterministic static type inference/checking
- sorted `io.stdout` effect inference with function and call propagation
- one explicit typed `webhook fn` source entrypoint with fixed HTTP contract
  aliases and deterministic request/response JSON Schemas
- literal-resource `config.read` and `secret.read` effects plus separate
  sorted capability requirements
- direct normalized-origin `http_request` with exact `http.request` resource
  facts and bearer-only opaque secret consumption
- provider-neutral `ai_invoke` with exact adapter facts and one host-side
  deterministic `http-json` adapter
- ordered typed `log_info`/`log_error` events with bounded buffering,
  key/value redaction, and stderr-only JSON Lines publication
- reusable stateful `AgentHost` policy for bounded retries, per-resource rate
  limits, embedding cancellation, process-local idempotency, and default-deny
  AI/bearer approval
- opaque `Secret` compiler/Core identity with static non-disclosure rules
- name-resolved, inferred typed Core IR with deterministic IDs and explicit
  evaluation order
- verified closures, recursive self bindings, captures, branches, and matches
- stable human and schema-1 JSON compiler explanations
- deterministic validator-accepted WebAssembly Component Model artifacts for
  the initial layout-concrete Core subset
- effect-selected `krit:runtime@0.2.0` pure and stdout WIT worlds
- explicit fail-closed Wasm feature/import policy, schema-1 adjacent metadata,
  and exact-byte BLAKE3 digests
- reusable-engine, fresh-Store Wasmtime component hosting with fuel, epoch
  deadline, stack, StoreLimits, host-call, and buffered-output bounds
- `krit sandbox` execution and artifact-aware effective permission reports
- typed webhook Component Model exports with exact effect-selected config,
  secrets, anonymous/authenticated HTTP, AI, logging, and optional stdout
  imports
- bounded webhook invocation, immutable host configuration, zeroizing
  host-side secret storage, DNS-pinned no-redirect outbound HTTP/TLS, and
  loopback `serve --once`
- deterministic fixture-driven `krit invoke --request FILE` with all outbound
  access still constrained by exact host policy
- recursive function declarations
- exhaustive empty/cons list matching
- deterministic comment-preserving canonical source formatting
- deterministic human and JSON diagnostics
- offline `krit lsp` diagnostics, formatting, hover, completion, symbols, and
  schema-1 compiler/package/permission facts
- optional `krit assist` provider-neutral suggestions with explicit context
  inspection, canonical diff review, compiler checks, permission approval, and
  atomic acceptance
- implementation-neutral conformance cases
- strict package manifest validation

Phase 4 is complete for the bounded stateless reference agent. The checked
reference flow calls a GitHub-like origin, one neutral AI adapter, and one
messaging-like origin with exact permissions and approval requirements.
General composite Wasm layouts beyond the documented webhook subset, modules,
dependency resolution, build caching, broader autonomous editing, and
production multi-tenant OS isolation are also future work. Krit is not
production-ready.

## Requirements

- Rust 1.94 or newer
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
```

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
```

The default output is `target/krit/krit.wasm` for this repository, with
metadata at `target/krit/krit.wasm.json`. Metadata schema 1 includes the exact
`blake3:<hex>` digest and byte size, package-relative entry, WIT world, sorted
effects/imports, exact resource and approval-required facts, and policy
version. The digest covers the final component bytes after bounded embedded
metadata is attached. Pure programs select the
zero-import `pure-program` world; programs with the checked `io.stdout` effect
select `program` and its stdout interface. Unused manifest grants do not widen
artifact imports, and validation derives the world and effects from the actual
component/core import surface before accepting metadata claims.

Artifact policy 1 supports `Int`, `Bool`, `Unit`, recursive and higher-order
non-capturing functions, blocks, conditionals/short circuit, checked integer
operators, primitive comparisons, and scalar `print`/`println`. The bounded
webhook policy-2 path additionally supports strings, the fixed HTTP records,
header lists, Result/Option matching, static non-capturing helper references,
config, opaque secrets, outbound HTTP, neutral AI calls, ordered log fields,
and unescaped JSON-string decoding used to validate the reference model
output. Other composites, general JSON shapes, escaped JSON strings, data
captures, and unresolved parametric layouts fail closed with stable
`K7001`/`K7002` diagnostics or a bounded guest trap. `krit build` never falls
back to direct interpretation. Run only an existing validated artifact:

```sh
krit sandbox
krit sandbox --manifest path/to/krit.pkg --artifact dist/program.wasm
```

`sandbox` never builds or falls back to source execution. It uses a reusable
Wasmtime engine with a fresh Store and instance, no WASI or inherited stdout,
and buffered output released only on success. The exact default and hard
limits plus the serialized epoch-scheduling and pre-deadline compilation
limitations are documented in [the sandbox specification](spec/WASM-SANDBOX.md).
`krit run` remains the full-language direct evaluator for pure/stdout source;
it fails with `K5003` for webhook, configuration, secret, HTTP, AI, and
structured-log host operations rather than fabricating values.

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
`idempotency-key`. AI and HTTP rate state and inbound response idempotency are
bounded process-local memory only: restart and multi-process deployments reset
them, concurrent first use can duplicate work, and Phase 6 will replace
best-effort replay with durable transactional records. Model output remains
nondeterministic raw UTF-8 and must be parsed or validated by source before
structured use.

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
cannot confuse draft agent APIs with compilable Krit 0.2 code.

### Agent contract authoring

Krit can check and explain a minimal webhook agent boundary:

```krit
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match secret("github-token") {
        Ok(token) => match http_request(
            "https://api.example.com",
            request,
            Some(token),
        ) {
            Ok(response) => response,
            Err(error) => record { status: 502, headers: [], body: error },
        },
        Err(error) => record { status: 500, headers: [], body: error },
    }
}
```

`config_string` returns `Result<String, String>`, `secret` returns
`Result<Secret, String>`, and `http_request` returns
`Result<HttpResponse, String>`. `ai_invoke("adapter", input)` returns raw
`Result<String, String>`. `log_info` and `log_error` accept a canonical event
literal plus an ordered `List<LogField>`, where `LogField` is
`Record { name: String, value: String }`. Resource names and normalized HTTP
origins must be direct literals so `krit explain`, metadata, and permission
review can report exact authority. `Secret` cannot be revealed or
structurally stored; only direct `Some(secret)` in the bearer position is
accepted.

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

See [spec/LANGUAGE.md](spec/LANGUAGE.md) for complete normative syntax and
runtime semantics.

## Package baseline

`krit.pkg` is strict TOML:

```toml
schema = 1

[package]
name = "akshay/krit"
version = "0.2.0"
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
- [Krit 0.2 language](spec/LANGUAGE.md)
- [Diagnostic contract](spec/DIAGNOSTICS.md)
- [Webhook agent contracts](spec/WEBHOOK-CONTRACTS.md) — compiler contracts
  and bounded HTTP runtime
- [AI and observability policy](spec/AI-OBSERVABILITY.md) — normative Phase 4
  AI, logs, reliability, idempotency, cancellation, and approval
- [Agent application model](spec/AGENT-APPLICATIONS.md) — draft
- [Types and effects](spec/TYPES-AND-EFFECTS.md) — implemented Phase 4 subset
- [Capabilities](spec/CAPABILITIES.md) — bounded Phase 4 host implemented
- [Modules and packages](spec/PACKAGES.md) — draft
- [WebAssembly sandbox](spec/WASM-SANDBOX.md) — policy-1 artifact and bounded
  host implemented
- [Guided AI authoring](spec/GUIDED-AUTHORING.md) — implemented deterministic
  LSP and review-gated provider-neutral assistance baseline
- [Narrow product MVP](docs/mvp.md)
- [Agent platform roadmap](docs/agent-roadmap.md)
- [Rust technical design](docs/technical-design.md)
- [Performance methodology](docs/performance.md)
- [Initial measured baseline](benchmarks/baseline.json)
- [Policy-1 Wasm host baseline](benchmarks/phase3-wasm-host.json)
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

The conformance suite runs through the Rust tests. See
[CONTRIBUTING.md](CONTRIBUTING.md) for the language-change process.

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
9. durable state and replay only after the stateless reference gate (Phase 6)

Performance claims follow [docs/performance.md](docs/performance.md), not
implementation-language assumptions.

## License

Krit's specifications, implementation, and documentation are licensed under
the [Apache License 2.0](LICENSE). It is permissive and includes an explicit
patent grant. It does not place licensing requirements on programs written in
Krit.
