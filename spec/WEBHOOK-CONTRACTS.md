# Webhook agent contracts

**Status:** Normative compiler contract  
**Contract schema:** 1  
**Runtime status:** Unavailable until `phase4-http-runtime`

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
contain exactly its three fields. The future HTTP host validates status range,
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

Each call requires one direct valid resource literal and contributes both a
coarse effect and an exact requirement pair:

```text
config.read("agent.model")
secret.read("github-token")
```

Effects and requirement pairs are independently sorted and deduplicated.
Function calls propagate both summaries transitively. `Secret` is an opaque
language/Core identity and a WIT resource. It is never a string or byte
sequence and cannot be printed, compared, JSON-encoded, structurally stored,
or revealed.

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
Direct `krit run` rejects unavailable webhook/config/secret hosts with
`K5003`. Even with matching resources, `krit build` rejects these layouts with
`K7002`. No command opens a socket, performs HTTP/TLS, loads a value, reveals a
secret, or invokes AI in this milestone.
