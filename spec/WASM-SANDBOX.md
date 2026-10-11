# WebAssembly sandbox

**Status:** Policy-1 scalar path and Phase 6 policy-2 durable host implemented
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

The same package defines buildable `webhook`, `config`, `secrets`, `http`,
`http-anonymous`, `ai`, `logging`, and `state` interfaces. A finite deterministic
webhook world exists for every supported effect combination. HTTP without
`secret.read` selects the
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

The state interface exposes bounded string key/value and checkpoint operations
plus explicit anonymous HTTP and AI replay operations. Replay external authority
remains exact metadata requirements; importing state does not import the general
HTTP or AI interfaces.

Phase 6 jobs add three narrow import interfaces and two export interfaces:

```wit
interface queue {
    queue-publish: func(queue: string, body: string) -> result<string, string>;
}

interface objects-read {
    object-get: func(bucket: string, key: string) -> result<option<string>, string>;
}

interface objects-write {
    object-put: func(bucket: string, key: string, value: string) -> result<_, string>;
    object-delete: func(bucket: string, key: string) -> result<_, string>;
}

interface job {
    record delivery {
        queue: string,
        id: string,
        body: string,
        attempt: s64,
        max-attempts: s64,
    }
    handle: func(delivery: delivery) -> result<string, string>;
}

interface schedule {
    record trigger {
        schedule: string,
        id: string,
        scheduled-at-millis: s64,
        fired-at-millis: s64,
        attempt: s64,
        max-attempts: s64,
    }
    handle: func(trigger: trigger) -> result<string, string>;
}
```

Phase 7 adds one further import interface:

```wit
interface database {
    resource transaction;

    begin-read: func(database: string) -> result<transaction, string>;
    begin-write: func(database: string) -> result<transaction, string>;
    db-query: func(
        transaction: borrow<transaction>,
        statement: string,
        parameters: list<string>,
    ) -> result<string, string>;
    db-execute: func(
        transaction: borrow<transaction>,
        statement: string,
        parameters: list<string>,
    ) -> result<s64, string>;
    db-commit: func(transaction: borrow<transaction>) -> result<_, string>;
    db-rollback: func(transaction: borrow<transaction>) -> result<_, string>;
}

interface cache-read {
    cache-get: func(namespace: string, key: string) -> result<option<string>, string>;
}

interface cache-write {
    cache-put: func(namespace: string, key: string, value: string, ttl-seconds: s64)
        -> result<_, string>;
    cache-delete: func(namespace: string, key: string) -> result<_, string>;
}

interface search-query {
    search-query: func(index: string, query: string, limit: s64) -> result<string, string>;
}

interface search-vector {
    vector-search: func(index: string, vector: string, limit: s64) -> result<string, string>;
}
```

Object read and write are separate interfaces so that read-only authority never
implies write authority. Database read and write share one interface because
both need the same lifecycle operations; validation therefore re-derives the
read/write split from the embedded requirement contract and rejects an artifact
whose database imports and declared database effects disagree, while the runtime
still checks every call against both the manifest grant and the artifact's own
requirement set. Worlds are named
`{webhook|job|schedule}-<sorted import tokens>-program@0.2.0` over the fixed
import order `stdout, config, secrets, http, ai, logs, state, queue, objread,
objwrite, db, cacheread, cachewrite, searchquery, searchvector`, with bearer
`http` selected only alongside `secrets`. Cache read and write, and search query
and vector, are separate interfaces like the object surfaces, so effects are
re-derived directly from imports with no shared-interface compromise. The finite
world set is deterministic: the checked-in package carries every world that
existed before Phase 6, and any other least-authority world is generated
byte-identically from its mask at build and validation time. Validation
re-derives every effect from the actual component imports and export, so
`queue.consume` and `schedule.trigger` cannot be forged in metadata and no world
may import a surface the guest never uses. WASI, clocks, randomness, files,
sockets, and databases are never imported.

## Build pipeline

