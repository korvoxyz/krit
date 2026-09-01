# Krit agent MVP

**Status:** Accepted scope
**Purpose:** Prove the product without building the entire platform

## Product test

Krit must let a person and an AI create a small webhook agent, understand its
behavior, inspect every permission, and run it in a bounded WebAssembly
sandbox.

The reference application:

1. accepts one typed HTTP webhook
2. validates JSON input
3. reads one approved opaque secret
4. makes bounded outbound HTTP calls to GitHub and one AI adapter
5. posts a typed result to one messaging adapter
6. returns typed JSON
7. handles failures through visible `Result` values
8. declares exact network, secret, and model permissions
9. compiles to a WebAssembly component
10. produces deterministic behavior and permission explanations

Application source should read as policy and business behavior, not HTTP,
async-runtime, TLS, SDK, or memory-management infrastructure.

## Required language work

- static checking for the implemented records, `Option<T>`, and `Result<T, E>`
- typed public entry-point boundaries
- schema-directed JSON conversion above the implemented dynamic conversion
- static effect inference
- explicit async host operations without exposed executor machinery

## Required runtime work

- one WebAssembly component artifact backend (implemented for policy-1 scalar
  Core)
- one Rust component host (implemented for policy 1; full language and agent
  interfaces remain)
- typed HTTP request/response interface
- outbound HTTP allow-list
- opaque secret handles
- one provider-neutral AI interface
- memory, fuel, deadline, stack, host-call, and output limits (implemented for
  policy 1; request/response/log/model limits follow their future interfaces)
- fresh or provably reset invocation state (fresh Store/instance implemented)
- structured redacted logs

## Required authoring work

- canonical formatter (implemented in the 0.2 bootstrap)
- parser, name, type, effect, and capability diagnostics
- `krit check`
- `krit explain` (typed Core/type/effect facts implemented; agent interfaces
  remain future work)
- `krit permissions` (requested and artifact-effective reports implemented)
- `krit sandbox` (policy-1 execution implemented)
- language-server compiler facts
- one optional inline prediction provider
- accepted suggestions formatted and checked immediately
- semantic cleanup shown as a diff and explicitly approved

The MVP does not train or host its own model.

## Deferred

- package registry and publishing
- arbitrary Rust extension authoring
- databases and durable transactions
- queues and schedules
- many provider connectors
- autonomous whole-project rewriting
- custom bytecode VM
- native compiler backend
- browser deployment
- distributed agent coordination
- graphical workflow editor

These features require evidence from the reference agent rather than being
prerequisites for it.

The post-MVP service order is transactional state, queues and schedules,
object storage, database access, then cache and search. See
[agent-roadmap.md](agent-roadmap.md).

## Acceptance criteria

- A new user can identify the webhook input, external calls, failures, and
  permissions from source and `krit explain`.
- The capability report contains no undeclared or ambient authority.
- The component cannot access files, processes, environment variables,
  arbitrary network targets, or secret values outside its grants.
- Invalid JSON and model output fail schema validation.
- Resource exhaustion terminates the invocation without damaging the host.
- AI assistance can be disabled without changing source, build, or runtime
  behavior.
- Suggested edits cannot bypass formatting, checks, tests, or permission
  approval.
- The reference Krit application is materially smaller and faster to review
  than an equivalent generated Rust service.
- Startup, memory, and request overhead are measured before optimization
  claims are made.
