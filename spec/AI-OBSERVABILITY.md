# AI invocation, observability, and stateless host policy

**Status:** Normative bounded runtime
**Contract schema:** 1
**Host configuration schema:** 2 (schema 1 remains accepted)
**Runtime milestone:** `phase4-ai-observability`

## Scope

This document defines the final Phase 4 host boundary. It adds one optional,
provider-neutral AI operation, structured application logs, bounded transport
retries, process-local rate and idempotency policy, embedding cancellation,
and explicit approval checks.

AI is neither language syntax nor a build dependency. Model output is
nondeterministic, private, and untrusted external input. The compiler and
runtime remain useful without an AI adapter, provider SDK, credential, or
network connection.

Durable state/replay and language-server work are outside this Phase 4
milestone and are specified by their later normative documents.

## Source contracts

Edition 2026 reserves these fallible host built-ins:

```krit
ai_invoke("reviewer", input) // Result<String, String>

log_info(
    "review.started",
    [record { name: "delivery", value: delivery }],
) // Result<Unit, String>

log_error("review.failed", [record { name: "reason", value: reason }])
// Result<Unit, String>
```

`LogField` is the closed built-in alias:

```text
LogField = Record { name: String, value: String }
```

The adapter argument and log event argument must be direct string literals.
Adapter and event names use 1-64 lowercase ASCII letters, digits, `.` or `-`,
without leading or trailing punctuation, `..`, or `--`. A nonliteral or
invalid name is `K3008`.

The logging field list is ordered. Field names and values are ordinary
strings. `Secret` cannot be placed in `LogField`, a list, or any other
structural value, so no logging operation can accept a secret handle.

The operations add these deterministic facts:

```text
ai_invoke  -> effect ai.invoke
              requirement ai.invoke("reviewer")
log_info   -> effect observe.log
log_error  -> effect observe.log
```

Effects and resource requirements propagate through calls and are sorted and
deduplicated in analysis, Core, explanation, and artifact metadata.
`observe.log` has no resource requirement; event names are stable source/Core
facts but are not grants.

`ai_invoke` returns only bounded raw UTF-8 text. The host does not execute,
evaluate, interpolate, deserialize into a trusted object, or otherwise grant
authority to that text. Source must explicitly validate or parse the returned
text before using it as structured data. The bounded policy-2 backend supports
only the documented concrete shapes. For the reference flow it validates and
decodes an unescaped JSON string when `json_decode` is inferred as
`String -> String`; escaped strings and unsupported JSON result shapes
continue to fail closed with `K7001`, `K7002`, or a bounded guest validation
trap.

Direct source execution has no AI or logging host and reports `K5003`.

## Manifest requests

Manifest schema 1 is compatibly extended:

```toml
[capabilities]
http = [
  "https://api.github.example",
  "https://ai.example",
  "https://message.example",
]
secrets = ["github-token", "ai-token", "message-token"]
ai = ["reviewer"]
logs = true
```

`ai` is a sorted-unique set of canonical adapter names. `logs` is a boolean.
The manifest requests authority but cannot configure adapters, approve an
operation, weaken network policy, raise a runtime limit, or add an artifact
import.

## Typed component interfaces

The checked-in `krit:runtime@0.2.0` package contains:

```wit
interface ai {
    invoke: func(adapter: string, input: string) -> result<string, string>;
}

interface logging {
    record field {
        name: string,
        value: string,
    }

    info: func(event: string, fields: list<field>) -> result<_, string>;
    error: func(event: string, fields: list<field>) -> result<_, string>;
}
```

Webhook worlds are a finite deterministic matrix over
`io.stdout`, `config.read`, `secret.read`, `http.request`, `ai.invoke`, and
`observe.log`. HTTP selects `http-anonymous` unless `secret.read` is also
present. No selected world imports an unused interface. In particular, an
AI-only artifact imports only `ai`, and a log-only artifact imports only
`logging`.

Component validation re-derives effects and the finite world from exact
imports and rejects unknown imports, underdeclared imports, duplicate imports,
unknown worlds, and adjacent/embedded metadata disagreement.

## Host configuration schema 2

Schema 1 host files containing only `config` and `secrets` remain parseable,
but their implicit host uses the default-deny approval policy. A schema-1
bearer operation therefore receives a visible approval denial. Migrate bearer
hosts to strict schema 2 with an exact `http.bearer` approval:

