# Capability model

**Status:** Implemented bounded Phase 6 local host
**Target:** Krit 0.4

## Rule

Krit code has no ambient authority.

Files, network, processes, environment variables, clocks, randomness, secrets,
and AI providers are unavailable unless the host grants a capability. A
package can request authority but cannot grant or widen it.

## Capability names

Capability identifiers are hierarchical:

```text
io.stdout
io.stdin
config.read
http.request
fs.read
fs.write
net.connect
process.spawn
env.read
clock.read
random.read
secret.read
ai.invoke
state.transaction
queue.publish
queue.consume
schedule.trigger
object.read
object.write
database.read
database.write
```

Unknown capability identifiers are errors. Names are versioned through the
language edition and package manifest schema.

## Grants

Grants are narrow data:

```toml
[capabilities]
stdout = true
config = ["agent.model", "agent.timeout-ms"]
http = ["https://api.github.com", "https://slack.com"]
secrets = ["github-token", "slack-token"]
ai = ["reviewer"]
logs = true
state = ["agent-work"]
queues = ["render-jobs"]
consumes = ["render-jobs"]
schedules = ["hourly-sweep"]
buckets = ["render-output"]
readOnlyBuckets = ["reference"]
databases = ["catalog"]
readOnlyDatabases = ["reference-data"]
```

The schema-1 manifest implements requests for `stdout`, `config`, `http`,
`secrets`, `ai`, structured `logs`, durable `state`, durable queue publish
(`queues`) and consume (`consumes`), scheduled triggers (`schedules`), and
object buckets (`buckets` for read and write, `readOnlyBuckets` for read only;
the two lists are disjoint). The language emits literal-resource facts for
`config.read`, `secret.read`, `http.request`, `ai.invoke`, `state.transaction`,
`queue.publish`, `object.read`, and `object.write`; `queue.consume` and
`schedule.trigger` come from the `queue` and `schedule` entrypoint
declarations, and `observe.log` is resource-free. Files, generic sockets,
processes, environment variables, clocks, and randomness remain unavailable.
Application databases add `databases` (read and write) and `readOnlyDatabases`
(read only); the two lists are disjoint and yield `database.read` and
`database.write` requirements per named database.
Durable state, queues, schedules, and buckets are available only through exact
named resources that a host configuration binds to owner-only stores; see
[JOBS-AND-STORAGE.md](JOBS-AND-STORAGE.md). Application databases are
operator-owned files with a host-owned statement catalog; see
[DATABASE.md](DATABASE.md).

Paths are package-root-relative and resolved before execution. A lexical path
that escapes the granted root is rejected. Symlink and platform-specific path
rules must be enforced by the host sandbox, not string matching alone.

Network grants include an exact host and port or a documented restricted
pattern. DNS rebinding and redirects cannot expand the original grant.

Secret grants expose opaque handles where possible. Secrets must not appear in
debug output, diagnostics, cache keys, lockfiles, or telemetry.

## Bounded host operations

The edition-2026 source contracts are:

```krit
config_string("agent.model") // Result<String, String>
secret("github-token")       // Result<Secret, String>
http_request(
    "https://api.github.com",
    request,
    Some(token),
)                            // Result<HttpResponse, String>
ai_invoke("reviewer", input) // Result<String, String>
log_info("review.started", fields) // Result<Unit, String>
state_get("agent-work", key) // Result<Option<String>, String>
checkpoint_put("agent-work", "posted-message", value) // Result<Unit, String>
replay_ai("agent-work", "summarize", "reviewer", input) // Result<String, String>
```

The string must be a direct literal so analysis can emit one exact sorted
requirement pair. `config_string` emits `config.read` plus its key.
`secret` emits `secret.read` plus its logical name. `http_request` emits
`http.request` plus one normalized exact origin. Its bearer argument is
directly `None` or `Some(secret)`; this is the only approved structural use of
the opaque handle. The host injects the Authorization header without exposing
secret bytes. `ai_invoke` emits `ai.invoke` plus one named adapter.
`log_info` and `log_error` emit `observe.log`; their event names are direct
canonical literals and their ordered fields contain only ordinary strings.
No host value is read during checking or explanation.

Phase 6 adds exact `state.transaction("store")` requirements for bounded
state, checkpoint, and replay operations. Replay also retains the exact
external HTTP-origin or AI-adapter requirement. Store/checkpoint/replay names
are direct canonical literals. Database paths and durability policy are
host-owned schema-3 configuration and never source or manifest data. The
normative contract is [DURABLE-STATE.md](DURABLE-STATE.md).