```text
Krit source
    -> resolve + type/effect check
    -> typed Core IR
    -> dead-function elimination from the selected entrypoint
    -> exhaustive residual-type and layout/support check (policy 1 or 2)
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
not inside, the hashed bytes. `krit check --target` runs the same pipeline
with the output discarded (see [Target checks](#target-checks)).

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
- String concatenation (`+`), equality (`==`, `!=`), and byte-lexicographic
  ordering (`<`, `<=`, `>`, `>=`) through the guest string runtime below
- `json_decode` into `String` through the guest JSON string decoder below
- fixed `HttpHeader`, `HttpRequest`, and `HttpResponse` records
- ordered lists of headers
- the Result/Option shapes returned by config, secrets, and HTTP
- Result/Option matching and static references to non-capturing helpers
- postfix `?` on those Result/Option shapes in helpers and in `queue` and
  `schedule` entrypoints, whenever the function's declared result has a
  layout (for example `Result<HttpResponse, String>`, `Result<String, String>`,
  or `Option<String>`); see [Function exits](#function-exits)
- a typed webhook export plus exact stdout/config/secrets/HTTP imports
- neutral AI string results and ordered structured logging fields
- transactional string state, named checkpoints, anonymous HTTP replay, and AI
  replay through the state interface

The core uses guest linear-memory offsets, never host pointers. Canonical
parameters/results, list elements, records, variants, UTF-8, and allocation
bounds are checked by the component adapter and host.

### Function exits

Core lowers postfix `?` (`LANGUAGE.md` section 8.2) to a variant match whose
failure arm builds the `Err` or `None` at the function's declared result type
and ends with an `exit` operation (`TYPES-AND-EFFECTS.md`, "Error
propagation"). Both code generators lower `exit` to loading its value and a
core `return`, which leaves the function from any depth of nested `if` blocks
with the function's own result layout: a policy-2 `Result` or `Option` result
is the `i32` guest offset of its variant, exactly what the function's normal
end returns, so callers and the entry adapter see one result representation.
Core verification guarantees that an `exit` is the last operation and the
result of its block; the instructions after the `return` that store that
block's result are unreachable, never execute, and validate against the
polymorphic operand stack, so no placeholder value is produced. Policy 1
lowers `exit` the same way but never meets one, because a `?` operand is a
`Result` or `Option`, which policy 1 has no layout for. Support checking
treats an `exit` like any other operation: its value has the function's
result type, which is checked with the function signature, and a `?` whose
`Result` or `Option` shape has no policy-2 layout fails at that shape with
`K7002`.

### Guest string runtime

A policy-2 `String` value is an `i32` guest offset of an 8-byte canonical
`(ptr, len)` pair describing immutable UTF-8 bytes, whether the bytes are a
static literal, were written by the host when lifting a canonical argument
or result, or were produced by concatenation. String operators lower to calls
of two generated core functions:

- `compare(left, right) -> i32` returns `-1`, `0`, or `1`. It compares the
  shared prefix eight bytes at a time until a word differs, then byte by
  byte as unsigned values; an equal shared prefix orders the shorter string
  first. `==`, `!=`, `<`, `<=`, `>`, and `>=` test that result against zero,
  so the guest order is exactly the byte-lexicographic UTF-8 order the
  direct evaluator uses (see `LANGUAGE.md` section 9).
- `concat(left, right) -> pair` returns the other operand's pair unchanged
  when one operand is empty (pairs are never mutated). Otherwise it checks
  the 32-bit length sum, allocates the joined bytes and a fresh pair through
  the module's bounded `realloc`, and copies both operands into the new
  allocation.

Each helper is a plain loop over MVP instructions (no bulk-memory or other
proposal), so the core validator keeps its empty feature set. A helper is
emitted at most once per module and only when a live function uses it; the
helpers follow every other function definition, so a module without String
operators contains no helper, no helper type, and unchanged function
indices. Allocation never frees: the bump allocator is bounded by the
16 MiB memory maximum and the host's memory limit. A 32-bit length overflow
traps (`K4001`) and a memory growth the host refuses traps (`K5103`), both
before any byte of the result is written; copies only ever target the fresh
allocation, and a trapping invocation publishes no response. Fuel bounds the
copy and compare loops like any other guest code.

### Guest JSON string decoding

`json_decode` whose inferred result is `String` lowers to a call of one more
generated core function, `json-decode-string(text) -> pair`, emitted under the
same rules as the string helpers (at most once per module, only when a live
function calls it, after every other definition). It accepts exactly the
texts for which the direct evaluator yields a string:

- The text is one JSON string, optionally preceded and followed by JSON
  whitespace: space, tab, line feed, and carriage return (RFC 8259 section
  2). Any other byte outside the string, including a byte-order mark or a
  form feed, is rejected.
- Inside the quotes every byte below 0x20 and every unescaped `"` is
  rejected. Every other byte, including DEL and multibyte UTF-8, is kept
  unchanged.
