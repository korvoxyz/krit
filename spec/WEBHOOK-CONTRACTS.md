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
Unknown fields and ungranted names fail closed. Unix secret files must grant
no group/other permissions. Secret bytes stay in a host-owned zeroizing store
and only a Wasmtime resource handle enters the guest.

Outbound HTTP verifies the exact origin on every call, disables redirects,
pins the one request's DNS result, rejects non-public IPv4/IPv6 ranges
including private, shared, documentation, benchmark, link-local, loopback,
reserved, and metadata addresses by default, validates TLS certificates and
hostnames, and
enforces DNS/connect/read/overall timeouts plus call/header/body limits. An
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