Package build orchestration intersects these requirements with manifest
resources and reports `K5001` for an absent exact match. A matching supported
webhook compiles to an effect-selected typed component; unsupported general
composites and captures still fail closed with `K7001`/`K7002`.

## Grant authority

The effective grant is the intersection of:

1. capabilities declared by the root package
2. command-line or embedding-host grants
3. operating-system sandbox restrictions

Dependencies contribute required effects but cannot add grants.

`krit permissions` without an artifact displays the unchanged requested plan.
`krit permissions --artifact PATH` validates the artifact and displays
requested, required, effective, denied, exact imports, and local denial
reasons. It separately displays approval-required AI adapters and bearer HTTP
origins without treating approval as a grant. A denied report is printed
completely and exits 4. Deployment and approval policy evaluation remain
`not-evaluated` in this inspection command.

The request-only `krit permissions --json` output remains:

```json
{"schema":1,"package":"akshay/agent","requested":[{"capability":"http.request","resource":"https://api.github.com"},{"capability":"secret.read","resource":"github-token"}],"grantStatus":"not-evaluated"}
```

Requests sort by capability and resource. Configuration keys, HTTP origins,
and secret names are identifiers only; their values are never included.

Artifact-aware JSON uses schema 1 and includes `world`, `required`,
`effective`, `denied`, `imports`, `approvalRequired`, `approvalStatus`,
`denialReasons`, `localGrantStatus`, and `deploymentGrantStatus`.

## Development mode

Standalone source execution may receive `io.stdout` only. A source containing
a webhook entrypoint or configuration/secret/HTTP host call fails with
`K5003`; there is no fallback value or `--allow` escape hatch. `krit invoke`
and `krit serve` load only an existing validated artifact and explicit host
inputs.

Package execution reads grants from the root manifest and may still require
interactive or host approval. CI should use a non-interactive explicit policy.

## Installation

Package installation and dependency resolution never execute package code.
Krit has no lifecycle scripts such as pre-install, post-install, or build
scripts.

Build-time generators, if introduced, run as separately declared capability-
restricted programs whose outputs are content-addressed.

## Enforcement

Language-level checks are not a security boundary by themselves. The runtime
instantiates a validated WebAssembly component with only the typed imports
required by its resolved capability plan. It must pair capability handles with
operating-system mechanisms where available:

- pre-opened file or directory handles
- restricted child process APIs
- network allow-list enforcement
- environment construction rather than inheritance
- WebAssembly memory and control-flow isolation
- fuel, memory, stack, host-call, output, and deadline limits
- process or container isolation for untrusted multi-tenant workloads

An unavailable host enforcement mechanism fails closed for untrusted mode.
The complete boundary is defined in `WASM-SANDBOX.md`.

## Revocation and accounting

Embedding hosts may revoke handles between operations. Phase 4 exposes an
atomic cancellation handle checked before instantiation, on every host call
and backoff, and during active libcurl transfers. Current limits include
bytes, requests, AI/HTTP calls, logs, fuel, stack, memory, and wall time;
provider token accounting remains future work.

Quota exhaustion is distinct from permission denial and receives a separate
diagnostic.

## AI providers

AI calls are optional library/runtime capabilities, not core syntax or a
build requirement. Provider adapters implement a versioned neutral interface
containing:

- model identifier
- bounded UTF-8 input
- bounded raw UTF-8 output
- timeout and resource limits
- explicit nondeterminism declaration

Schema-directed structured messages and output are future adapter revisions.
The current source must explicitly validate or parse raw output.

Prompts and responses are private capability data. They are never uploaded for
telemetry by default.

The exact implemented Phase 4 contracts, privacy boundary, and reliability
policy are normative in `AI-OBSERVABILITY.md`.

## Service delivery order

Capabilities are added in this order:

1. configuration, secrets, HTTP, observability, and reliability controls
2. transactional state, queues, schedules, and object storage
3. database, cache, and search connectors

Configuration is typed immutable startup data, not inherited environment.
State provides durable correctness and replay before a general database is
exposed. Caches are optional optimizations whose absence cannot change
correctness.

`docs/agent-roadmap.md` defines the implementation gates.

## Open decisions

- Final manifest grant syntax
- Host sandbox support matrix
- Resource quota schema
- Capability delegation between isolated components
- Signed organization policies