- The escapes are `\"`, `\\`, `\/`, `\b`, `\f`, `\n`, `\r`, `\t`, and
  `\uXXXX` with hex digits in either case (RFC 8259 section 7). A high
  surrogate escape (`\uD800`-`\uDBFF`) must be followed immediately by a low
  surrogate escape (`\uDC00`-`\uDFFF`), and the pair decodes to the one
  supplementary scalar value it encodes, as four UTF-8 bytes. A lone or
  reversed surrogate is rejected, as is any other escape.
- Valid JSON that is not a string (a number, `true`, `null`, an object, an
  array) is rejected. The direct evaluator returns such a value unchecked,
  because `json_decode` is not yet schema-directed; a component decoding into
  `String` has no layout for it and fails closed.

Every String is valid UTF-8 and escapes produce only scalar values, so the
result is valid UTF-8. RFC 8259 lets a parser accept lone surrogates and
ignore a byte-order mark; Krit rejects both, exactly as the evaluator does,
because a Krit string holds only Unicode scalar values.

The decoder first validates the whole text and measures its decoded length
without allocating, so a rejected text never allocates and is rejected the
same way at any size. A text without escapes decodes to a pair that shares
the input's content bytes (bytes are never mutated). Otherwise the exact
decoded length is allocated through the module's bounded `realloc`, which
traps with `K5103` when guest memory is exhausted, and a second pass writes
the decoded bytes into that fresh allocation only. Both passes consume runs
of plain bytes eight at a time with MVP integer instructions. Fuel bounds
them like any other guest code; a text with escapes is scanned twice and so
costs more than twice the fuel of plain text of the same length.

Rejection executes `unreachable` inside the decoder. The decoder is the only
function the core module names in its standard `name` custom section, as
`krit:json-decode-string` (no Krit identifier can contain `:`), and the
runtime reports an `unreachable` trap whose innermost frame is that function
as `K4009`, the code the direct evaluator uses for invalid JSON, with a fixed
message that never contains the input. The trap aborts the invocation and
publishes no response. Like every guest trap it is reported at the artifact
(`artifact.wasm:1:1`) rather than at the `json_decode` call, because
artifacts carry no source map. A module that never decodes JSON into `String`
has neither the decoder nor a `name` section.

### Support checking

Builds fail with `K7001` for residual parametric layouts and `K7002` for
unsupported semantics. Both code generators walk nested Core blocks
recursively; source nesting limits (`LANGUAGE.md` section 4.1) bound that
nesting, and the backend independently refuses Core nested more than 8,192
blocks deep with `K1005`, before support checking and equally in a
`krit check` target check, rather than relying on that bound. Policy 1 rejects
strings, lists, records, options, results (and so every postfix `?`), JSON
conversion, lexical captures, matches, unsupported print values, and any
operation without a correct lowering. It never substitutes a trapping
placeholder or direct-evaluator fallback. Policy 1 has no linear memory, so a
String operator in a module program is reported where the String value is
produced (the literal, parameter, or field), with the "String has no concrete
guest layout in WebAssembly policy 1" message.

