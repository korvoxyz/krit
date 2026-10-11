# Webhook agent contracts

**Status:** Normative compiler and bounded host contract
**Contract schema:** 1
**Runtime status:** Extended by `phase4-ai-observability`

## Source entrypoint

A source module contains zero or one direct top-level declaration:

```krit
webhook fn NAME(request: HttpRequest) -> HttpResponse {
    // checked application logic
}
```

`webhook` is reserved. The declaration is a named exported-host entrypoint,
not an ambient effect and not a request to listen on a socket. Nested
declarations are `K1004`, duplicates are `K2002`, and every signature other
than exactly one `HttpRequest` parameter plus `HttpResponse` result is
`K3007`.

## Fixed language types

Contract schema 1 defines:

```text
HttpHeader   = Record { name: String, value: String }
HttpRequest  = Record {
    method: String,
    path: String,
    query: String,
    headers: List<HttpHeader>,
    body: String,
}
HttpResponse = Record {
    status: Int,
    headers: List<HttpHeader>,
    body: String,
}
```

These are built-in closed aliases, not user-defined type syntax. Header list
order is preserved and duplicate names are representable. A response must
contain exactly its three fields. The HTTP host validates status range,
request limits, and protocol syntax.

## Request paths and queries

One path rule covers every `HttpRequest` the host accepts: an inbound webhook
request, every guest `http_request`, durable HTTP replay, and the request an
AI adapter or search connector builds. The path must be an origin-form
absolute path. It starts with `/` but not `//`, and it contains no `://`, no
space, no backslash, no `?` or `#`, and no ASCII control byte.

Before routing, servers and proxies commonly resolve dot segments, strip `;`
path parameters, and percent-decode the path, sometimes twice when a proxy
decodes in front of a backend that decodes again. The rule refuses every path
whose segment structure one of those transforms could change. Each
`/`-separated segment is percent-decoded once, and the path is refused when:

- a segment is a dot segment: once decoded, its part before the first `;` is
  exactly `.` or `..`. This covers `/a/./b`, `/a/../b`, `/..`, `/a/..;/b`,
  `/a/.;x=1/b`, and `/a/..%3B/b`;
