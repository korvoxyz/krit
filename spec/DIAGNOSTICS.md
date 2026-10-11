# Krit diagnostic contract

**Status:** Normative  
**Schema:** 1

Diagnostics are part of Krit's human and AI tooling interface. Their codes and
structured fields are stable within an edition; prose may improve without
breaking compatibility.

## Severity

Every diagnostic a command reports has a severity:

| Severity | Meaning |
|---|---|
| `error` | The program or input is rejected and the command fails |
| `warning` | The program is accepted; the diagnostic reports something the author should change |

A warning never changes a command's outcome or exit status: a program whose
only diagnostics are warnings checks, runs, builds, and is explained exactly as
it would without them. `krit check`, `krit run`, `krit build`, and
`krit explain` report every warning that analysis finds, ordered by source
span and then code, on standard error in the selected diagnostic format. When
analysis also rejects the program, the warnings found before the error are
reported first and the error last. The language server publishes warnings with
LSP `DiagnosticSeverity.Warning` and errors with `DiagnosticSeverity.Error`,
and `krit/compilerFacts` diagnostics carry the same `severity` strings as the
JSON Lines format. A published LSP diagnostic's message is the diagnostic's
message followed by each of its notes on its own line as `help: <note>`, as in
the human format; `krit/compilerFacts` diagnostics carry the message and a
`notes` array as the JSON Lines format does. Both are bounded as described
next.

The language server bounds what it publishes for one document, so that the
diagnostics of a document within its size limit fit in a protocol frame. It
keeps the first 100 warnings in the order above, and stops before the first
warning that would take their combined published message text, notes
included, past 64 KiB. When it leaves warnings out, it publishes one `K8003`
notice after the warnings it kept: the notice sits at the span of the first
warning left out, has LSP `DiagnosticSeverity.Information`, and states how
many warnings were left out. The error that rejects the document, if any, is
always published, last; its message is cut to 64 KiB and each of its notes to
4 KiB, at character boundaries, ending with a marker saying the text was cut.
`krit/compilerFacts` returns the same list, with severity `info` for the
notice. Should a document's diagnostics still not fit in a protocol frame, the
server publishes a single `K8002` error for the document in their place and
keeps running. The commands above bound neither warnings nor messages.

The schema reserves `info` for informational diagnostics. Only the language
server's `K8003` notice uses it; commands never report an `info` diagnostic on
standard error.

## Human format

The default format is:

```text
path/to/file.krit:4:11: error[K2001]: undefined name `totl`
help: did you mean `total`?
path/to/file.krit:7:9: warning[K2003]: `_` is a discard; reading it will be an error in edition 2027
```