Policy 2 does not generalize these layouts to arbitrary records/lists,
capturing closures, JSON other than `json_decode` into `String`, or
composite equality (`==` over options, results, records, or lists). Those
shapes continue to fail closed rather than receiving fake values.

Before any layout is chosen, dead-function elimination keeps only the
functions reachable from module initialization and the selected entrypoint.
A function is reachable when a reachable function uses its closure value as
an operand (call, argument, field, list element, branch result, and so on)
or captures a binding bound to it; binding or discarding a closure is not a
use. An unreferenced top-level helper is therefore neither checked nor
lowered, so an unused generic helper no longer fails the module with
`K7001`. Eliminated functions keep their Core identifiers and table slots
and are emitted as trapping stubs that no live code references.

The support check then reports every finding rather than the first: each
residual type (`K7001`) and each unsupported construct (`K7002`) in a live
function, at the originating Core operation's source span, falling back to
the enclosing block, parameter, or function declaration. A type that still
contains an inference variable is reported only as `K7001`. Diagnostics are
sorted by span start, span end, and code; one code is reported at most once
per source position (the narrowest span wins when nested constructs start
together). Code generation runs only after the support check passes and is
a backstop: any error it raises is attached to the Core operation or
function being lowered, so no source construct is reported without a
location. Only internal failures that no source construct causes (a Core
verification error, or `K7003`/`K7004` artifact failures) can be spanless.

`K7002` messages name the layout policy of the program kind: module programs
use "WebAssembly policy 1", and webhook, queue, and schedule entrypoint
programs use "the bounded webhook ABI (WebAssembly policy 2)". This layout
policy is distinct from the metadata validation `policyVersion`, which
records whether the artifact imports host surfaces introduced after artifact
policy 1.

## Target checks

`krit check [--target component|module|none] FILE` adds a target check after
successful analysis and Core lowering. A target check runs the complete
`krit build` pipeline above (dead-function elimination, support checking,
code generation, componentization, metadata attachment, and validation)
with the output discarded, so it reports exactly the `K7001`/`K7002` set,
codes and spans, that `krit build` reports for the same source. It reads no
manifest: package grants (`K5001`) remain a `build` check.

- `component` checks the artifact `krit build` would emit: the selected
  webhook, queue, or schedule world, or the module-program world.
- `module` checks the file as a module program; a webhook, queue, or
  schedule entrypoint is reported as `K7002` at its declaration because
  entrypoint programs build only as components.
- `none` skips the target check.

Without `--target`, a file that declares a webhook, queue, or schedule
entrypoint is checked as `component`, because a component is the only way
such a file can run. Other files default to `none`; `krit run` executes them
directly. Every target diagnostic is printed in the selected
`--diagnostic-format`, in the sorted order above, and the command exits `1`
without printing its success line. The language server does not run target
checks in 0.3.

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
profile, approval-required AI/bearer resources, and validation policy version.
State-free artifacts remain policy 1. Artifacts importing the state interface
use policy 2 and must carry exact state and replay external-resource
requirements.
Embedded metadata is bounded and
contains no source text, absolute paths, credentials, or secret values.
`krit-wasm::validate_artifact`
revalidates the bytes, policy, embedded facts, byte size, and digest. Validation
derives effects and the selected world from the validated component and core
import surfaces: zero imports mean no effects and `pure-program`; the exact
stdout component interface and canonical core imports mean `io.stdout` and
`program`. Embedded and adjacent claims must match those derived facts.

## Artifact compatibility

A runtime loads an artifact only when every recorded compatibility fact is a
member of the runtime's published supported sets
(`krit_wasm::SUPPORTED_*`):

| Fact | Recorded in | Supported |
|---|---|---|
| Metadata schema | embedded and adjacent `schema` | 1 |
| Validation policy version | embedded and adjacent `policyVersion` | 1, 2 |
| `krit:runtime` WIT package version | component import and export names | 0.2.0 |
| Edition | embedded and adjacent `edition` | 2026 |

