# WebAssembly sandbox

**Status:** Policy-1 scalar path and policy-2 bounded webhook host implemented
**Artifact metadata schema:** 1
**Validation policy:** 1

## Decision

The primary deployable Krit artifact is a small WebAssembly component. The
implemented policy-1 artifact and reference host have no ambient filesystem,
network, process, environment, clock, randomness, secret, AI, inherited
standard output, or WASI access. `krit sandbox` executes only a separately
built component whose adjacent metadata validates against its exact bytes.

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

The checked-in package `krit:runtime@0.2.0` defines two buildable versioned
policy-1 worlds. `pure-program` exports `run: func()` with no imports.
`program` exports the same function and imports only
`krit:runtime/stdout@0.2.0`:

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

The same package defines buildable `webhook`, `config`, `secrets`, `http`, and
`http-anonymous` interfaces. A finite deterministic webhook world exists for
every supported effect combination. HTTP without `secret.read` selects the
anonymous interface, so unauthenticated HTTP does not implicitly import
secret acquisition. Worlds containing both effects select the bearer-capable
`http` interface and the explicit `secrets` interface.
Its HTTP records preserve header order and duplicate names. `secrets.secret`
is a WIT `resource`, never bytes or string. The webhook base world exports the
canonical typed `handle` operation through its exported `webhook` interface
and has no world imports.

```wit
interface webhook {
    record header { name: string, value: string }
    record request {
        method: string,
        path: string,
        query: string,
        headers: list<header>,
        body: string,
    }
    record response {
        status: s64,
        headers: list<header>,
        body: string,
    }

    handle: func(request: request) -> response;
}

interface config {
    get-string: func(key: string) -> result<string, string>;
}

interface secrets {
    resource secret;
    acquire: func(name: string) -> result<secret, string>;
}

```

The `http` interface has the same closed request/response record shapes and
accepts `option<borrow<secrets.secret>>`; `http-anonymous` omits the bearer
parameter. A runtime-only all-host binding world supplies linker definitions,
but extra definitions are unreachable: validation accepts only the exact
component imports and corresponding finite compiler world.

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
traps. Compiler-generated checked-overflow branches deliberately execute a
genuine Wasm integer-overflow operation; arbitrary guest `unreachable`
instructions remain generic `K4001` traps rather than being misclassified as
K4005.

Policy 2 adds a deliberately bounded webhook ABI:

- canonical UTF-8 strings and checked linear-memory allocation/reallocation
- fixed `HttpHeader`, `HttpRequest`, and `HttpResponse` records
- ordered lists of headers
- the Result/Option shapes returned by config, secrets, and HTTP
- Result/Option matching and static references to non-capturing helpers
- a typed webhook export plus exact stdout/config/secrets/HTTP imports

The core uses guest linear-memory offsets, never host pointers. Canonical
parameters/results, list elements, records, variants, UTF-8, and allocation
bounds are checked by the component adapter and host.

Builds fail with `K7001` for residual parametric layouts and `K7002` for
unsupported semantics. Policy 1 rejects strings, lists, records, options,
results, JSON conversion, lexical captures, matches, unsupported print
values, and any operation without a correct lowering. It never substitutes a
trapping placeholder or direct-evaluator fallback.

Policy 2 does not generalize these layouts to arbitrary records/lists,
capturing closures, JSON, or dynamic string operators. Those shapes continue
to fail closed rather than receiving fake values.

## Artifact policy

Policy-1 core modules have no start function or linear memory. Function
values use one `funcref` table whose minimum and maximum are the same finite
size. Policy-2 webhook modules add one bounded 32-bit non-shared memory with a
16 MiB maximum and the standard32 realloc export; generated canonical adapter
modules remain bounded and are included in preflight accounting. The
validator enables only core MVP plus the Component Model and rejects
threads, shared memory, multi-memory, memory64, SIMD, floating-point use,
exceptions, GC, component async/threading, unknown sections, component starts,
WASI, and undeclared imports.