- an escape decodes to `.`, `/`, `\`, or NUL, in either case (`%2E`, `%2F`,
  `%5C`, `%00`, `%2e`, `%2f`, `%5c`), so `%2e%2e` and `.%2e` cannot spell a dot
  segment, `%2f` cannot split one segment into two, and `%00` cannot truncate
  `..%00` to `..`;
- a decoded segment still contains a `%` followed by a hex digit or `u`
  (`%252e`, `%2541`, `%25%32%65`, `%25u002e`), so a second decoding pass cannot
  produce another escaped byte at any depth;
- escapes begin an overlong UTF-8 sequence: `%C0` or `%C1` before a
  continuation byte, `%E0` before `%80`-`%9F`, `%F0` before `%80`-`%8F`, or the
  five- and six-byte forms `%F8` before `%80`-`%87` and `%FC` before
  `%80`-`%83`. A lenient decoder reads `%C0%AE` as `.` and `%C0%AF` as `/`;
- a `%` does not begin a well-formed `%HH` escape (`%`, `%2`, `%zz`,
  `%u002e`), because servers decode it in different ways.

Dots inside a segment (`/.well-known`, `/v1.2`, `/a..b`), `;` parameters on any
other segment (`/files;v=2`), and every other well-formed escape remain valid:
`%20`, `%7E`, escaped UTF-8 such as `%C3%A9`, a lone escaped byte that is not
UTF-8 such as `%E9`, and a `%25` that no hex digit or `u` follows (`/100%25`,
`/50%25off`). Raw non-ASCII text such as `/café` is valid as well.

The rule does not model transforms that belong to one server, framework, or
platform: Unicode compatibility normalization (NFKC turns a fullwidth `．．`,
raw or escaped as `%EF%BC%8E%EF%BC%8E`, into `..`), case folding, or the Windows
trimming of trailing dots and spaces from a segment. A path that relies on
such a transform is accepted. An origin that applies one must enforce its own
confinement, because the host cannot.

The query may not start with `?` and may not contain a fragment, a space, a
control byte, or a backslash. Encoded separators in a query are ordinary data.

The origin is the only HTTP authority a program holds. This rule keeps a path
built from untrusted text, such as an inbound field or model output, from
escaping the prefix the program meant to address after a server resolves it.

A violation is `K4001`, and the diagnostic never echoes the path. An inbound
request is refused before guest execution. A guest `http_request` or durable
`replay_http` traps the invocation before approval, before the replay journal
is consulted, and before DNS resolution or any connection. An AI adapter or
search connector path that breaks the rule is refused earlier, when the host
configuration is validated (`K7003` for an adapter, `K5404` for a connector).

### What reaches the wire

The host never resolves dot segments in an accepted path and never decodes its
escapes, and it disables libcurl's own dot-segment normalization
(`CURLOPT_PATH_AS_IS`). Dot segments, `;` parameters, and every escape
therefore reach the origin with the meaning the host checked. libcurl still
respells the path in two ways that leave its decoded bytes unchanged. It writes
the hex digits of every escape in uppercase, so `%7e` is sent as `%7E`. It also
percent-encodes each raw non-ASCII byte, so `/café` is sent as `/caf%C3%A9`.
The query is sent byte-for-byte. A program that signs its request path should
sign this spelling, or use uppercase escapes and escape non-ASCII bytes itself.
libcurl refuses a raw space anywhere in the request target, which is why the
rule refuses one before approval rather than letting the transfer fail later.

## Request JSON Schema

The exact schema-1 request document is:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "https://krit.dev/schemas/webhook/request-1.json",
  "title": "Krit HttpRequest contract v1",
  "type": "object",
  "additionalProperties": false,
  "required": ["method", "path", "query", "headers", "body"],
  "properties": {
    "body": {"type": "string"},
    "headers": {
      "type": "array",
      "items": {"$ref": "#/$defs/HttpHeader"}
    },
    "method": {"type": "string"},
    "path": {"type": "string"},
    "query": {"type": "string"}
  },
  "$defs": {
    "HttpHeader": {
      "type": "object",
      "additionalProperties": false,
      "required": ["name", "value"],
      "properties": {
        "name": {"type": "string"},
        "value": {"type": "string"}
      }
    }
  }
}
```

## Response JSON Schema

