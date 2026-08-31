# WebAssembly sandbox

**Status:** Draft
**Target:** Krit 0.4

## Decision

The primary deployable Krit artifact is a small WebAssembly component. The
component has no ambient filesystem, network, process, environment, clock,
randomness, secret, or AI access.

All external operations cross typed host interfaces backed by explicit
capability handles.

WebAssembly is one security layer, not a complete security claim. Production
hosts use defense in depth with runtime validation, quotas, operating-system
isolation, and narrow credentials.

## Threat model

The sandbox assumes source, generated code, package code, model output, and
compiled components may be buggy or malicious.

It protects the host and other applications from:

- arbitrary host memory access
- undeclared filesystem or network access
- process and environment access
- unbounded CPU, memory, table, stack, or output use
- forged capability handles
- cross-tenant state access
- package initialization side effects

It does not by itself protect against:

- vulnerabilities in the WebAssembly runtime or host interfaces
- denial of service outside configured limits
- authorized but harmful API operations
- secrets intentionally returned by an authorized external service
- unsafe host extensions
- application-level authorization mistakes

## Component model

Krit targets the WebAssembly Component Model and canonical typed interfaces.
The exact supported version is recorded in the compiler and artifact metadata.
Unknown component features fail closed.

Components export typed agent entry points and import only interfaces selected
from the package's resolved capability plan.

Conceptual imports:

```text
krit:http/client
krit:secrets/read
krit:ai/invoke
krit:state/key-value
krit:log/structured
```

Krit does not grant a general WASI environment to agent components. Preview-1
filesystem and socket inheritance are not a compatibility path.

## Build pipeline

```text
Krit source
    -> resolve + type/effect check
    -> typed Core IR
    -> WebAssembly component
    -> validate
    -> attach source/effect/interface metadata
    -> content hash
    -> optional signature
    -> immutable artifact store
```

Validation occurs after compilation and before every untrusted artifact enters
the cache. Cached validation can be trusted only when the runtime, validator,
artifact bytes, and feature policy hashes match.

## Host runtime

The Rust host runtime:

- validates component structure and imports
- constructs capability handles
- enforces memory, execution, and output limits
- maps typed exports to HTTP, webhook, schedule, queue, and tool entry points
- records structured effect and denial events
- controls deadlines, cancellation, and shutdown
- prevents environment and credential inheritance

Wasmtime is the initial reference runtime candidate because it supports the
component model, fuel/epoch interruption, and mature Rust embedding. Krit
keeps its host interfaces runtime-neutral so another conforming runtime can be
used when footprint or platform requirements justify it.

## Resource limits

Every invocation receives explicit maximums:

- linear memory
- table elements
- stack depth
- execution fuel or equivalent instruction budget
- wall-clock deadline
- host calls
- concurrent tasks
- request and response bytes
- log bytes
- outbound requests
- AI tokens and calls

Limits are set by the host and may be narrowed by the package. An application
cannot raise them.

Exhaustion produces a stable resource diagnostic distinct from capability
denial and application failure.

## Instance lifecycle

The safest baseline creates a fresh logical application state for each
invocation. Hosts may pool initialized instances only when:

- linear memory and mutable globals are reset
- capability handles cannot survive the invocation
- request-local state is cleared
- cancellation from a prior request cannot fire in the next
- differential tests prove pooled and fresh behavior equivalent

Long-running schedules and queue consumers are repeated bounded invocations,
not immortal unrestricted component threads.

## Capabilities

A component import is necessary but not sufficient for authority.

Effective authority is the intersection of:

1. statically inferred effects
2. package-declared capability requests
3. deployment policy grants
4. host interface restrictions
5. operating-system restrictions

Unused capability requests are warnings. Missing grants prevent instantiation
or fail at a documented optional-operation boundary.

Handles are unforgeable host resources. Raw secret values, sockets, file
descriptors, cloud credentials, and provider clients are not placed in
component linear memory unless an interface explicitly returns bounded data.

## HTTP and network safety

The host, not the component, performs DNS, TLS, redirects, proxy handling, and
connection pooling.

An outbound grant binds:

- scheme
- hostname
- port
- optional path prefix and method set
- redirect policy
- response-size limit
- timeout and request quota

Resolved IP changes and redirects cannot widen the original hostname grant.
Private, loopback, link-local, and metadata endpoints require separate
explicit authority.

## Secrets

Applications refer to secrets by manifest-approved logical name. Connectors
receive opaque authentication handles where possible.

Secret data is excluded from:

- component artifacts
- build and execution cache keys
- diagnostics and explanations
- logs and traces
- crash reports
- LLM authoring context

## Small artifacts

The component contains application logic, required language support, public
interfaces, and source maps selected by the build profile. It does not bundle
the host runtime, TLS stack, connector implementations, or provider SDKs.

The compiler uses section-level dead elimination and shares host functionality
through imports. Size goals are established only from measured reference
applications.

## Defense in depth

Untrusted multi-tenant deployments add:

- a dedicated unprivileged host process or container
- OS memory and CPU quotas
- restricted system calls
- read-only artifact mounts
- isolated temporary storage
- egress enforcement outside the process
- per-tenant identity and storage namespaces

If a required isolation mechanism is unavailable, untrusted mode fails closed.

## Security testing

Required coverage:

- malformed component fuzzing
- import and interface mismatch tests
- fuel, memory, stack, output, and deadline exhaustion
- path traversal, symlink, redirect, DNS rebinding, and metadata endpoint tests
- forged and stale handle tests
- instance-pool reset tests
- secret-redaction tests
- hostile connector response tests
- component/runtime differential conformance

Security claims name the runtime version, host policy, OS boundary, and known
limitations.
