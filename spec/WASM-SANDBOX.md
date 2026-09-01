# WebAssembly sandbox

**Status:** Artifact pipeline implemented; runtime host draft
**Artifact metadata schema:** 1
**Validation policy:** 1

## Decision

The primary deployable Krit artifact is a small WebAssembly component. The
implemented policy-1 artifact has no ambient filesystem, network, process,
environment, clock, randomness, secret, AI, or WASI access. A component host
is not implemented yet, so this milestone builds and inspects artifacts but
does not execute them in a sandbox.

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

The checked-in package `krit:runtime@0.2.0` defines two versioned policy-1
worlds. `pure-program` exports `run: func()` with no imports. `program` exports
the same function and imports only `krit:runtime/stdout@0.2.0`:

```wit
interface stdout {
    write-int: func(value: s64, newline: bool);
    write-bool: func(value: bool, newline: bool);
    write-unit: func(newline: bool);
}

world pure-program {
    export run: func();
}

world program {
    import stdout;
    export run: func();
}
```

The backend selects the exact world from checked effects: no effects select
`pure-program`, while exactly `io.stdout` selects `program`. Manifest grants
are only an upper bound and unused stdout authority does not add an import.
The backend derives the standard32 core names and canonical ABI signatures
from the selected parsed WIT and verifies the expected scalar contract. It
does not maintain a second unchecked ABI declaration or add optional dummy
imports. Krit does not grant a general WASI environment; Preview 1 inheritance
is not a compatibility path.

## Build pipeline

```text
Krit source
    -> resolve + type/effect check
    -> typed Core IR
    -> policy-1 layout/support check
    -> wasm-encoder core module
    -> restricted core validation
    -> embed parsed WIT component types
    -> wit-component componentization with validation
    -> bounded standard + krit.metadata metadata
    -> restricted component validation
    -> BLAKE3 hash of the exact final bytes
    -> adjacent schema-1 artifact metadata
```

`krit build` performs this complete pipeline in memory before writing. The
component and adjacent JSON are staged beside their destinations and replaced
with rollback on failure. The digest is `blake3:<hex>` and is stored beside,
not inside, the hashed bytes.

## Implemented backend support

Policy 1 represents `Int` as core `i64`, `Bool` as core `i32`, `Unit` as a
zero-width value, and non-capturing function values as bounded-table `i32`
slots. It supports:

- integer, boolean, unit, and non-capturing function values
- named recursion and higher-order non-capturing calls through `call_indirect`
- literals, immutable binds, discards, nested blocks, conditionals, and
  source-ordered short circuiting
- checked negation, addition, subtraction, multiplication, division, and
  remainder
- integer ordering and primitive equality
- `print` and `println` for `Int`, `Bool`, and `Unit`

Overflow, division by zero, and remainder by zero produce deterministic Wasm
traps. Mapping traps to stable runtime diagnostics belongs to the host
milestone.

Builds fail with `K7001` for residual parametric layouts and `K7002` for
unsupported semantics. Policy 1 rejects strings, lists, records, options,
results, JSON conversion, lexical captures, matches, unsupported print
values, and any operation without a correct lowering. It never substitutes a
trapping placeholder or direct-evaluator fallback.

## Artifact policy

The emitted core module has no start function or linear memory. Function
values use one `funcref` table whose minimum and maximum are the same finite
size. The validator enables only core MVP plus the Component Model and rejects
threads, shared memory, multi-memory, memory64, SIMD, floating-point use,
exceptions, GC, component async/threading, unknown sections, component starts,
WASI, and undeclared imports.

Schema-1 adjacent metadata contains compiler and language versions, edition,
package name/version, target, WIT world, package-relative entry, exact digest
and byte size, sorted effects/imports, build profile, and validation policy
version. Embedded metadata is bounded and contains no source text, absolute
paths, credentials, or secret values. `krit-wasm::validate_artifact`
revalidates the bytes, policy, embedded facts, byte size, and digest. Validation
derives effects and the selected world from the validated component and core
import surfaces: zero imports mean no effects and `pure-program`; the exact
stdout component interface and canonical core imports mean `io.stdout` and
`program`. Embedded and adjacent claims must match those derived facts.

## Host runtime (not implemented)

The planned Rust host runtime will:

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
