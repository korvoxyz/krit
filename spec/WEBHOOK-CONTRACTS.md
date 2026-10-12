# Webhook agent contracts

**Status:** Normative compiler and bounded host contract
**Contract schema:** 1
**Runtime status:** Extended by `phase4-ai-observability`

## Source entrypoint

A source module contains zero or one direct top-level declaration:

```text
webhook fn NAME(request: HttpRequest) -> HttpResponse {
    // checked application logic
}
```

For example, a webhook that routes on the method and composes its response:

```krit
webhook fn handle(request: HttpRequest) -> HttpResponse {
    if request.method == "POST" {
        HttpResponse { status: 202, body: "queued \(request.path)" }
    } else {
        HttpResponse { status: 405, headers: ["allow": "POST"] }
    }
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

Source builds these records by name with spec-fixed defaults
(`HttpRequest { method, path }` defaults `query: ""`, `headers: []`, and
`body: ""`; `HttpResponse { status }` defaults `headers: []` and `body: ""`)
and writes header lists as entry lists (`["accept": "application/json"]`);
both mean exactly the full records (`LANGUAGE.md` sections 8.4 and 11).

## Request paths and queries

One path rule covers every `HttpRequest` the host accepts: an inbound webhook
request, every guest `http_request`, durable HTTP replay, and the request an
AI adapter or search connector builds. The path must be an origin-form
absolute path. It starts with `/` but not `//`, and it contains no `://`, no
space, no backslash, no `?` or `#`, and no ASCII control byte.

Before routing, servers and proxies commonly resolve dot segments, strip `;`
path parameters, trim whitespace, and percent-decode the path, sometimes twice
when a proxy decodes in front of a backend that decodes again, or once before
parsing the decoded target again. The rule refuses every path whose segment
structure one of those transforms could change. Each `/`-separated segment is
percent-decoded once, and the path is refused when:

- a segment is a dot segment: once decoded, its part before the first `;`, `?`,
  or `#`, with spaces trimmed from both ends, is exactly `.` or `..`. This
  covers `/a/./b`, `/a/../b`, `/..`, `/a/..;/b`, `/a/.;x=1/b`, `/a/..%3B/b`,
  `/a/..%3F/b` and `/a/..%23/b` (a server that parses the decoded target again
  reads the rest as a query or fragment), and `/a/..%20/b` and `/a/%20../b` (a
  server that trims the segment reads `..`);
