# Capability model

**Status:** Draft  
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
fs_read = ["config/*.json", "data/**"]
net_connect = ["api.example.com:443"]
env_read = ["KRIT_PROFILE"]
secret_read = ["model-api-key"]
ai_invoke = ["openai/*", "local/*"]
```

Paths are package-root-relative and resolved before execution. A lexical path
that escapes the granted root is rejected. Symlink and platform-specific path
rules must be enforced by the host sandbox, not string matching alone.

Network grants include an exact host and port or a documented restricted
pattern. DNS rebinding and redirects cannot expand the original grant.

Secret grants expose opaque handles where possible. Secrets must not appear in
debug output, diagnostics, cache keys, lockfiles, or telemetry.

## Grant authority

The effective grant is the intersection of:

1. capabilities declared by the root package
2. command-line or embedding-host grants
3. operating-system sandbox restrictions

Dependencies contribute required effects but cannot add grants.

`krit permissions` displays requested, granted, denied, and unused
capabilities with the source package that introduced each requirement.

## Development mode

Standalone source execution may receive `io.stdout` only. Every other
capability remains denied unless explicitly passed through an `--allow`
option.

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
must pair capability handles with operating-system mechanisms where available:

- pre-opened file or directory handles
- restricted child process APIs
- network allow-list enforcement
- environment construction rather than inheritance
- Wasm component or process isolation for untrusted workloads

An unavailable host enforcement mechanism fails closed for untrusted mode.

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

## Open decisions

- Final manifest grant syntax
- Host sandbox support matrix
- Resource quota schema
- Capability delegation between isolated components
- Signed organization policies
