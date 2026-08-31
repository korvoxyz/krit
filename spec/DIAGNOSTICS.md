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
| `K2001` | Undefined name |
| `K2002` | Duplicate binding or parameter |
| `K4001` | Wrong runtime value kind |
| `K4002` | Calling a non-function |
| `K4003` | Function argument count mismatch |
| `K4004` | Division or remainder by zero |
| `K4005` | Integer overflow |
| `K4006` | Function comparison |
| `K4007` | Output operation failed |
| `K5001` | Required capability not granted |
| `K6001` | Invalid package manifest |
| `K6002` | Lockfile is stale or invalid |

## Exit statuses

| Status | Meaning |
|---|---|
| `0` | Command completed successfully |
| `1` | Source, compile, runtime, test, or conformance failure |
| `2` | Invalid CLI usage |
| `3` | Package resolution, lockfile, registry, or build-plan failure |
| `4` | Capability grant or sandbox setup failure |
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