- an escape decodes to `.`, `/`, `\`, or an ASCII control byte, in either case
  (`%2E`, `%2F`, `%5C`, `%00`-`%1F`, `%7F`), so `%2e%2e` and `.%2e` cannot spell
  a dot segment, `%2f` cannot split one segment into two, `%00` cannot truncate
  `..%00` to `..`, a trimming server cannot strip `..%09` or `..%0A` to `..`,
  and a decoded CR or LF cannot split a request line or header that a proxy
  rebuilds from the path. A raw control byte already breaks the shape rule;
- a decoded segment still contains a `%` followed by a hex digit, `u`, or `U`
  (`%252e`, `%2541`, `%25%32%65`, `%25u002e`), so a second decoding pass cannot
  produce another escaped byte at any depth;
- escapes spell an overlong UTF-8 sequence, which a lenient decoder reads as a
  shorter character: `%C0%AE` as `.` and `%C0%AF` as `/`. Lenient decoders
  often take the low six bits of each byte after a lead byte without checking
  that it is a continuation byte (the IIS 4 and 5 class), so `%C1%DC` reads as
  `\` and `%E0@n` as `.`. The rule therefore reads the byte after a lead the
  same way. It refuses `%C0` and `%C1` wherever they appear, because they begin
  only overlong forms and never occur in UTF-8. It also refuses `%E0`, `%F0`,
  and the five- and six-byte leads `%F8` and `%FC` when the low six bits of the
  next byte, raw or escaped, are below `20`, `10`, `08`, and `04` hex
  respectively, which makes the sequence overlong. A well-formed sequence such
  as `%E0%A4%85` or `%F0%9F%98%80` never trips the check;
- a `%` does not begin a well-formed `%HH` escape (`%`, `%2`, `%zz`,
  `%u002e`), because servers decode it in different ways.

Dots inside a segment (`/.well-known`, `/v1.2`, `/a..b`), `;` parameters on any
other segment (`/files;v=2`), a `%3F` or `%23` that does not end a dot segment
(`/C%23`, `/a%3Fb`), and every other well-formed escape remain valid: `%20`
beside other text, `%7E`, escaped UTF-8 such as `%C3%A9`, a lone escaped byte
that is not UTF-8 such as `%E9` (any byte but `%C0` and `%C1`, when the byte
after it does not make it overlong as described above), and a `%25` that no hex
digit, `u`, or `U` follows (`/100%25`, `/50%25off`). Raw non-ASCII text such as
`/café` is valid as well.

The rule does not model transforms that belong to one server, framework, or
platform: Unicode compatibility normalization (NFKC turns a fullwidth `．．`,
raw or escaped as `%EF%BC%8E%EF%BC%8E`, into `..`), case folding, trimming of
non-ASCII whitespace such as an escaped no-break space (`%C2%A0`), the Windows
trimming of trailing dots from a segment, and lenient UTF-8 decoders that
misread bytes in ways other than the six-bit reading above. A path that relies
on such a transform is accepted. An origin that applies one must enforce its
own confinement, because the host cannot.

The query may not start with `?` and may not contain a fragment, a space, a
control byte, or a backslash. Encoded separators in a query are ordinary data.

The origin is the only HTTP authority a program holds. This rule keeps a path
built from untrusted text, such as an inbound field or model output, from
escaping the prefix the program meant to address after a server resolves it.
Programs build such paths and queries with the standard library
(`LANGUAGE.md` section 12.1): `url_path_segment` returns `Err` for exactly
the texts whose percent-encoded segment this rule refuses (and the empty
text), so a program handles untrusted text as a value instead of trapping,
and every `url_query` result satisfies the query rule. The host and the
compiler share one definition of both rules. Warning `K9013` reports a path
or query piece that neither function encoded (`DIAGNOSTICS.md`).

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

## Outbound User-Agent

Every outbound request the host sends on a program's behalf identifies the
host. This covers a guest `http_request`, durable `replay_http`, and the
requests an AI adapter (`ai_invoke`, `replay_ai`) or a search connector builds.
When the request's header list has no `User-Agent` header, with names compared
without regard to letter case, the host appends `User-Agent: krit/<version>`.
`<version>` is the release version of the `krit-runtime` host crate, so a 0.3.0
host sends `krit/0.3.0`. The value is fixed when the host is built. It reveals
nothing about the program, the invocation, or the machine, and every retry of
one call sends the same value.

A guest-supplied `User-Agent` is sent exactly as written, in its own letter
case and position, and the host adds no default beside it. A guest `User-Agent`
with an empty value sends no `User-Agent` at all, because libcurl never sends a
header whose value is blank. This is how a program opts out. AI adapters and
search connectors build their own headers and never name a `User-Agent`, so
their requests always carry the default.

Like the bearer `Authorization` header, the default is added at the transport.
It does not count toward the request header limits, and it is not part of a
durable replay's input digest.

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

```text
config_string("agent.model") // Result<String, String>
secret("github-token")       // Result<Secret, String>
```

Outbound HTTP is the single fallible operation:

```text
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
Because a webhook can only run as a component, `krit check` also runs the
component target check for webhook files and reports every `K7001`/`K7002`
that `krit build` would.
Package build planning rejects missing exact manifest resources with `K5001`.
It does not yet relate secrets to origins; secret bindings are enforced by the
host at invocation time with `K5006`.
Direct `krit run` rejects webhook/config/secret/HTTP hosts with `K5003`;
runtime access is only through a validated component.

The bounded policy-2 compiler supports the fixed HTTP records (written as
records or by named construction), strings with `+`, `==`, `!=`, ordering,
interpolation, and text blocks, header lists (entry lists included),
Result/Option matching with nested patterns, postfix `?` in helpers, let-else
in the webhook body itself, the standard library, `json_decode` into
`String`, direct host calls, and static non-capturing helper references
(`WASM-SANDBOX.md`, "Implemented backend support"). User records and lists,
other JSON shapes, data captures, and residual layouts remain stable
`K7001`/`K7002` failures.

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