Schema-1 adjacent metadata contains compiler and language versions, edition,
package name/version, target, WIT world, package-relative entry, exact digest
and byte size, sorted effects/imports, exact resource requirements, build
profile, and validation policy version. Embedded metadata is bounded and
contains no source text, absolute paths, credentials, or secret values.
`krit-wasm::validate_artifact`
revalidates the bytes, policy, embedded facts, byte size, and digest. Validation
derives effects and the selected world from the validated component and core
import surfaces: zero imports mean no effects and `pure-program`; the exact
stdout component interface and canonical core imports mean `io.stdout` and
`program`. Embedded and adjacent claims must match those derived facts.

## Host runtime

`krit-runtime` uses one reusable Wasmtime engine and creates a fresh Store,
host state, component instance, and output buffer for every invocation. It
revalidates metadata, digest, component shape, exact WIT world, imports, and
resource shape before compiling. The pure world links no imports. The stdout
world links only the three checked-in typed stdout functions; it does not
inherit the CLI process stdout or add WASI.

The stdout implementation buffers bytes in host memory. Output is returned to
the CLI only after successful guest completion. A guest trap, host-call limit,
output limit, deadline, fuel exhaustion, or authorization failure discards the
entire invocation buffer. Each host write reserves and checks its complete
addition before changing the buffer or accounting counters.

Webhook invocation uses the same Engine but a fresh Store, instance, resource
table, output buffer, and host handles. Config is explicit immutable startup
data, never inherited environment. Secret bytes live behind an opaque
Wasmtime resource in a host-owned `zeroize` buffer. Persistent buffers are
zeroed on final drop; TLS/client-library transient copies cannot be guaranteed
zeroized, so they are tightly scoped and never formatted, serialized, or
logged.

Outbound HTTP uses exactly locked `curl` 0.4.50 with a statically linked,
rustls-backed libcurl and native platform trust roots. Environment proxies
are disabled. HTTP/1.1 header ordering, zero redirects, exact
scheme/host/effective-port checks, a per-request `CURLOPT_RESOLVE` pin, and
DNS/connect/read/overall bounds no greater than the invocation deadline are
enforced. Budgets below libcurl's one-millisecond resolution are treated as
expired rather than rounded to its unlimited-timeout sentinel. Default policy
rejects non-public IPv4 and IPv6 ranges, including private, shared,
documentation, benchmark, link-local, loopback, multicast, unspecified,
reserved, and metadata-class destinations. The only relaxation is an
embedding/test policy for loopback; bearer authentication over plain HTTP
needs a second explicit host-only switch.

The wall deadline uses Wasmtime epoch interruption. Because an engine epoch is
shared across Stores, each `Runtime` serializes component compilation and
execution behind one scheduler lock. The deadline worker is cancellable and
always joined before the invocation returns, so a completed invocation cannot
interrupt a later one. Component validation and compilation happen before the
execution deadline starts; this limitation is explicit and compilation is
still bounded by component and metadata input limits.

