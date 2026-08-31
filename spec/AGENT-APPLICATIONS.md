# Agent application model

**Status:** Draft  
**Target:** Krit 0.4

## Product scope

Krit applications are small, typed, event-driven backend components that
people can understand and AI systems can generate reliably.

The initial domain is:

- HTTP APIs
- webhooks
- scheduled jobs
- queue and event consumers
- tool endpoints used by other agents
- third-party API integrations
- structured AI-provider calls
- deterministic business rules and data transformation

Krit does not initially target operating-system services, device drivers,
graphics, hard real-time systems, or high-performance numerical computing.
Rust remains the implementation and extension language for those needs.

## Application boundary

An application exports one or more typed entry points:

```text
HTTP route       request -> response
Webhook          verified event -> acknowledgement
Schedule         scheduled event -> unit
Queue consumer   message -> acknowledgement or retry
Agent tool       structured input -> structured output
```

Entry points are declarations, not top-level side effects. Loading a component
must not open sockets, read secrets, start tasks, or invoke models.

The host owns listeners, TLS, connection pooling, request limits, graceful
shutdown, and sandbox lifecycle. Krit code owns application behavior and
policy.

## Typed boundaries

Every exported entry point has a schema-derived request and response type.
Invalid external input is rejected before application logic runs.

The first application type system should include:

- records with named fields
- variants with exhaustive matching
- `Option<T>`
- `Result<T, E>`
- lists and maps
- validated JSON conversion
- opaque secret and capability handles

Public API schemas must be exportable as stable JSON Schema and component
interface definitions. Dynamic untyped JSON is available only through an
explicit value type and validation operation.

## Integrations

Third-party services are package-provided connectors backed by narrow host
interfaces. A connector declares:

- operations and typed inputs/outputs
- required capabilities
- authentication handle kinds
- timeout and retry behavior
- idempotency behavior
- provider error variants
- nondeterministic fields

Connectors never receive the full process environment or unrestricted network
access. The host can grant a connector only the endpoints and secret handles
it needs.

Initial connector set:

1. generic HTTP and JSON
2. GitHub
3. Slack or Discord
4. provider-neutral AI invocation
5. schedules and webhooks

Breadth is less important than predictable, well-documented behavior.

## Failures

Recoverable failures are values, not hidden exceptions. Entry points return
typed results that make retry, rejection, and permanent failure visible.

The host supplies bounded policies for:

- deadlines
- retries with backoff
- rate limits
- idempotency keys
- concurrency
- response and payload sizes

Application code can request a narrower policy but cannot exceed host limits.

## State

The first runtime supports request-local immutable values. Durable state is
accessed through capability-scoped key/value or database interfaces.

State operations must make transaction, consistency, and retry behavior
explicit. Applications cannot access a host database through ambient
credentials.

## AI calls

AI is an optional connector, not a privileged language construct. Calls use:

- a provider-neutral model selector
- structured messages or input
- an expected output schema
- timeout and token limits
- explicit nondeterminism
- an `ai.invoke` capability

Model output is untrusted external input and must pass its declared schema
before use.

## Observability

The host emits structured request, effect, denial, latency, and resource
events. Krit code can add structured fields but cannot read or alter protected
audit metadata.

Logs redact secrets and capability contents by construction. Trace identifiers
are host values, not global mutable variables.

## First reference application

The first end-to-end acceptance application:

1. receives a typed HTTP webhook
2. verifies the webhook through a connector
3. reads an opaque approved secret
4. calls GitHub and an AI provider
5. posts a typed result to Slack
6. returns JSON
7. declares exact network, secret, and AI capabilities
8. runs inside the WebAssembly sandbox
9. exposes its effects and permissions through `krit explain`

The source should remain short enough to review as application policy rather
than infrastructure code.

## Success measures

Representative agent tasks compare Krit with generated Rust:

- source lines and syntax nodes
- distinct concepts exposed to the reviewer
- human review time
- first-generation check success
- automatic diagnostic-repair success
- capability over-request rate
- cold and warm startup
- peak memory
- request throughput and p95 latency

Krit succeeds when it reduces review complexity without hiding behavior or
authority.
