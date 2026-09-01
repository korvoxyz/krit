# Capability model

**Status:** Implemented contracts with draft hosts
**Target:** Krit 0.3

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
```

The schema-1 manifest implements requests for `stdout`, `config`, `http`, and
`secrets`. The language now emits literal-resource facts for `config.read`
and `secret.read`, but no configuration/secret value provider or outbound
HTTP host exists yet. Files, generic sockets, processes, environment
variables, clocks, randomness, state, queues, storage, and AI invocation
remain unavailable.

Paths are package-root-relative and resolved before execution. A lexical path
that escapes the granted root is rejected. Symlink and platform-specific path
rules must be enforced by the host sandbox, not string matching alone.

Network grants include an exact host and port or a documented restricted
pattern. DNS rebinding and redirects cannot expand the original grant.

Secret grants expose opaque handles where possible. Secrets must not appear in
debug output, diagnostics, cache keys, lockfiles, or telemetry.

## Contract-only host operations

The edition-2026 source contracts are:

```krit
config_string("agent.model") // Result<String, String>
secret("github-token")       // Result<Secret, String>
```

The string must be a direct literal so analysis can emit one exact sorted
requirement pair. `config_string` emits `config.read` plus its key.
`secret` emits `secret.read` plus its logical name. The opaque `Secret` handle
cannot be printed, compared, JSON-encoded, structurally stored, or revealed.
No host value is read during checking or explanation.

Package build orchestration intersects these requirements with manifest
resources and reports `K5001` for an absent match. A matching manifest does
not make a component buildable in this contracts milestone: unsupported
webhook/config/secret backend layouts still fail closed with `K7002`.

## Grant authority

The effective grant is the intersection of:

1. capabilities declared by the root package
2. command-line or embedding-host grants
3. operating-system sandbox restrictions

Dependencies contribute required effects but cannot add grants.

`krit permissions` without an artifact displays the unchanged requested plan.
`krit permissions --artifact PATH` validates the artifact and displays
requested, required, effective, denied, exact imports, and local denial
reasons. A denied report is printed completely and exits 4. Deployment policy
is not implemented and remains `not-evaluated`.

The request-only `krit permissions --json` output remains:

```json
{"schema":1,"package":"akshay/agent","requested":[{"capability":"http.request","resource":"https://api.github.com"},{"capability":"secret.read","resource":"github-token"}],"grantStatus":"not-evaluated"}
```

Requests sort by capability and resource. Configuration keys, HTTP origins,
and secret names are identifiers only; their values are never included.

Artifact-aware JSON uses schema 1 and includes `world`, `required`,
`effective`, `denied`, `imports`, `denialReasons`, `localGrantStatus`, and
`deploymentGrantStatus`.

## Development mode

Standalone source execution may receive `io.stdout` only. A source containing
a webhook entrypoint or configuration/secret host call fails with `K5003`;
there is no fallback value and no `--allow` escape hatch in this milestone.

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

Embedding hosts may revoke handles between operations. Long-running operations
receive cancellation. Future limits may include bytes, requests, tokens,
process count, CPU time, wall time, and memory.

Quota exhaustion is distinct from permission denial and receives a separate
diagnostic.

## AI providers

AI calls are library/runtime capabilities, not core syntax. Provider adapters
implement a versioned neutral interface containing:

- model identifier
- structured input
- structured output schema
- timeout and resource limits
- explicit nondeterminism declaration

Prompts and responses are private capability data. They are never uploaded for
telemetry by default.

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