Under the [upstream release
policy](https://docs.wasmtime.dev/stability-release.html), Wasmtime 47 is a
non-LTS monthly release with a two-month support window (through 2026-09-20
for version 47). The dependency requirement is compatible `47.0.4`, while
`Cargo.lock` pins the exact tested patch. Security patches within 47 may be
adopted with `cargo update` plus the complete host test suite. Krit plans an
audited move to the Wasmtime 48 LTS line before 47 leaves support. A runtime
major or Rust MSRV increase is never implicit: it requires CI validation,
documentation, and a changelog entry.

URL parsing is pinned to `url` 2.5.8, HTTP types to `http` 1.5.0, the blocking
HTTP/TLS client to `curl` 0.4.50, secret clearing to `zeroize` 1.9.0, and CLI
serving to `tiny_http` 0.12.0. Unix secret opens use `rustix` 1.1.4 with
`O_NOFOLLOW`. Default features are disabled except curl's rustls and static-libcurl
backends. The ignored `trusted_public_https_smoke_test` exercises platform
roots when public DNS/network access is available. Every curl/libcurl upgrade requires an audited
`CURLOPT_RESOLVE`, proxy, redirect, TLS, timeout, and ordered-header review
plus the complete network test suite. Exact resolved transitive versions
remain in `Cargo.lock`.

## Resource limits

Policy 1 uses these exact default and hard host limits:

| Resource | Default | Hard maximum |
|---|---:|---:|
| Component bytes before compilation | 4 MiB | 16 MiB |
| Adjacent metadata bytes | 64 KiB | 1 MiB |
| Linear memory | 16 MiB | 64 MiB |
| Function-table elements | 4,096 | 65,536 |
| Instances | 16 | 64 |
| Tables | 8 | 32 |
| Memories | 1 | 8 |
| Wasm stack | 512 KiB | 8 MiB |
| Fuel | 10,000,000 | 1,000,000,000 |
| Wall deadline | 1 second | 30 seconds |
| Host calls | 1,024 | 1,000,000 |
| Buffered output | 1 MiB | 16 MiB |
| Request body | 1 MiB | 16 MiB |
| Response body | 1 MiB | 16 MiB |
| Header count | 128 | 1,024 |
| Header bytes | 64 KiB | 1 MiB |
| Outbound HTTP calls | 16 | 1,024 |
| Connect timeout | 250 ms | 5 s |
| Read phase timeout | 500 ms | 10 s |
| Overall HTTP timeout | 750 ms | 20 s |
| Host config bytes | 64 KiB | 1 MiB |
| Bytes per secret | 64 KiB | 1 MiB |

The embedded authority document is capped at 48 KiB, leaving deterministic
headroom for package, compiler, digest, import, and entry fields in the
default 64 KiB adjacent metadata budget.

The host configuration may narrow defaults or explicitly select the hard
maximum policy, but guest code and package metadata cannot raise the selected
limits. Policy 1 emits no guest linear memory; policy 2 emits one bounded
canonical memory. StoreLimits and preflight shape checks count both application
and generated adapter resources so adversarial components fail closed.

Every invocation runs on a dedicated native thread whose stack is the selected
Wasm stack limit plus 2 MiB of fixed host headroom. This prevents guest
recursion from reaching the caller thread's native guard page before Wasmtime
converts the configured Wasm stack limit into `K5103`. The runtime joins this
thread before returning, including after traps and deadline interruption.

Exhaustion produces a stable resource diagnostic distinct from capability
denial and application failure.

## CLI execution and inspection

```text
krit build [--manifest PATH] [--output PATH]
krit sandbox [--manifest PATH] [--artifact PATH]
krit invoke [--manifest PATH] [--artifact PATH] [--host-config PATH] --request FILE
krit serve [--manifest PATH] [--artifact PATH] [--host-config PATH] [--bind IP:PORT] [--once]
krit permissions --artifact PATH [--json] [MANIFEST]
```

`sandbox` defaults to `krit.pkg` and
`target/krit/<package-name>.wasm`, with metadata at `<artifact>.json`. It never
builds, interprets source, or searches for a fallback artifact. Missing files
are K7003 errors. Artifact-aware `permissions` validates the component and
prints complete requested, required, effective, denied, import, local-grant,
and deployment-grant facts. A denied local manifest exits 4 after printing
the report; deployment remains `not-evaluated`.

`invoke` and `serve` likewise never build or interpret source. `invoke`
accepts strict request-schema JSON and writes only response JSON after
success. `serve` binds loopback by default and uses the same invocation path;
`--once` handles one accepted or rejected request. Host config JSON contains
immutable strings and relative secret-file references only. Unknown fields,
inline values, environment inheritance, ungranted names, escaping paths,
symlinks, oversized files, and group/other-readable Unix secret files fail
closed.

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

The host, not the component, performs DNS, TLS, redirect rejection, proxy
disablement, and connection setup.

An outbound grant binds:

- scheme
- hostname
- port
- exact normalized origin (no path, query, fragment, or userinfo)
- per-call validated origin-form path, query, method, and headers
- fixed no-redirect policy
- response-size limit
- timeout and request quota

Resolved IP changes and redirects cannot widen the original hostname grant.
Private, link-local, and metadata endpoints are denied. Loopback is available
only through an explicit embedding/test policy and never through source or a
manifest.

## Secrets

Applications refer to secrets by manifest-approved logical name. The host
acquires bounded owner-only files into a zeroizing store and returns only a
WIT resource handle. The HTTP host borrows that resource and injects bearer
authentication without returning bytes to the component.

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