The severity word comes first, followed by the code in brackets; warnings use
the same location style as errors. Each of the diagnostic's notes follows on
its own line, in order, as `help: <note>`. An implementation may add source
excerpts after these lines. It must not expose a host-language stack trace
unless explicitly requested with a developer option.

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
| `notes` | array of strings | One-line repair hints, in order, that have no precise source range (see [Fix hints](#fix-hints)) |

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
| `K9000`-`K9999` | Lints: warnings by default |

Codes outside the lint range are errors unless the table below gives them
another severity, and that severity is part of the code's contract within an
edition. Lints report readability, safety, and authority hazards in programs
that are otherwise valid. A lint is a warning by default; a project profile or
command mode may configure a different severity for a specific lint, and that
configuration is then part of the profile's or mode's contract. No profile or
mode configures lint severity yet, so every lint is currently a warning.

Initial required codes:

| Code | Meaning |
|---|---|
| `K0001` | Invalid source character or token |
| `K0002` | Unterminated string |
| `K0003` | Invalid string escape |
| `K1001` | Unexpected token, including a `?` that is not a conditional operator (`K1110`) |
| `K1002` | Expected token not found |
| `K1003` | Invalid match pattern |
| `K1004` | Webhook declaration is nested or otherwise misplaced |
| `K1005` | Nesting depth limit exceeded: syntax nests more than 256 levels, nesting plus the `else if` continuations, operators, and call or field-access links above a point exceeds 4,096, or an inferred type would nest more than 256 levels |
| `K1101` | `return`: a block's value is its final expression |
| `K1102` | `for`, `while`, `loop`, or `do` loop |
| `K1103` | Assignment, compound assignment (`+=`, `-=`, `*=`, `/=`, `%=`), `++` or `--`, `mut`, `var`, or `const` |
| `K1104` | `struct`, `class`, `impl`, `trait`, or `interface` declaration |
| `K1105` | `import`, `use`, `require`, or `from`: modules are not available |
| `K1106` | `async` or `await` |
| `K1107` | `try`, `catch`, `throw`, `raise`, `except`, or `finally` |
| `K1108` | Indexing `a[i]` |
| `K1109` | Arrow function `(x) => e` or `x => e` |
| `K1110` | Conditional operator `c ? a : b` |
| `K1111` | `switch`, `case`, or `default` |
| `K1112` | `return` opening an `else` block: an early exit |
| `K2001` | Undefined name; a note suggests the closest visible name, `None` for a spelling of null, or the fix for a construct from another language |
| `K2002` | Duplicate binding or parameter; the discard name `_` is exempt |
| `K2003` | Warning: the discard name `_` is read as a value; this becomes an error in edition 2027 |
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
| `K3011` | Method call `value.name(...)` on a value that has no field `name`: Krit values have no methods |
| `K4001` | Generic guest trap, wrong runtime value kind, or HTTP request/response the host refuses (including a request path that breaks the rule in `WEBHOOK-CONTRACTS.md`, "Request paths and queries") |
| `K4002` | Calling a non-function |
| `K4003` | Function argument count mismatch |
| `K4004` | Division or remainder by zero |
| `K4005` | Integer overflow |
| `K4006` | Function comparison |
| `K4007` | Output operation failed |
| `K4008` | Value cannot be encoded as JSON |
| `K4009` | Invalid or unsupported JSON input, in direct evaluation or a component's `json_decode` into `String` |
| `K4010` | Direct-evaluation depth limit exceeded: more than 10,000 active function calls, 100,000 nested evaluations, or 10,000 levels of list, record, and variant value nesting |
| `K5001` | Required capability not granted |
| `K5002` | Artifact import/world does not match the authorized host interface |
| `K5003` | Required host contract is unavailable in direct source execution |
| `K5006` | Secret would be injected into an origin its manifest binding does not list |
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
| `K6003` | Manifest declares dependencies, but dependency resolution is not supported |
| `K7001` | Residual or unresolved type requires specialization before layout |
| `K7002` | Core semantics or concrete layout is unsupported by the artifact target |
| `K7003` | WebAssembly emission, validation, metadata attachment, artifact loading, or output failed |
| `K7004` | Artifact metadata, byte size, or content digest mismatch, or adjacent metadata outside the runtime's supported compatibility sets or toolchain window |
| `K8001` | Source is not canonically formatted in formatter check mode |
| `K8002` | Language-server input, open-document state, or a document's published diagnostics exceed a bounded limit |
| `K8003` | Info: the language server left a document's remaining warnings out of its published diagnostics at the bounded limit |
| `K8101` | Authoring assistance is disabled or provider configuration is invalid |
| `K8102` | Authoring context, range, path, ignore, redaction, or privacy validation failed |
| `K8103` | Authoring provider transport or strict response decoding failed |
| `K8104` | Authoring request, response, edit, proposal, digest, or stale precondition is invalid |
| `K8105` | Proposed source failed canonical formatting or compiler checking |
| `K8106` | Required authority expansion lacks exact review approval or a manifest grant |
| `K8107` | Proposal or accepted-source atomic I/O failed |

## Build-target diagnostics

`K7001` and `K7002` are reported by `krit build` and by `krit check` when it
runs a target check (`--target component|module`, or by default for files
that declare a webhook, queue, or schedule entrypoint). Both commands run the
same backend pipeline and report the same set. The backend runs only on a
program that passes front-end checking, so its diagnostics follow any
warnings that analysis reported (see [Severity](#severity)). Unlike front-end
analysis, which stops at its first error, a failing target check reports every
residual type and unsupported construct in the code that the selected
entrypoint can reach. Before that support check, the backend refuses Core
nested deeper than its compilers accept with a single `K1005`
(`WASM-SANDBOX.md`, "Support checking"). Each diagnostic caused by a source
construct carries the span of the originating operation (or of the enclosing
parameter, block, or declaration) and is never reported at a placeholder
`1:1` position. They are emitted in source order (span start, span end, then
code) with at most one diagnostic per code and source position. `K7002`
messages name the layout policy of the program kind: "WebAssembly policy 1"
for module programs and "the bounded webhook ABI (WebAssembly policy 2)" for
entrypoint programs. Diagnostics about a file whose text is unavailable, such
as an invalid manifest (`K6001`), use the file's first position.

## Fix hints

A diagnostic's notes are one-line repair hints. Their wording is prose and
may improve, but which diagnostics carry one is part of each code's contract:

- `K1101`-`K1112` and `K3011` always carry exactly one note with the fix.
- `K2001` carries one note when one of the rules below applies.
- `K3002` for a call of `None` carries one note: `None` is a value, written
  without parentheses.
- `K1001` for a `?` after an operand carries one note: postfix `?` is not
  available yet.

### Constructs from other languages

Models and people write constructs from other languages out of habit. Krit
reports each with its own syntax code, under the rules of `LANGUAGE.md`
section 4.2, instead of a generic `K1001` or `K1002`:

| Code | Example | Fix in the note |
|---|---|---|
| `K1101` | `return total;` | a block's value is its final expression; for early exit, `?` and let-else arrive in Krit 0.3 |
| `K1102` | `for item in items { ... }` | write a recursive function over `[]` and `[head, ..tail]`; list functions arrive in Krit 0.4 |
| `K1103` | `count = count + 1;`, `x += 1`, `let mut x` | values are immutable; bind each new value with `let` under a new name |
| `K1104` | `struct Point { x: Int }` | use a `record { field: value }` value and plain functions |
| `K1105` | `import json;` | modules are not available yet; built-in functions need no import |
| `K1106` | `await fetch()` | host calls finish before they return; call them directly |
| `K1107` | `throw "empty";`, `try { ... }` | return `Ok(value)` or `Err(error)` and handle the `Result` with `match` |
| `K1108` | `items[0]` | match the list with `[]` and `[head, ..tail]`; `list_get` arrives in Krit 0.4 |
| `K1109` | `(x) => x + 1` | write `fn(x) { x + 1 }` |
| `K1110` | `ready ? 1 : 2` | `if` is an expression: `if ready { 1 } else { 2 }` |
| `K1111` | `switch code { case 200: ... }` | use `match`, or an `if ... else if ... else` chain |
| `K1112` | `else { return 0; }` | exit early with let-else (arriving in Krit 0.3) |

```text
main.krit:2:5: error[K1101]: `return` is not Krit syntax
help: a block's value is its final expression, written without `return` or `;`; for early exit, `?` and let-else arrive in Krit 0.3
```

### Undefined names

`K2001` carries the first of these notes that applies:

1. For `null`, `NULL`, `Null`, `nil`, `Nil`, `NIL`, `undefined`, or
   `nullptr`: write `None` for an absent `Option` value.
2. For `return`, `await`, `throw`, `raise`, `require`, or `import`, which
   parse as names where they could still be one (`return;`, `return -1;`,
   `require("json")`): the fix of the construct the word comes from.
3. `did you mean `name`?`, naming the closest visible name: a binding in
   scope at the reference, a built-in name, `true`, or `false`. A name
   qualifies when it is at most two edits from the undefined name (inserting,
   deleting, or substituting one character, or swapping two adjacent ones) and
   fewer edits than the undefined name has characters. Fewer edits win, then
   the bytewise smallest name, so the suggestion never depends on declaration
   order.

```text
main.krit:2:9: error[K2001]: undefined name `null`
help: Krit has no `null`; write `None` for an absent `Option` value
```

### Method calls

Krit values have no methods. `value.name(arguments)` calls the function
stored in field `name` of the record `value`; when `value` has no field
`name`, because it is not a record or is a record without that field,
checking reports `K3011` instead of the field-access error `K3004`. A field
access that is not called still reports `K3004`. The note names the
standard-library function that does the job when there is one:

| Receiver | Methods | Note names |
|---|---|---|
| `String` | `len`, `length`, `size`, `count`, `byte_len` | `string_byte_len(s)` (counts UTF-8 bytes) |
| `String` | `is_empty`, `isEmpty`, `empty` | `string_is_empty(s)` |
| `String` | `contains`, `includes` | `string_contains(s, part)` |
| `String` | `starts_with`, `startsWith`, `startswith`, `hasPrefix` | `string_starts_with(s, prefix)` |
| `String` | `ends_with`, `endsWith`, `endswith`, `hasSuffix` | `string_ends_with(s, suffix)` |
| `String` | any other | the five string functions |
| `Int` | `to_string`, `toString`, `string`, `str` | `int_to_string(n)` |
| `List` | any | list patterns; list functions arrive in Krit 0.4 |
| `Option`, `Result` | any | `match` |
| other | any | passing the value to a function |

The note spells the receiver as written when it is a name or a short field
path (`string_byte_len(request.body)`), and as `value` otherwise.

```text
main.krit:2:9: error[K3011]: cannot call `.len()` on a value of type `String`: Krit values have no methods
help: call `string_byte_len(name)` instead; it counts UTF-8 bytes (standard library, arriving in Krit 0.3)
```

## Exit statuses

| Status | Meaning |
|---|---|
| `0` | Command completed successfully |
| `1` | Source, compile, runtime, test, or conformance failure |
| `2` | Invalid CLI usage |
| `3` | Package resolution, lockfile, registry, or build-plan failure |
| `4` | Capability, unavailable-host, import, or secret-binding authorization denial (`K5001`, `K5002`, `K5003`, or `K5006`); artifact-aware permission reports are still printed |
| `101` | Internal compiler error |

Warnings never affect the exit status.

An internal compiler error is always a Krit bug. It must include a stable crash
identifier but must not include source content, environment variables, or
secrets in telemetry without explicit consent.

`krit run` checks source exactly as `krit check` does before evaluating it,
except that it never runs a build-target check (see
[Build-target diagnostics](#build-target-diagnostics)): direct execution builds
no artifact. For a program that fails checking it emits check's diagnostic and
exits with check's status, so it reports runtime codes (`K4000`-`K4999`) only
for programs that pass checking.

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