```json
{
  "schema": 2,
  "config": {},
  "secrets": {
    "ai-token": {"file": "secrets/ai-token"}
  },
  "aiAdapters": {
    "reviewer": {
      "kind": "http-json",
      "origin": "https://ai.example",
      "path": "/v1/invoke",
      "model": "review-model",
      "secret": "ai-token",
      "maxInputBytes": 65536,
      "maxResponseBytes": 65536,
      "timeoutMs": 750
    }
  },
  "approvals": [
    {"operation": "ai.invoke", "resource": "reviewer"},
    {"operation": "http.bearer", "resource": "https://api.github.example"}
  ],
  "retries": {
    "defaultHttp": {"maxAttempts": 1, "baseDelayMs": 25, "maxDelayMs": 200},
    "defaultAi": {"maxAttempts": 1, "baseDelayMs": 25, "maxDelayMs": 200},
    "http": {},
    "ai": {}
  },
  "rateLimits": {
    "defaultHttp": {"capacity": 64, "windowMs": 60000},
    "defaultAi": {"capacity": 16, "windowMs": 60000},
    "http": {},
    "ai": {},
    "maxTrackedResources": 128
  },
  "idempotency": {
    "maxEntries": 128,
    "maxBytes": 16777216,
    "ttlMs": 300000,
    "maxKeyBytes": 128
  }
}
```

Every object denies unknown fields. Maps are bounded and their keys must be
manifest-granted exact origins or adapter names. An adapter name must be
manifest-granted and artifact-required before use. Its exact origin and
optional secret must also be manifest-granted. Host configuration cannot add
a grant.

Configuration, secret, adapter, approval, retry, and rate collections are each
bounded to at most 256 entries, in addition to the selected host-config byte
limit.

Adapter transport origin and secret therefore appear in the manifest's
requested plan, while the component's direct requirement/import remains only
`ai.invoke(adapter)`. They do not add HTTP or secret guest imports. Runtime
validation intersects the host adapter mapping with those manifest requests
before instantiation.

The milestone adapter kind is `http-json`. It is implemented behind the
provider-neutral Rust adapter interface and uses this deterministic mapping:

```json
{"model":"configured-model","input":"raw source input"}
```

It sends `POST` with `content-type: application/json` and optional host-side
bearer authentication. A successful provider response is the strict object:

```json
{"output":"raw UTF-8 model text"}
```

Unknown response fields, malformed JSON, non-2xx status, non-UTF-8 data, an
oversized body, or an oversized output returns a redacted `Err(String)`.
Provider response bodies, prompts, model output, and credentials are never
included in diagnostics, default logs, metadata, permission output, cache
keys, or stats.

Adapter input, response, and timeout values must be nonzero and no greater
than both the selected runtime limits and the hard maxima. Paths are safe
origin-form absolute paths without query or fragment. Model identifiers are
bounded printable identifiers and are not artifact facts.

## Structured logging and publication

The host buffers each validated event inside the invocation:

```text
LogEvent {
    sequence: integer,
    level: "info" | "error",
    event: string,
    fields: ordered List<LogField>,
}
```

Default limits are 128 events, 32 fields per event, 64 bytes per event or
field name, 4 KiB per field value, and 64 KiB total encoded field bytes.
Hard maxima are 1,024 events, 128 fields, 64-byte names, 64 KiB values, and
1 MiB total bytes. Validation happens before the event buffer changes.
Field names use lowercase ASCII letters, digits, `.`, `-`, or `_`, beginning
and ending with a letter or digit.

Before buffering, names are compared case-insensitively after treating `_`
and `-` as equivalent. Keys containing `token`, `secret`, `password`,
`authorization`, or `api-key` have value `[REDACTED]`. As defense in depth,
an ordinary field value exactly equal to any configured secret byte sequence
that is valid UTF-8 is also replaced with `[REDACTED]`. The original is never
emitted. Host private values do not implement revealing `Debug`.

Successful invocation results expose `events` separately from stdout and the
webhook response. `krit invoke` keeps the exact response JSON on stdout and
publishes events as compact JSON Lines on stderr only after successful
completion. `krit serve` never places logs in an HTTP body. On a trapped or
failed invocation, normal stdout/response is discarded; already validated,
redacted events may be published on stderr with `"outcome":"failure"`.
Successful lines use `"outcome":"success"`.

## Retry policy

Retries are transport attempts inside the host HTTP/adapter operation. Guest
code is never re-entered and a webhook is never re-executed by retry policy.

- Default and minimum `maxAttempts` is 1.
- The hard maximum is 4 total attempts.
- Only DNS/connect/connection timeout failures and status
  `429`, `502`, `503`, or `504` are retryable.
- A request is retry-eligible only for `GET`/`HEAD`, or when it contains one
  valid ordinary `idempotency-key` header.
- Each logical AI adapter call receives a host-generated opaque
  `idempotency-key`; all transport attempts for that call reuse it, while
  separate calls receive different keys. This makes explicitly configured AI
  retries eligible without asking guest code to construct provider headers.
- Authentication/approval/rate/validation failures and every other 4xx status
  are not retried.