The compiler version (embedded `compilerVersion`, adjacent `compiler.version`,
and the producers `processed-by` entry) and the language version (adjacent
`language.version` and the producers `language` entry) are recorded, never
required to equal the running toolchain. Each is compared with the runtime's
own compiler or language version only by the toolchain window: it must be a
strict Semantic Versioning 2.0 version with the runtime's major version, and
its minor version must be the runtime's own or one of the two before it. A 0.3 runtime therefore loads artifacts from
0.1, 0.2, and 0.3 toolchains and refuses 0.4 and newer, any other major, and
unparsable versions. Patch, pre-release, and build components never affect
the outcome. The same rule applies after 1.0 until a release publishes a
longer window.

Self-consistency stays exact. The adjacent compiler version, language
version, and edition must equal the ones the component records, and the
embedded compiler version must equal the producers `processed-by` version.
Compatibility never relaxes derivation: effects, the WIT world, and the policy
version are always re-derived from the validated component and core import
surfaces, and every recorded claim must match them. An unsupported fact fails
closed before compilation, as K7004 for adjacent metadata and K7003 for the
component's own records. Messages name the supported set or window and echo
only parsed version numbers, never other artifact text.

Loading an older WIT package version means linking its worlds, so when the
checked-in package moves to a new version, the previous one stays in the
supported set while the window still includes a toolchain that emitted it.
`conformance/build/compat/<toolchain>/` keeps golden artifacts written by each
released toolchain; the runtime must load and run every case there.

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
table, output buffer, structured-log buffer, and host handles. A reusable
embedding-owned `AgentHost` separately owns bounded rate and idempotency state
across requests. Idempotency is bounded by both entry count and retained
response bytes. Phase 6 optionally backs idempotency and guest-visible state
with host-configured SQLite while every component Store and transaction
overlay remains fresh. Config is explicit immutable startup
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

The neutral AI interface selects a named host adapter. Phase 4 includes only
`http-json`, which constructs and parses one strict provider mapping behind a
provider-neutral Rust trait. Adapter origin and optional secret must already
be manifest-granted, and a bound secret must list the adapter origin. Prompts,
provider bodies, model output, and credentials are absent from metadata,
permission output, default logs, cache keys, and stats.

Transport retries occur inside one host operation and never re-enter guest
code. Only connection/timeouts and 429/502/503/504 are retryable, and only for
GET/HEAD or a request carrying one valid ordinary idempotency key. Backoff and
Retry-After are capped by the total deadline. Finite fixed-window counters are
owned by `AgentHost` and keyed by exact origin or adapter.

An atomic embedding cancellation handle is checked before instantiation, on
every host call and retry/backoff, during DNS waiting, and by libcurl's
progress callback during active transfers. DNS worker creation is capped.
Epoch interruption remains responsible for guest execution but is not relied
upon to interrupt blocking HTTP.

Inbound mutating requests may use one bounded `Idempotency-Key`. Without a
schema-3 durable store selection, `AgentHost` preserves the Phase 4
process-local TTL/LRU cache. With an explicit manifest-granted selection,
SQLite persists in-progress leases and completed serialized responses across
`AgentHost` and process restarts. Matching requests replay without Store
creation; credential-sensitive digest conflicts return 409; live reservations
reject duplicate work. Failed/trapped/state-conflicted invocations remove
their owned reservation.

Invocation state writes and checkpoints are staged in host memory and commit
with one revision-checked SQLite transaction only after successful guest and
response validation. Replay records are separate immediate durable commits so
completed external operations survive a later guest trap/cancellation. The
remaining provider-effect-before-record crash window is explicit and no
distributed exactly-once behavior is claimed.

The wall deadline uses Wasmtime epoch interruption. Because an engine epoch is
shared across Stores, each `Runtime` serializes component compilation and
execution behind one scheduler lock. The deadline worker is cancellable and
always joined before the invocation returns, so a completed invocation cannot
interrupt a later one. Component validation and compilation happen before the
execution deadline starts; this limitation is explicit and compilation is
still bounded by component and metadata input limits.

