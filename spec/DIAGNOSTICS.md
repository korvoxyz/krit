# Krit diagnostic contract

**Status:** Normative  
**Schema:** 1

Diagnostics are part of Krit's human and AI tooling interface. Their codes and
structured fields are stable within an edition; prose may improve without
breaking compatibility.

## Human format

The default format is:

```text
path/to/file.krit:4:11: error[K2001]: undefined name `total`
```

An implementation may add source excerpts and suggestions after this first
line. It must not expose a host-language stack trace unless explicitly
requested with a developer option.

Paths should be relative to the package root. Standalone files use the path
provided by the user. Command-line expressions use `<command-line>` and the
REPL uses `<repl>`.

## JSON Lines format

`--diagnostic-format json` emits one compact JSON object per line to standard
error:

```json
{"schema":1,"severity":"error","code":"K2001","message":"undefined name `total`","file":"src/main.krit","span":{"start":{"line":4,"column":11,"byte":57},"end":{"line":4,"column":16,"byte":62}},"labels":[],"notes":[]}
```

Required top-level fields:

| Field | Type | Meaning |
|---|---|---|
| `schema` | integer | Diagnostic schema version, currently `1` |
| `severity` | string | `error`, `warning`, or `info` |
| `code` | string | Stable Krit diagnostic code |
| `message` | string | Concise UTF-8 explanation |
| `file` | string | Package-relative or synthetic source name |
| `span` | object or null | Primary source range |
| `labels` | array | Additional labeled source ranges |
| `notes` | array of strings | Context that has no precise source range |

Line and column values are one-based. Byte offsets are zero-based UTF-8 byte
offsets. End positions are exclusive.

Unknown fields may be added within schema 1. Consumers must ignore unknown
fields and must reject an unknown schema number.

## Code ranges

| Range | Category |
|---|---|
| `K0001`-`K0999` | Source decoding and lexical errors |
| `K1000`-`K1999` | Syntax errors |
| `K2000`-`K2999` | Name and module resolution |
| `K3000`-`K3999` | Static type and effect errors |
| `K4000`-`K4999` | Runtime errors |
| `K5000`-`K5999` | Capability violations |
| `K6000`-`K6999` | Package, manifest, and lockfile errors |
| `K7000`-`K7999` | Build and cache errors |
| `K8000`-`K8999` | Formatter and tooling errors |

Initial required codes:

| Code | Meaning |
|---|---|
| `K0001` | Invalid source character or token |
| `K0002` | Unterminated string |
| `K0003` | Invalid string escape |
| `K1001` | Unexpected token |
| `K1002` | Expected token not found |
| `K1003` | Invalid match pattern |
| `K1004` | Webhook declaration is nested or otherwise misplaced |
| `K2001` | Undefined name |
| `K2002` | Duplicate binding or parameter |
| `K3001` | Static type mismatch or invalid typed operator |
| `K3002` | Statically known non-function call target |
| `K3003` | Static function argument count mismatch |
| `K3004` | Invalid record field access |
| `K3005` | Invalid match subject, family, or exhaustiveness |
| `K3006` | Type is not comparable or JSON-encodable |
| `K3007` | Webhook signature is not exactly `(HttpRequest) -> HttpResponse` |
| `K3008` | Host capability resource is not a direct valid string literal |
| `K3009` | Opaque secret is used by a revealing or structural operation |
| `K3010` | Opaque database transaction handle is used outside a database operation |
| `K4001` | Generic guest trap or wrong runtime value kind |
| `K4002` | Calling a non-function |
| `K4003` | Function argument count mismatch |
| `K4004` | Division or remainder by zero |
| `K4005` | Integer overflow |
| `K4006` | Function comparison |
| `K4007` | Output operation failed |
| `K4008` | Value cannot be encoded as JSON |
| `K4009` | Invalid or unsupported JSON input |
| `K5001` | Required capability not granted |
| `K5002` | Artifact import/world does not match the authorized host interface |
| `K5003` | Required host contract is unavailable in direct source execution |
| `K5101` | Wasm fuel budget exhausted |
| `K5102` | Wasm wall-clock deadline exceeded |
| `K5103` | Wasm memory, table, instance, stack, or host resource limit exceeded |
| `K5104` | Host-call limit exceeded |
| `K5105` | Buffered output-byte limit exceeded |
| `K5106` | Embedding cancellation requested before guest execution |
| `K5201` | Durable state database, schema, corruption, migration, or I/O failure |
| `K5202` | Durable state transaction conflict, configured state/queue/schedule/object limit exceeded, or an unrepresentable delivery instant |
| `K5203` | Replay operation is unsafe, in progress, expired incorrectly, or conflicts with recorded input |
| `K5204` | Durable inbound idempotency reservation or completion failed |
| `K5205` | Durable delivery lease, acknowledgement, or outcome detail is invalid |
| `K5301` | Application database file, schema, catalog, or connection is invalid |
| `K5302` | Database transaction lifecycle, handle, or lock-safety rule was violated |
| `K5303` | Configured database count, byte, row, time, or busy bound was exceeded |
| `K5401` | Cache namespace is not configured, or refuses the requested operation |
| `K5402` | Configured cache key, value, entry, byte, or time-to-live bound was exceeded |
| `K5403` | Cache backend is unavailable |
| `K5404` | Search connector is missing, misconfigured, or returned invalid output |
| `K6001` | Invalid package manifest |
| `K6002` | Lockfile is stale or invalid |
| `K7001` | Residual or unresolved type requires specialization before layout |
| `K7002` | Core semantics or concrete layout is unsupported by the artifact target |
| `K7003` | WebAssembly emission, validation, metadata attachment, artifact loading, or output failed |
| `K7004` | Artifact metadata, byte size, or content digest mismatch |
| `K8001` | Source is not canonically formatted in formatter check mode |
| `K8002` | Language-server input or open-document state exceeds a bounded limit |
| `K8101` | Authoring assistance is disabled or provider configuration is invalid |
| `K8102` | Authoring context, range, path, ignore, redaction, or privacy validation failed |
| `K8103` | Authoring provider transport or strict response decoding failed |
| `K8104` | Authoring request, response, edit, proposal, digest, or stale precondition is invalid |
| `K8105` | Proposed source failed canonical formatting or compiler checking |
| `K8106` | Required authority expansion lacks exact review approval or a manifest grant |
| `K8107` | Proposal or accepted-source atomic I/O failed |

## Exit statuses

| Status | Meaning |
|---|---|
| `0` | Command completed successfully |
| `1` | Source, compile, runtime, test, or conformance failure |
| `2` | Invalid CLI usage |
| `3` | Package resolution, lockfile, registry, or build-plan failure |
| `4` | Capability, unavailable-host, or import authorization denial (`K5001`, `K5002`, or `K5003`); artifact-aware permission reports are still printed |
| `101` | Internal compiler error |

An internal compiler error is always a Krit bug. It must include a stable crash
identifier but must not include source content, environment variables, or
secrets in telemetry without explicit consent.

## Determinism and privacy

Diagnostics must not include:

- memory addresses
- nondeterministic hash ordering
- absolute cache paths
- secret values
- environment variable values
- capability contents
- model prompts or responses unless explicitly requested

The same failing program should produce the same diagnostic code and primary
span across supported platforms.