The exact schema-1 response document is:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "https://krit.dev/schemas/webhook/response-1.json",
  "title": "Krit HttpResponse contract v1",
  "type": "object",
  "additionalProperties": false,
  "required": ["status", "headers", "body"],
  "properties": {
    "body": {"type": "string"},
    "headers": {
      "type": "array",
      "items": {"$ref": "#/$defs/HttpHeader"}
    },
    "status": {"type": "integer"}
  },
  "$defs": {
    "HttpHeader": {
      "type": "object",
      "additionalProperties": false,
      "required": ["name", "value"],
      "properties": {
        "name": {"type": "string"},
        "value": {"type": "string"}
      }
    }
  }
}
```

The compiler serializes these typed structures deterministically. Object
member order is stable implementation output; JSON consumers must use normal
object semantics.

## Host-operation identities

```krit
config_string("agent.model") // Result<String, String>
secret("github-token")       // Result<Secret, String>
```

Outbound HTTP is the single fallible operation:

```krit
http_request(
    "https://api.example.com",
    request,
    None,
) // Result<HttpResponse, String>
```

The third argument is directly `None` or `Some(secret_handle)`. The latter is
the only ordinary source position allowed to wrap a `Secret`; the host injects
`Authorization: Bearer <secret>` without placing secret bytes in guest memory.

The host injects a secret only into an origin its manifest binding allows. A
secret declared by a `[secrets.<name>]` table may reach only the origins that
table lists; a secret named only by the legacy `[capabilities] secrets` list is
unbound and may reach any granted origin (see `CAPABILITIES.md`). The secret
handle carries its logical name, and `http_request` checks the pairing
immediately after the origin grant, before the call limit, approval, DNS
resolution, or any request byte. A misdirected pairing, such as a GitHub token
sent to a messaging origin, traps the invocation with `K5006`; it is not a
guest-visible `Err`, and nothing is sent.

Each host call requires a direct valid resource literal and contributes both a
coarse effect and an exact requirement pair:

```text
config.read("agent.model")
secret.read("github-token")
http.request("https://api.example.com")
```

Effects and requirement pairs are independently sorted and deduplicated.
Function calls propagate both summaries transitively. HTTP origins use the
same parser as manifests: lowercase `http`/`https`, normalized host and
effective port, no default-port spelling, userinfo, trailing slash, path,
query, or fragment. `Secret` is an opaque language/Core identity and a WIT
resource. It is never a string or byte sequence and cannot be printed,
compared, JSON-encoded, structurally stored, or revealed.

## Explanation fact

Schema-1 explanation JSON retains the existing synthetic `entrypoint` field
and adds:

```json
{
  "entrypoints": {
    "schema": 1,
    "items": [
      {
        "name": "handle",
        "kind": "webhook",
        "functionId": 1,
        "signature": "webhook fn handle(request: HttpRequest) -> HttpResponse",
        "effects": ["config.read"],
        "capabilityRequirements": [
          {"capability": "config.read", "resource": "agent.model"}
        ],
        "contract": {
          "schema": 1,
          "requestType": "HttpRequest",
          "responseType": "HttpResponse",
          "requestSchema": {},
          "responseSchema": {}
        }
      }
    ]
  }
}
```

The two schema placeholders above stand for the exact documents in this
specification. The actual fact also includes the synthetic module-init item
before a source webhook item.

## Runtime and build boundary

`krit check` and `krit explain` implement these contracts without a manifest.
Package build planning rejects missing exact manifest resources with `K5001`.
It does not yet relate secrets to origins; secret bindings are enforced by the
host at invocation time with `K5006`.
Direct `krit run` rejects webhook/config/secret/HTTP hosts with `K5003`;
runtime access is only through a validated component.

The bounded policy-2 compiler supports the fixed HTTP records, strings, header
lists, Result/Option matching, direct host calls, and static non-capturing
helper references needed by the reference webhook. Unsupported composites,
JSON, data captures, and residual layouts remain stable `K7001`/`K7002`
failures.

`krit invoke --request FILE` accepts the exact request JSON schema and prints
only the exact response JSON after a successful fresh invocation.
`krit serve` loads the same existing artifact, sidecar, manifest, and optional
host config, binds loopback by default, and creates a fresh invocation per
request. `--once` serves one accepted or rejected request for deterministic
tests.

Host configuration is explicit immutable JSON. Schema 1 contains string
config and relative secret-file references; schema 2 compatibly adds the
bounded AI/reliability/approval policy defined in `AI-OBSERVABILITY.md`.
Unknown fields and ungranted names fail closed. A host-injected credential (an
AI adapter or search connector `secret`) that the manifest binds must list that
adapter's or connector's origin, so host configuration cannot widen a binding
(`K5006`). Unix secret files must grant no group/other permissions. Secret
bytes stay in a host-owned zeroizing store and only a Wasmtime resource handle
enters the guest.

Outbound HTTP verifies the exact origin on every call, never resolves or
decodes the validated path, disables redirects, pins the one request's DNS
result, rejects non-public IPv4/IPv6 ranges including private, shared,
documentation, benchmark, link-local, loopback, reserved, and metadata
addresses by default, validates TLS certificates and hostnames, and enforces
DNS/connect/read/overall timeouts plus call/header/body limits. An
embedding-only test policy may allow loopback; a separate explicit policy is
required to send bearer authentication over plain HTTP. Neither switch is
available to guest code or manifests.

WIT 0.2 names the webhook and HTTP records separately but gives them identical
field order and canonical shape. Reusing types from the function-owning
`webhook` interface would make that callable interface an implicit import.
The duplication is deliberate least authority; compiler/runtime tests enforce
the semantic identity at both adapters. Authenticated and anonymous HTTP are
also separate WIT interfaces so `None` does not silently grant secret
acquisition.

`AI-OBSERVABILITY.md` normatively extends this contract with provider-neutral
AI invocation, structured redacted logging, retries, rate limits,
cancellation, process-local idempotency, and approval policy. The HTTP record,
secret, origin, rollback, and fresh-Store rules in this document remain
unchanged.