Under the [upstream release
policy](https://docs.wasmtime.dev/stability-release.html), a Wasmtime release
whose major version is a multiple of 12 is a long-term-support (LTS) release
supported for 24 months; other monthly releases are supported for two months.
Krit follows LTS lines only. The host runs Wasmtime 48 LTS, released
2026-08-20 and supported through 2028-08-20. The dependency requirement is
compatible `48.0.5`, while `Cargo.lock` pins the exact tested patch. Security
patches within 48 may be adopted with `cargo update` plus the complete host
test suite. Wasmtime 48 requires Rust 1.95, which is the workspace MSRV. A
runtime major or Rust MSRV increase is never implicit: it requires CI
validation, documentation, and a changelog entry.

URL parsing is pinned to `url` 2.5.8, HTTP types to `http` 1.5.0, the blocking
HTTP/TLS client to `curl` 0.4.50, secret clearing to `zeroize` 1.9.0, and CLI
serving to `tiny_http` 0.12.0. Unix secret opens use `rustix` 1.1.4 with
`O_NOFOLLOW`. Default features are disabled except curl's rustls and static-libcurl
backends. The ignored `trusted_public_https_smoke_test` exercises platform
Durable state, queues, schedules, and object buckets use locked `rusqlite`
0.40.2 with bundled SQLite through the isolated `krit-state` crate. Application
databases use the same locked bundled SQLite through the separate
`krit-database` crate, which shares no schema, table, or migration logic with
`krit-state`; Phase 7 adds no new dependency. SQLite paths
are never guest-visible and no system SQLite linkage is used. Phase 6 jobs and
object storage add no new dependency. The ignored `trusted_public_https_smoke_test` exercises platform
roots when public DNS/network access is available. Every curl/libcurl upgrade requires an audited
`CURLOPT_RESOLVE`, proxy, redirect, TLS, timeout, and ordered-header review
plus the complete network test suite. Exact resolved transitive versions
remain in `Cargo.lock`.

## Resource limits

The runtime uses these exact default and hard host limits across policy 1 and
the bounded policy-2 webhook surface:

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
| AI calls | 8 | 256 |
| AI input | 64 KiB | 1 MiB |
| AI response | 64 KiB | 1 MiB |
| AI timeout | 750 ms | 20 s |
| Structured log events | 128 | 1,024 |
| Fields per log event | 32 | 128 |
| Bytes per log field value | 4 KiB | 64 KiB |
| Total structured log bytes | 64 KiB | 1 MiB |
| Connect timeout | 250 ms | 5 s |
| Read phase timeout | 500 ms | 10 s |
| Overall HTTP timeout | 750 ms | 20 s |
| Host config bytes | 64 KiB | 1 MiB |
| Bytes per secret | 64 KiB | 1 MiB |
| Durable stores | 4 | 16 |
| State operations per invocation | 128 | 1,024 |
| State key | 256 bytes | 4 KiB |
| State/checkpoint value | 64 KiB | 1 MiB |
| Staged state transaction | 1 MiB | 16 MiB |
| Durable database | 64 MiB | 1 MiB minimum, 1 GiB maximum |
| SQLite busy timeout | 250 ms | 5 s |
| Replay entries per store | 1,024 | 65,536 |
| Replay retained bytes | 16 MiB | 256 MiB |
| Replay TTL | 7 days | 30 days |
| Replay/in-progress lease | 30 seconds | 5 minutes |
| Configured queues, schedules, buckets | none | 16 each |
| Queue depth | none | 65,536 |
| Job body | none | 1 MiB |
| Retained queue bytes | none | 256 MiB |
| Delivery attempts | none | 16 |
| Delivery lease | none | 5 minutes |
| Retry backoff | none | 1 hour |
| Dead-letter entries | none | 4,096 |
| Dead-letter retention | none | 30 days |
| Schedule interval | none | 1 second .. 365 days |
| Schedule catch-up per tick | none | 64 |
| Retained schedule fires | none | 4,096 |
| Objects per bucket | none | 65,536 |
| Object key | none | 1 KiB |
| Object value | none | 4 MiB |
| Bucket bytes | none | 1 GiB |
| Delivery outcome detail | 4 KiB | 4 KiB |
| Deliveries per CLI dispatch | 1 | 1,024 |
| Delivery lease minimum | — | deadline + store busy timeout |
| Collected `--json` guest output | — | twice the output limit |
| Configured application databases | none | 8 |
| Catalog statements per database | none | 64 |
| Statement SQL | none | 4 KiB |
| Database parameters per statement | none | 16 |
| Database result columns | none | 32 |
| Database parameter bytes | none | 64 KiB |
| Database result rows | none | 4,096 |
| Encoded database result | none | 256 KiB |
| Cache namespaces | none | 16 |
| Cache entries per namespace | none | 4,096 |
| Cache entries in total | none | 16,384 |
| Cache key bytes | none | 512 |
| Cache value bytes | none | 64 KiB |
| Cache bytes per namespace | none | 8 MiB |
| Cache bytes in total | none | 64 MiB |
| Cache time to live | none | 1 s to 7 days |
| Search connectors | none | 8 |
| Search query bytes | none | 4 KiB |
| Encoded vector bytes | none | 64 KiB |
| Vector dimensions | none | 4,096 |
| Search results per call | none | 100 |
| Encoded search result | none | 256 KiB |
| Operations per transaction | none | 256 |
| Transactions per invocation | 1 | 8 |
| Open transactions at once | — | 1 |
| Transaction wall bound | none | 5 s |
| Application database bytes | none | 64 KiB .. 1 GiB |

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

Fuel uses Wasmtime's default cost model. The runtime sets the budget before it
instantiates the component, so instantiation and the call draw on one budget.
Each function invocation costs one unit, and each executed operator costs one
unit except `nop`, `drop`, `block`, `loop`, `else`, `end`, `return`, and
`unreachable`, which are free. `memory.grow` costs one unit however many pages
it requests. Wasmtime charges bulk memory, table, and GC array operators per
byte or element and checks the epoch deadline only once per such operation;
the feature policy rejects all of them in guest code.

Instantiation also consumes fuel, and part of that cost depends on the host.
A core module runs a startup function when it has initialization Wasmtime can
neither precompute nor map, such as the active element segment with which a
generated canonical adapter module fills an imported table. Entering that
function costs one unit, and Wasmtime may add small charges for the constant
expressions and elements it evaluates. Active data segments dominate. Every
Krit core module with linear memory places its literal data in one active
segment at a constant offset. Wasmtime merges such segments into one image
padded to the compilation target's page alignment: 4 KiB on x86-64, 16 KiB on
Apple aarch64, and 64 KiB on other aarch64 targets. Where Wasmtime maps that
image copy-on-write, which today means Linux through `memfd`, initializing it
costs nothing. On every other host, including macOS and Windows, the startup
function copies the image like `memory.init` at one unit per byte of the
padded image. `fuel_consumed` for those artifacts is therefore host-dependent:
on an Apple aarch64 host a one-page image costs 16,384 units beyond the
startup unit, which a Linux host never pays. A narrowed fuel budget must be
sized for the most expensive host that will run the artifact, and running out
during instantiation is `K5101` like any other fuel exhaustion. Host tests pin
the operator costs and these instantiation charges with per-platform
expectations, so a runtime upgrade that changes either fails before release.

Exhaustion produces a stable resource diagnostic distinct from capability
denial and application failure. A memory or table growth that the limits
refuse traps the guest and is reported as `K5103`, including when the
refusal arrives wrapped in a guest backtrace, as it does when the guest's
bounded allocator grows memory for a string.

## CLI execution and inspection

```text
krit check [--diagnostic-format human|json] [--target component|module|none] FILE
krit build [--manifest PATH] [--output PATH] [--diagnostic-format human|json]
krit sandbox [--manifest PATH] [--artifact PATH]
krit invoke [--manifest PATH] [--artifact PATH] [--host-config PATH] --request FILE
krit serve [--manifest PATH] [--artifact PATH] [--host-config PATH] [--bind IP:PORT] [--once]
krit permissions --artifact PATH [--json] [MANIFEST]
```

`build` prints every manifest, source, authority, and target diagnostic in
the selected format (human by default, JSON Lines with
`--diagnostic-format json`); on a target failure it prints all backend
diagnostics in span order. `sandbox` defaults to `krit.pkg` and
`target/krit/<package-name>.wasm`, with metadata at `<artifact>.json`. It never
builds, interprets source, or searches for a fallback artifact. Missing files
are K7003 errors. Artifact-aware `permissions` validates the component and
prints complete requested, required, effective, denied, import, local-grant,
and deployment-grant facts. A denied local manifest exits 4 after printing
the report; deployment remains `not-evaluated`.

`invoke` and `serve` likewise never build or interpret source. `invoke`
accepts strict request-schema JSON and writes only response JSON after
success. `serve` binds loopback by default and uses the same invocation path;
`--once` handles one accepted or rejected request. Host config schema 1
contains immutable strings and relative secret-file references only. Strict
schema 2 retains those fields and adds bounded AI adapters, retries, rates,
idempotency, and explicit noninteractive approval allow entries. Unknown
fields, inline values, environment inheritance, ungranted names, escaping
paths, symlinks, oversized files, and group/other-readable Unix secret files
fail closed. Non-Unix hosts refuse secrets outright; see
[Platform support](#platform-support).

Schema 3 retains all schema-2 fields and adds manifest-narrowed durable stores,
durability/limit policy, and an optional durable inbound-idempotency store.
There is no default state path. Relative `.db` paths require existing
owner-only directories on Unix; database/WAL/SHM files reject symlinks,
non-files, oversize data, and group/other permissions. Schema 1 and 2 remain
readable unchanged. Non-Unix hosts refuse durable state outright.

`invoke` response JSON remains exact on stdout. Structured application logs
are emitted as JSON Lines on stderr only after completion. `serve` never
places them in an HTTP body. Validated redacted failure logs may publish with
an explicit failure outcome while normal output/response remains rolled back.
Artifact permissions list approval-required resources separately and keep
deployment and approval evaluation `not-evaluated`.

## Platform support

Linux and macOS (Unix hosts) are supported for every command. Windows is
supported for `check`, `build`, `fmt`, `lsp`, and `run`, and for host mode
(`invoke`, `worker`, `schedule`, and `serve`) only without durable state or
secrets.

Host mode protects secret files, durable state stores, and application
databases with owner-only file permissions, and only Unix hosts enforce them.
On any non-Unix host, a host configuration with a non-empty `secrets`,
`state` (a store or a durable idempotency store), `jobs` (queues, schedules,
or object buckets), or `databases` section is refused with `K5001` and exit
status 4. The refusal follows strict parsing and comes before any secret file
is read and before any store or database is resolved, created, or opened. Its
message names the offending sections only, never secret names, paths, or
values. Config values; retry, rate, idempotency, and approval policy; AI
adapters and search connectors that use no secret; and the process-local cache
remain available. Windows can hold durable state or secrets only once
owner-only access-control checks exist there.

## Instance lifecycle

The safest baseline creates a fresh component instance and invocation
transaction overlay for each invocation. Explicit durable state lives only in
the host store. Hosts may pool initialized instances only when:

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
- per-call validated origin-form path, query, method, and headers; the path
  obeys the request path rule in `WEBHOOK-CONTRACTS.md` ("Request paths and
  queries"), and libcurl never resolves its dot segments or decodes its escapes
  (`CURLOPT_PATH_AS_IS`)
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
WIT resource handle that carries the secret's logical name. Hosts that cannot
enforce owner-only files refuse secrets (see [Platform support](#platform-support)).
The HTTP host borrows that resource, checks the name's manifest origin binding
(`K5006` on a mismatch, before any network I/O), and injects bearer
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