- Delay before attempt `n` is
  `min(maxDelay, baseDelay * 2^(n-2))`.
- A decimal-seconds `Retry-After` on a retryable response may raise that delay,
  but never above `maxDelay`. HTTP-date, malformed, duplicate, and overflowing
  values are ignored.
- No retry starts unless approval still allows the sensitive operation,
  cancellation is clear, and both the delay and next attempt fit the total
  invocation/adapter deadline.

Default delay is 25 ms and cap is 200 ms. The hard delay cap is 2 seconds.
Attempt count is exposed only as numeric invocation stats.

## Rate policy

An embedding-owned `AgentHost` contains bounded fixed-window counters keyed by
the exact HTTP origin or AI adapter. Defaults are finite: 64 HTTP attempts and
16 AI attempts per 60-second window. Capacity is at most 10,000, a window is
1 ms through 1 hour, and at most 256 resources may be tracked.

Configured entries override only their exact resource. The state uses bounded
LRU replacement when an embedding reuses one host across more artifacts than
the selected tracking bound. Rate exhaustion returns a visible operation
`Err`; it does not sleep and is distinct from capability or approval denial.
Per-invocation host-call, HTTP-call, AI-call, and deadline limits still apply.

Rate state is process-local and resets on restart. It is not billing,
distributed coordination, or durable abuse prevention.

## Cancellation

The embedding API exposes a clonable cancellation handle backed by atomics.
Cancellation is checked before component instantiation, before every host
call, before every retry, and during backoff. libcurl's progress callback
aborts an active transfer when cancellation is requested or the absolute
deadline expires. Epoch interruption remains the guest execution deadline;
it is not relied upon to interrupt a blocking host call.

Cancellation before guest execution is runtime error `K5106`. Cancellation
observed by a fallible AI or HTTP call is a visible `Err(String)` so guest code
may handle it.

## Inbound idempotency

For methods other than `GET`, `HEAD`, `OPTIONS`, and `TRACE`, `invoke` and
`serve` recognize one case-insensitive `Idempotency-Key` header. A key is
1-128 visible ASCII bytes using letters, digits, `.`, `_`, `:`, or `-`.
Duplicate or invalid keys are rejected before guest execution.
CLI serving returns 400 for that rejection.

`AgentHost` keeps only completed successful webhook responses in a bounded
process-local TTL/LRU store. The default is 128 entries, 16 MiB retained
response data, and five minutes; hard maxima are 1,024 entries, 64 MiB, and
one hour. A response larger than the selected total byte budget is not cached.
A matching key and request digest replays the exact response without creating
a Store or performing guest side effects. The same key with a different
digest returns 409. Traps, runtime failures, rejected requests, and incomplete
responses are never cached.

Cache keys are scoped to the artifact package/version, world, and component
digest. The request digest covers bounded method, path, query, every header
name/value (including credential-bearing headers), and body, excluding only
the idempotency key itself. Only the BLAKE3 digest is retained; raw credential
headers are not copied into the cache. Concurrent first use can execute more
than once because no in-progress record is stored.

This remains the default best-effort single-process behavior. Host config
schema 3 may instead select one manifest-granted SQLite store for durable
leases and completed inbound responses as defined by `DURABLE-STATE.md`.
Neither mode claims distributed exactly-once delivery.

## Approval policy

AI adapters and bearer-authenticated HTTP origins are sensitive operations.
The library accepts an `ApprovalPolicy` callback receiving only operation kind
and exact resource name, never prompt, response, request body, or secret.

CLI schema 2 supports only explicit noninteractive allow entries. There are no
prompts, and server mode never waits for a person. An undeclared sensitive
operation is denied. Approval is checked before secret lookup or network I/O
and again before every retry. Denial is a visible operation `Err`, distinct
from capability denial.

Source, manifests, adapter declarations, and dependency metadata cannot
self-approve. Artifact-aware permissions report `approvalRequired` separately
from grants and report `approvalStatus: "not-evaluated"` unless an embedding
explicitly supplies policy evaluation. Deployment policy remains
`not-evaluated`.

## Reference gate and limitations

The auditable reference webhook:

1. receives a typed webhook;
2. calls one exact GitHub-like origin;
3. invokes one named neutral AI adapter;
4. treats the returned text as untrusted and explicitly validates/parses it;
5. posts through one exact messaging-like origin;
6. emits structured lifecycle events;
7. handles every fallible host result; and
8. returns a typed response.

All acceptance tests use bounded loopback mock servers and placeholder secret
files. No test or default example requires Internet access or a real
credential.

Phase 4 does not make model output deterministic, make approval a deployment
grant, or provide distributed rate limiting. Later phases add optional
authoring and durable local state without changing this AI/logging contract.
